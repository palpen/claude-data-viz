//! SSH session establishment and shared connection state for a `(host, user, port)` triple.
//!
//! One `RemoteConnection` is shared by every active watch on the same host. It owns:
//!   - the russh session handle (multiplexes channels — file pollers and the transcript
//!     poller all open their own SFTP subsystem channels off the same session),
//!   - the per-host `SharedIndex` that the transcript poller writes into and file-watch
//!     enrichment reads from,
//!   - the transcript poller's task handle (cancelled on Drop).
//!
//! Host-key handling: russh's `Handler::check_server_key` is synchronous on the handshake
//! path (we cannot `await` a UI prompt from inside it without deadlocking the session
//! setup). We solve this with a probe-then-verify split:
//!
//!   - `probe_host_key` connects with `HostKeyPolicy::Capture`, which accepts whatever the
//!     server presents and stashes it in a sink, then tears down without authenticating.
//!   - The frontend asks the user to confirm the fingerprint and on Yes we append to
//!     `~/.ssh/known_hosts` via `known_hosts::learn_host`.
//!   - All real connections (`connect`) use `HostKeyPolicy::Strict`, which consults
//!     known_hosts via `verify_host` and fails closed (`Ok(false)` → handshake aborts).

use crate::ssh::config::ResolvedHost;
use crate::ssh::known_hosts::{self, HostVerdict};
use crate::transcript::{self, SharedIndex};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use russh::client::{self, Handle};
use russh_keys::agent::client::AgentClient;
use russh_keys::ssh_key::PublicKey;
use russh_sftp::client::SftpSession;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const KEEPALIVE_SECS: u64 = 30;
const CONNECT_TIMEOUT_SECS: u64 = 15;

/// What `ClientHandler::check_server_key` does when the server presents its host key.
#[derive(Clone)]
pub enum HostKeyPolicy {
    /// Compare against an ordered list of known_hosts files. Reject (return `Ok(false)`,
    /// which russh translates into a handshake error) on Mismatch, Revoked, or Absent.
    Strict {
        host: String,
        port: u16,
        files: Vec<PathBuf>,
    },
    /// Accept any presented key and stash it in `sink`. Used for probing — caller must
    /// tear down the session without attempting auth.
    Capture {
        sink: Arc<tokio::sync::Mutex<Option<PublicKey>>>,
    },
}

#[derive(Clone)]
pub struct ClientHandler {
    pub policy: HostKeyPolicy,
}

#[async_trait::async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        match &self.policy {
            HostKeyPolicy::Capture { sink } => {
                *sink.lock().await = Some(server_public_key.clone());
                Ok(true)
            }
            HostKeyPolicy::Strict { host, port, files } => {
                let paths: Vec<&Path> = files.iter().map(|p| p.as_path()).collect();
                let verdict = known_hosts::verify_host(&paths, host, *port, server_public_key);
                let ok = matches!(verdict, HostVerdict::Match);
                if !ok {
                    let fp = known_hosts::fingerprint(server_public_key);
                    eprintln!(
                        "ssh: rejecting host key for {host}:{port} — verdict={verdict:?} fingerprint={fp}"
                    );
                }
                Ok(ok)
            }
        }
    }
}

pub struct RemoteConnection {
    /// Kept for diagnostics — surfaces in error messages and logs.
    #[allow(dead_code)]
    pub host: String,
    #[allow(dead_code)]
    pub user: String,
    #[allow(dead_code)]
    pub port: u16,
    pub session: Mutex<Handle<ClientHandler>>,
    pub transcript_index: SharedIndex,
    cancel: Arc<AtomicBool>,
    transcript_task: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

impl RemoteConnection {
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// Open a fresh SFTP session on this SSH connection. Each caller gets their own channel
    /// so a slow listdir on one watch can't block the transcript poller on another.
    pub async fn open_sftp(&self) -> Result<SftpSession> {
        let session = self.session.lock().await;
        let channel = session
            .channel_open_session()
            .await
            .context("opening session channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("requesting sftp subsystem")?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .context("starting sftp session")?;
        Ok(sftp)
    }

    pub fn store_transcript_task(&self, handle: JoinHandle<()>) {
        if let Some(prev) = self.transcript_task.lock().replace(handle) {
            prev.abort();
        }
    }
}

impl Drop for RemoteConnection {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.transcript_task.lock().take() {
            h.abort();
        }
    }
}

/// Connect to `(host, user, port)` and authenticate via ssh-agent. The returned connection's
/// transcript poller is NOT yet started — caller should call `start_transcript_poller` after
/// inserting into the registry. (Avoids a circular borrow at construction time.)
///
/// Always uses `HostKeyPolicy::Strict { files: known_hosts::default_files() }`. If the host
/// key isn't in known_hosts, the handshake fails — the caller (e.g. the frontend Test flow)
/// must first probe + confirm + learn.
pub async fn connect(host: ResolvedHost, user_override: Option<String>) -> Result<RemoteConnection> {
    let user = user_override
        .or(host.user.clone())
        .ok_or_else(|| anyhow!("no user — set User in ~/.ssh/config or pass explicitly"))?;

    let mut config = client::Config::default();
    config.inactivity_timeout = Some(Duration::from_secs(KEEPALIVE_SECS * 4));
    config.keepalive_interval = Some(Duration::from_secs(KEEPALIVE_SECS));
    let config = Arc::new(config);

    let addr = (host.host_name.as_str(), host.port);

    let handler = ClientHandler {
        policy: HostKeyPolicy::Strict {
            host: host.host_name.clone(),
            port: host.port,
            files: known_hosts::default_files(),
        },
    };

    let mut handle = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        client::connect(config, addr, handler),
    )
    .await
    .context("connect timed out")?
    .map_err(|e| anyhow!("ssh connect failed: {e} (host key may not be in ~/.ssh/known_hosts — probe + confirm via the dialog first)"))?;

    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| anyhow!("no ssh-agent: {e}. Run `ssh-add ~/.ssh/id_ed25519`."))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| anyhow!("agent request_identities failed: {e}"))?;
    if identities.is_empty() {
        return Err(anyhow!(
            "ssh-agent has no keys. Run `ssh-add ~/.ssh/id_ed25519` and try again."
        ));
    }

    let mut authed = false;
    let mut last_err: Option<String> = None;
    for ident in identities {
        match handle
            .authenticate_publickey_with(user.clone(), ident, &mut agent)
            .await
        {
            Ok(true) => {
                authed = true;
                break;
            }
            Ok(false) => {
                last_err = Some("agent key rejected".into());
            }
            Err(e) => {
                last_err = Some(format!("agent sign failed: {e}"));
            }
        }
    }
    if !authed {
        return Err(anyhow!(
            "ssh authentication failed: {}",
            last_err.unwrap_or_else(|| "no key accepted by server".into())
        ));
    }

    Ok(RemoteConnection {
        host: host.host_name.clone(),
        user,
        port: host.port,
        session: Mutex::new(handle),
        transcript_index: transcript::new_index(),
        cancel: Arc::new(AtomicBool::new(false)),
        transcript_task: parking_lot::Mutex::new(None),
    })
}

/// Wall-clock now in milliseconds since epoch.
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Connect with a Capture policy, snapshot the presented host key, and tear the session down
/// without attempting authentication. Used by the Test flow before we ask the user to confirm
/// trust. Returns an error if TCP / handshake fails or the server somehow gave us no key
/// (russh's contract guarantees `check_server_key` runs once before the handler returns).
pub async fn probe_host_key(host: &str, port: u16) -> Result<PublicKey> {
    let mut config = client::Config::default();
    config.inactivity_timeout = Some(Duration::from_secs(CONNECT_TIMEOUT_SECS));
    let config = Arc::new(config);

    let sink: Arc<tokio::sync::Mutex<Option<PublicKey>>> = Arc::new(tokio::sync::Mutex::new(None));
    let handler = ClientHandler {
        policy: HostKeyPolicy::Capture { sink: sink.clone() },
    };

    let handle = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        client::connect(config, (host, port), handler),
    )
    .await
    .context("probe timed out")?
    .context("probe ssh connect failed")?;

    // Drop the session immediately — we never ran auth.
    drop(handle);

    let captured = sink.lock().await.take();
    captured.ok_or_else(|| anyhow!("server did not present a host key"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_keys::parse_public_key_base64;
    use std::fs;
    use tempfile::TempDir;

    const KEY_A_B64: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B_B64: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    #[tokio::test]
    async fn handler_strict_rejects_unknown() {
        let dir = TempDir::new().unwrap();
        let kh = dir.path().join("known_hosts");
        // known_hosts holds KEY_B; we'll present KEY_A and expect rejection.
        fs::write(&kh, format!("box.example ssh-ed25519 {}\n", KEY_B_B64)).unwrap();

        let presented = parse_public_key_base64(KEY_A_B64).unwrap();
        let mut handler = ClientHandler {
            policy: HostKeyPolicy::Strict {
                host: "box.example".into(),
                port: 22,
                files: vec![kh],
            },
        };
        let ok = client::Handler::check_server_key(&mut handler, &presented)
            .await
            .unwrap();
        assert!(!ok, "Strict policy should reject a key not in known_hosts");
    }

    #[tokio::test]
    async fn handler_strict_accepts_match() {
        let dir = TempDir::new().unwrap();
        let kh = dir.path().join("known_hosts");
        fs::write(&kh, format!("box.example ssh-ed25519 {}\n", KEY_A_B64)).unwrap();

        let presented = parse_public_key_base64(KEY_A_B64).unwrap();
        let mut handler = ClientHandler {
            policy: HostKeyPolicy::Strict {
                host: "box.example".into(),
                port: 22,
                files: vec![kh],
            },
        };
        let ok = client::Handler::check_server_key(&mut handler, &presented)
            .await
            .unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn handler_capture_stores_key_and_returns_ok() {
        let sink: Arc<tokio::sync::Mutex<Option<PublicKey>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let presented = parse_public_key_base64(KEY_A_B64).unwrap();
        let mut handler = ClientHandler {
            policy: HostKeyPolicy::Capture { sink: sink.clone() },
        };
        let ok = client::Handler::check_server_key(&mut handler, &presented)
            .await
            .unwrap();
        assert!(ok);
        let captured = sink.lock().await.clone();
        assert!(captured.is_some());
        assert_eq!(captured.unwrap(), presented);
    }
}
