//! SSH session establishment and shared connection state for a `(host, user, port)` triple.
//!
//! One `RemoteConnection` is shared by every active watch on the same host. It owns:
//!   - the russh session handle (multiplexes channels — file pollers and the transcript
//!     poller all open their own SFTP subsystem channels off the same session),
//!   - the per-host `SharedIndex` that the transcript poller writes into and file-watch
//!     enrichment reads from,
//!   - the transcript poller's task handle (cancelled on Drop).
//!
//! v1 host-key handling: we accept any presented key and log its fingerprint. Real
//! known_hosts verification is v1.5 (the comment trail in `config::lookup_known_host`
//! tracks the half-built bits).

use crate::ssh::config::ResolvedHost;
use crate::transcript::{self, SharedIndex};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use russh::client::{self, Handle};
use russh_keys::agent::client::AgentClient;
use russh_sftp::client::SftpSession;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const KEEPALIVE_SECS: u64 = 30;
const CONNECT_TIMEOUT_SECS: u64 = 15;

#[derive(Clone)]
pub struct ClientHandler;

#[async_trait::async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh_keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO(v1.5): verify against ~/.ssh/known_hosts (plain + hashed) and TOFU on first
        // connect with user confirmation. For v1 we accept all keys but log the fingerprint
        // so a careful user can spot a swap.
        let fp = server_public_key
            .fingerprint(russh_keys::ssh_key::HashAlg::Sha256)
            .to_string();
        eprintln!("ssh: server key fingerprint = {fp}");
        Ok(true)
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
pub async fn connect(host: ResolvedHost, user_override: Option<String>) -> Result<RemoteConnection> {
    let user = user_override
        .or(host.user.clone())
        .ok_or_else(|| anyhow!("no user — set User in ~/.ssh/config or pass explicitly"))?;

    let mut config = client::Config::default();
    config.inactivity_timeout = Some(Duration::from_secs(KEEPALIVE_SECS * 4));
    config.keepalive_interval = Some(Duration::from_secs(KEEPALIVE_SECS));
    let config = Arc::new(config);

    let addr = (host.host_name.as_str(), host.port);

    let mut handle = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        client::connect(config, addr, ClientHandler),
    )
    .await
    .context("connect timed out")?
    .context("ssh connect failed")?;

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
