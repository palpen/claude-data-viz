use crate::fs_watcher;
use crate::persistence;
use crate::ssh::{
    self, cache, config as ssh_config, connection, known_hosts, registry::RemoteKey,
    watcher as ssh_watcher,
};
use crate::state::{mark_history_dirty, AppState, WatchHandle};
use crate::types::{
    HostKeyChangedInfo, InitialState, RecentRemote, RemoteDirListing, SshAgentProbe, SshHostEntry,
    TestResult, TestStage, UnknownHostInfo, VizGone, VizItem, Watch, WatchSource, WatchStatus,
};
use chrono::Utc;
use globset::Glob;
use russh_keys::agent::client::AgentClient;
use russh_sftp::client::SftpSession;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

#[tauri::command]
pub fn get_state(state: State<Arc<AppState>>) -> InitialState {
    let watches = state.watches.lock().clone();
    let mut items: Vec<VizItem> = state.items.lock().values().cloned().collect();
    items.sort_by_key(|i| -i.mtime);
    InitialState {
        watches,
        items,
        follow_latest: *state.follow_latest.lock(),
        selected: state.selected_id.lock().clone(),
    }
}

#[derive(Deserialize)]
pub struct AddLocalWatchArgs {
    pub path: String,
    pub session_path: Option<String>,
}

#[tauri::command]
pub fn add_local_watch(
    app: AppHandle,
    state: State<Arc<AppState>>,
    args: AddLocalWatchArgs,
) -> Result<Watch, String> {
    let id = Uuid::new_v4().to_string();
    let root = PathBuf::from(&args.path);

    let watch = Watch {
        id: id.clone(),
        source: WatchSource::Local {
            path: args.path.clone(),
        },
        // Kept for back-compat with persisted state; the global tail watches every session.
        session_path: args.session_path.clone(),
    };
    state.watches.lock().push(watch.clone());

    fs_watcher::start_watch(app.clone(), state.inner().clone(), id, root)
        .map_err(|e| e.to_string())?;

    persistence::save_prefs(&app, state.inner());
    Ok(watch)
}

/// Re-run a local watch's cold scan (file enumeration + dedupe). For SSH watches this is a
/// no-op — the SFTP poller already scans on its own cadence and a one-shot extra pass would
/// race with it. Errors are swallowed and surfaced via existing event channels.
#[tauri::command]
pub async fn rescan_watch(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    watch_id: String,
) -> Result<(), String> {
    let source = state
        .watches
        .lock()
        .iter()
        .find(|w| w.id == watch_id)
        .map(|w| w.source.clone());
    let Some(source) = source else {
        return Err(format!("watch {watch_id} not found"));
    };
    if let WatchSource::Local { path } = source {
        let root = PathBuf::from(path);
        if !root.is_dir() {
            return Err("watch path is not a directory".into());
        }
        let state_clone = state.inner().clone();
        fs_watcher::cold_scan(&app, &state_clone, &watch_id, &root).await;
    }
    Ok(())
}

#[tauri::command]
pub fn remove_watch(app: AppHandle, state: State<Arc<AppState>>, watch_id: String) {
    fs_watcher::stop_watch(state.inner(), &watch_id);
    state.watches.lock().retain(|w| w.id != watch_id);
    persistence::save_prefs(&app, state.inner());
    persistence::save_history(&app, state.inner());
}

#[tauri::command]
pub fn set_follow_latest(app: AppHandle, state: State<Arc<AppState>>, value: bool) {
    *state.follow_latest.lock() = value;
    persistence::save_prefs(&app, state.inner());
}

#[tauri::command]
pub fn set_selected(
    app: AppHandle,
    state: State<Arc<AppState>>,
    watch_id: Option<String>,
    abs_path: Option<String>,
) {
    let new = match (watch_id, abs_path) {
        (Some(w), Some(p)) => Some((w, p)),
        _ => None,
    };
    *state.selected_id.lock() = new;
    persistence::save_prefs(&app, state.inner());
}

#[tauri::command]
pub fn clear_gallery(app: AppHandle, state: State<Arc<AppState>>) {
    state.items.lock().clear();
    *state.selected_id.lock() = None;
    mark_history_dirty(state.inner());
    persistence::save_prefs(&app, state.inner());
    persistence::save_history(&app, state.inner());
}

// ----- SSH commands ------------------------------------------------------------------------

#[tauri::command]
pub async fn probe_ssh_agent() -> SshAgentProbe {
    match AgentClient::connect_env().await {
        Ok(mut agent) => match agent.request_identities().await {
            Ok(ids) => SshAgentProbe {
                available: true,
                key_count: ids.len() as u32,
                error: None,
            },
            Err(e) => SshAgentProbe {
                available: false,
                key_count: 0,
                error: Some(format!("agent reachable but request_identities failed: {e}")),
            },
        },
        Err(e) => SshAgentProbe {
            available: false,
            key_count: 0,
            error: Some(format!("no ssh-agent: {e}")),
        },
    }
}

#[tauri::command]
pub fn list_ssh_hosts() -> Vec<SshHostEntry> {
    ssh_config::list_hosts()
}

#[derive(Deserialize)]
pub struct TestSshArgs {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Empty / missing → resolve to the SFTP cwd (the user's home dir on the remote).
    #[serde(default)]
    pub remote_path: Option<String>,
    pub glob: String,
}

/// Resolve an optional remote path to a concrete absolute one. If the user left it blank, we
/// canonicalize "." which the SFTP server roots at the authenticated user's home dir. This is
/// what `sftp host` lands you in interactively, so it matches mental model.
async fn resolve_remote_path(
    sftp: &SftpSession,
    requested: &Option<String>,
) -> Result<String, String> {
    let trimmed = requested.as_deref().map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        sftp.canonicalize(".")
            .await
            .map_err(|e| format!("resolve home dir: {e}"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn ok_stage() -> TestStage {
    TestStage {
        ok: true,
        error: None,
        matched_files: None,
    }
}

fn err_stage(msg: impl Into<String>) -> TestStage {
    TestStage {
        ok: false,
        error: Some(msg.into()),
        matched_files: None,
    }
}

fn skip_stage() -> TestStage {
    TestStage {
        ok: false,
        error: Some("skipped".into()),
        matched_files: None,
    }
}

#[tauri::command]
pub async fn test_ssh_connection(args: TestSshArgs) -> TestResult {
    // Resolve via ssh_config so aliases compose with Host * defaults.
    let resolved = ssh_config::resolve(&args.host);
    let user = args.user.clone().or(resolved.user.clone());
    let port = args.port.unwrap_or(resolved.port);

    // Stage 1 — TCP reachability.
    let reachable = match tokio::time::timeout(
        Duration::from_secs(8),
        tokio::net::TcpStream::connect((resolved.host_name.as_str(), port)),
    )
    .await
    {
        Ok(Ok(_)) => ok_stage(),
        Ok(Err(e)) => {
            return TestResult {
                reachable: err_stage(format!("tcp connect: {e}")),
                authenticated: skip_stage(),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
        Err(_) => {
            return TestResult {
                reachable: err_stage("tcp connect timed out"),
                authenticated: skip_stage(),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
    };

    // Stage 1.5 — Host-key probe + verification. We do this BEFORE attempting auth so the
    // user never sends agent-signed challenges to an unverified host. Strict policy will
    // reject anyway, but the explicit probe gives us a fingerprint to show.
    let host_for_probe = resolved.host_name.clone();
    let probed_key = match connection::probe_host_key(&host_for_probe, port).await {
        Ok(k) => k,
        Err(e) => {
            return TestResult {
                reachable,
                authenticated: err_stage(format!("host-key probe failed: {e}")),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
    };

    let kh_files = known_hosts::default_files();
    let kh_paths: Vec<&std::path::Path> = kh_files.iter().map(|p| p.as_path()).collect();
    let verdict = known_hosts::verify_host(&kh_paths, &host_for_probe, port, &probed_key);

    match &verdict {
        known_hosts::HostVerdict::Match => {
            // Continue to auth.
        }
        known_hosts::HostVerdict::Absent => {
            let raw = probed_key.to_openssh().unwrap_or_default();
            let key_type = probed_key.algorithm().as_str().to_string();
            return TestResult {
                reachable,
                authenticated: err_stage("unknown host — confirm to trust"),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: Some(UnknownHostInfo {
                    host: host_for_probe.clone(),
                    port,
                    fingerprint: known_hosts::fingerprint(&probed_key),
                    key_type,
                    raw_openssh: raw,
                }),
                host_key_changed: None,
            };
        }
        known_hosts::HostVerdict::Mismatch { line_no, .. }
        | known_hosts::HostVerdict::Revoked { line_no, .. } => {
            // Best-effort: pull the recorded key off the matching line so the UI can show
            // the "known" fingerprint. If parsing fails we fall back to a placeholder.
            let known_fp = known_fingerprint_at_line(&kh_files, *line_no)
                .unwrap_or_else(|| "unknown".to_string());
            return TestResult {
                reachable,
                authenticated: err_stage("host key changed — see ssh-keygen -R"),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: Some(HostKeyChangedInfo {
                    host: host_for_probe.clone(),
                    port,
                    fingerprint_offered: known_hosts::fingerprint(&probed_key),
                    fingerprint_known: known_fp,
                    known_hosts_line: *line_no as u64,
                }),
            };
        }
    }

    // Stage 2 — Auth via ssh-agent.
    let user_resolved = match user {
        Some(u) => u,
        None => {
            return TestResult {
                reachable,
                authenticated: err_stage("no user — set User in ~/.ssh/config or pass explicitly"),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
    };
    let mut resolved_for_connect = resolved.clone();
    resolved_for_connect.port = port;
    let conn = match connection::connect(resolved_for_connect, Some(user_resolved.clone())).await {
        Ok(c) => c,
        Err(e) => {
            return TestResult {
                reachable,
                authenticated: err_stage(format!("auth: {e}")),
                path_exists: skip_stage(),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
    };
    let authenticated = ok_stage();

    // Stage 3 — Path exists (SFTP stat).
    let sftp = match conn.open_sftp().await {
        Ok(s) => s,
        Err(e) => {
            return TestResult {
                reachable,
                authenticated,
                path_exists: err_stage(format!("sftp open: {e}")),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
    };
    let resolved_path = match resolve_remote_path(&sftp, &args.remote_path).await {
        Ok(p) => p,
        Err(e) => {
            return TestResult {
                reachable,
                authenticated,
                path_exists: err_stage(e),
                matched: skip_stage(),
                unknown_host: None,
                host_key_changed: None,
            };
        }
    };
    if let Err(e) = sftp.metadata(&resolved_path).await {
        return TestResult {
            reachable,
            authenticated,
            path_exists: err_stage(format!("path stat: {e}")),
            matched: skip_stage(),
            unknown_host: None,
            host_key_changed: None,
        };
    }
    let path_exists = ok_stage();

    // Stage 4 — Matched files. Light scan: just the immediate readdir + glob filter, no
    // recursion — same pattern won't catch subdirs but the count is illustrative.
    let matched = match Glob::new(&args.glob) {
        Err(e) => err_stage(format!("invalid glob: {e}")),
        Ok(g) => {
            let matcher = g.compile_matcher();
            match sftp.read_dir(&resolved_path).await {
                Ok(entries) => {
                    let count = entries
                        .into_iter()
                        .filter(|e| {
                            let n = e.file_name();
                            !n.is_empty() && matcher.is_match(&n)
                        })
                        .count() as u32;
                    TestStage {
                        ok: true,
                        error: None,
                        matched_files: Some(count),
                    }
                }
                Err(e) => err_stage(format!("read_dir: {e}")),
            }
        }
    };

    TestResult {
        reachable,
        authenticated,
        path_exists,
        matched,
        unknown_host: None,
        host_key_changed: None,
    }
}

/// Read the known_hosts file containing the conflict line and compute the fingerprint of the
/// stored key on that line. Best-effort — returns None on any parse failure so the UI just
/// shows a placeholder.
fn known_fingerprint_at_line(files: &[std::path::PathBuf], line_no: usize) -> Option<String> {
    use russh_keys::parse_public_key_base64;
    for file in files {
        let Ok(contents) = std::fs::read_to_string(file) else {
            continue;
        };
        let Some(line) = contents.lines().nth(line_no.saturating_sub(1)) else {
            continue;
        };
        // Strip optional `@marker` prefix to isolate the host_field key_type key_b64 columns.
        let working = line
            .trim()
            .strip_prefix('@')
            .map(|rest| rest.splitn(2, char::is_whitespace).nth(1).unwrap_or(""))
            .unwrap_or(line.trim());
        let mut parts = working.split_whitespace();
        let _host_field = parts.next()?;
        let _key_type = parts.next()?;
        let key_b64 = parts.next()?;
        if let Ok(k) = parse_public_key_base64(key_b64) {
            return Some(known_hosts::fingerprint(&k));
        }
    }
    None
}

#[derive(Deserialize)]
pub struct AddRemoteWatchArgs {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Empty / missing → resolve to the user's home dir on the remote.
    #[serde(default)]
    pub remote_path: Option<String>,
    pub glob: String,
}

#[tauri::command]
pub async fn add_remote_watch(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    args: AddRemoteWatchArgs,
) -> Result<Watch, String> {
    let resolved = ssh_config::resolve(&args.host);
    let user = args
        .user
        .clone()
        .or(resolved.user.clone())
        .ok_or_else(|| {
            "no user — set User in ~/.ssh/config or pass it explicitly".to_string()
        })?;
    let port = args.port.unwrap_or(resolved.port);
    let host_name = resolved.host_name.clone();

    let key = RemoteKey::new(host_name.clone(), user.clone(), port);
    let mut resolved_for_connect = resolved.clone();
    resolved_for_connect.port = port;
    let conn = state
        .remote_connections
        .acquire(&key, resolved_for_connect, &app, state.inner())
        .await
        .map_err(|e| e.to_string())?;

    // Resolve home-dir default before storing — Watch is the source of truth, and we don't
    // want it carrying empty/relative paths through reconnects or restarts.
    let resolved_path = {
        let sftp = conn.open_sftp().await.map_err(|e| {
            state.remote_connections.release(&key);
            format!("sftp open: {e}")
        })?;
        let p = resolve_remote_path(&sftp, &args.remote_path).await.map_err(|e| {
            state.remote_connections.release(&key);
            e
        })?;
        drop(sftp);
        p
    };

    let watch_id = Uuid::new_v4().to_string();
    let watcher = ssh_watcher::start(
        app.clone(),
        state.inner().clone(),
        conn,
        key.clone(),
        watch_id.clone(),
        resolved_path.clone(),
        args.glob.clone(),
    )
    .map_err(|e| {
        // Roll back the acquire so we don't leak a refcount on a failed start.
        state.remote_connections.release(&key);
        e.to_string()
    })?;

    let watch = Watch {
        id: watch_id.clone(),
        source: WatchSource::Ssh {
            host: args.host.clone(),
            user: user.clone(),
            port,
            remote_path: resolved_path.clone(),
            glob: args.glob.clone(),
        },
        session_path: None,
    };

    state.watches.lock().push(watch.clone());
    state.watch_handles.lock().insert(
        watch_id.clone(),
        WatchHandle {
            id: watch_id.clone(),
            watcher: Box::new(watcher),
        },
    );

    persistence::record_recent_remote(
        state.inner(),
        RecentRemote {
            host: args.host.clone(),
            user,
            port,
            remote_path: resolved_path,
            glob: args.glob.clone(),
            last_used_ms: Utc::now().timestamp_millis(),
        },
    );
    persistence::save_prefs(&app, state.inner());
    Ok(watch)
}

#[tauri::command]
pub fn list_recent_remotes(state: State<Arc<AppState>>) -> Vec<RecentRemote> {
    state.recent_remotes.lock().clone()
}

#[tauri::command]
pub fn forget_recent_remote(
    app: AppHandle,
    state: State<Arc<AppState>>,
    host: String,
    user: String,
    port: u16,
    remote_path: String,
) {
    state.recent_remotes.lock().retain(|r| {
        !(r.host == host && r.user == user && r.port == port && r.remote_path == remote_path)
    });
    persistence::save_prefs(&app, state.inner());
}

#[tauri::command]
pub async fn fetch_remote_file(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    watch_id: String,
    abs_path: String,
) -> Result<String, String> {
    // Resolve the watch's RemoteKey from persisted state.
    let source = state
        .watches
        .lock()
        .iter()
        .find(|w| w.id == watch_id)
        .map(|w| w.source.clone())
        .ok_or_else(|| format!("watch {watch_id} not found"))?;

    let (host, user, port) = match &source {
        WatchSource::Ssh {
            host, user, port, ..
        } => (host.clone(), user.clone(), *port),
        WatchSource::Local { .. } => {
            return Err("watch is local — abs_path is already accessible".into());
        }
    };
    let resolved = ssh_config::resolve(&host);
    let host_name = resolved.host_name.clone();
    let key = RemoteKey::new(host_name, user, port);
    let conn = state
        .remote_connections
        .get(&key)
        .ok_or_else(|| "remote connection is not active".to_string())?;

    // Fetch metadata first (cheap) so the cache check is precise.
    let sftp = conn.open_sftp().await.map_err(|e| e.to_string())?;
    let meta = sftp
        .metadata(&abs_path)
        .await
        .map_err(|e| format!("stat: {e}"))?;
    drop(sftp); // release SFTP channel before the (potentially serialized) fetch.
    let size = meta.size.unwrap_or(0);
    let mtime_ms = meta
        .mtime
        .map(|t| (t as i64).saturating_mul(1000))
        .unwrap_or(0);

    let local = cache::fetch_file(
        &app,
        &state.fetch_locks,
        &state.fetch_semaphore,
        &watch_id,
        &conn,
        &abs_path,
        mtime_ms,
        size,
    )
    .await
    .map_err(|e| e.to_string())?;

    // If HTML, opportunistically pull siblings (depth=1, capped). Failure logged, not fatal.
    if abs_path.to_lowercase().ends_with(".html") || abs_path.to_lowercase().ends_with(".htm") {
        if let Err(e) = cache::fetch_html_siblings(
            &app,
            &state.fetch_locks,
            &state.fetch_semaphore,
            &watch_id,
            &conn,
            &abs_path,
        )
        .await
        {
            eprintln!("ssh: html sibling fetch failed: {e}");
        }
    }

    Ok(local.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn get_watch_status(state: State<Arc<AppState>>, watch_id: String) -> Option<WatchStatus> {
    state
        .watch_handles
        .lock()
        .get(&watch_id)
        .map(|h| h.watcher.status())
}

#[tauri::command]
pub async fn reconnect_watch(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    watch_id: String,
) -> Result<(), String> {
    // Snapshot the source for this watch, then drop the existing handle (which releases the
    // connection refcount), then start a fresh one.
    let source = state
        .watches
        .lock()
        .iter()
        .find(|w| w.id == watch_id)
        .map(|w| w.source.clone())
        .ok_or_else(|| format!("watch {watch_id} not found"))?;
    let WatchSource::Ssh {
        host,
        user,
        port,
        remote_path,
        glob,
    } = source
    else {
        return Err("watch is not an SSH source".into());
    };

    state.watch_handles.lock().remove(&watch_id);

    let resolved = ssh_config::resolve(&host);
    let host_name = resolved.host_name.clone();
    let key = RemoteKey::new(host_name, user.clone(), port);
    let mut resolved_for_connect = resolved.clone();
    resolved_for_connect.port = port;
    let conn = state
        .remote_connections
        .acquire(&key, resolved_for_connect, &app, state.inner())
        .await
        .map_err(|e| e.to_string())?;
    let watcher = ssh_watcher::start(
        app.clone(),
        state.inner().clone(),
        conn,
        key.clone(),
        watch_id.clone(),
        remote_path,
        glob,
    )
    .map_err(|e| {
        state.remote_connections.release(&key);
        e.to_string()
    })?;
    state.watch_handles.lock().insert(
        watch_id.clone(),
        WatchHandle {
            id: watch_id,
            watcher: Box::new(watcher),
        },
    );
    Ok(())
}

/// Retarget an existing SSH watch at a new directory. Reuses the live RemoteConnection (no
/// re-auth), validates the new path, swaps the poller, and clears stale items + emits viz:gone
/// for the frontend so the sidebar doesn't show entries from the old folder.
#[tauri::command]
pub async fn update_remote_watch_path(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    watch_id: String,
    new_path: Option<String>,
) -> Result<Watch, String> {
    let source = state
        .watches
        .lock()
        .iter()
        .find(|w| w.id == watch_id)
        .map(|w| w.source.clone())
        .ok_or_else(|| format!("watch {watch_id} not found"))?;
    let WatchSource::Ssh {
        host,
        user,
        port,
        remote_path: _,
        glob,
    } = source
    else {
        return Err("watch is not an SSH source".into());
    };

    let resolved_cfg = ssh_config::resolve(&host);
    let key = RemoteKey::new(resolved_cfg.host_name.clone(), user.clone(), port);

    // Bump the refcount BEFORE dropping the old watcher. This keeps the SSH session alive
    // even if this is the only watch on this host — otherwise we'd tear it down only to
    // re-auth a moment later.
    let mut resolved_for_connect = resolved_cfg.clone();
    resolved_for_connect.port = port;
    let conn = state
        .remote_connections
        .acquire(&key, resolved_for_connect, &app, state.inner())
        .await
        .map_err(|e| e.to_string())?;

    // Validate the new path. Empty → home dir.
    let resolved_path = {
        let sftp = conn.open_sftp().await.map_err(|e| {
            state.remote_connections.release(&key);
            format!("sftp open: {e}")
        })?;
        let p = match resolve_remote_path(&sftp, &new_path).await {
            Ok(p) => p,
            Err(e) => {
                state.remote_connections.release(&key);
                return Err(e);
            }
        };
        if let Err(e) = sftp.metadata(&p).await {
            state.remote_connections.release(&key);
            return Err(format!("path stat: {e}"));
        }
        p
    };

    // Drop the existing watcher (releases the old refcount; ours from acquire above remains).
    state.watch_handles.lock().remove(&watch_id);

    // Clear items for this watch_id and notify the frontend so the sidebar updates immediately.
    let stale_paths: Vec<String> = {
        let mut items = state.items.lock();
        let stale: Vec<String> = items
            .keys()
            .filter(|(wid, _)| wid == &watch_id)
            .map(|(_, p)| p.clone())
            .collect();
        for p in &stale {
            items.remove(&(watch_id.clone(), p.clone()));
        }
        stale
    };
    if !stale_paths.is_empty() {
        mark_history_dirty(state.inner());
        for p in stale_paths {
            let _ = app.emit(
                "viz:gone",
                VizGone {
                    watch_id: watch_id.clone(),
                    abs_path: p,
                },
            );
        }
    }

    let watcher = ssh_watcher::start(
        app.clone(),
        state.inner().clone(),
        conn,
        key.clone(),
        watch_id.clone(),
        resolved_path.clone(),
        glob.clone(),
    )
    .map_err(|e| {
        state.remote_connections.release(&key);
        e.to_string()
    })?;

    state.watch_handles.lock().insert(
        watch_id.clone(),
        WatchHandle {
            id: watch_id.clone(),
            watcher: Box::new(watcher),
        },
    );

    // Update the persisted Watch source.
    let updated = {
        let mut watches = state.watches.lock();
        let entry = watches
            .iter_mut()
            .find(|w| w.id == watch_id)
            .ok_or_else(|| format!("watch {watch_id} vanished"))?;
        entry.source = WatchSource::Ssh {
            host: host.clone(),
            user: user.clone(),
            port,
            remote_path: resolved_path.clone(),
            glob: glob.clone(),
        };
        entry.clone()
    };

    persistence::record_recent_remote(
        state.inner(),
        RecentRemote {
            host,
            user,
            port,
            remote_path: resolved_path,
            glob,
            last_used_ms: Utc::now().timestamp_millis(),
        },
    );
    persistence::save_prefs(&app, state.inner());
    Ok(updated)
}

/// List subdirectories at `path` on the remote so the frontend can render a folder browser
/// when the user is picking which directory to watch. `path` is None / empty → SFTP home dir.
/// Skips well-known noise dirs (node_modules, .git, target, etc.) — same list the watcher uses.
#[tauri::command]
pub async fn list_remote_dirs(
    state: State<'_, Arc<AppState>>,
    watch_id: String,
    path: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<RemoteDirListing, String> {
    let source = state
        .watches
        .lock()
        .iter()
        .find(|w| w.id == watch_id)
        .map(|w| w.source.clone())
        .ok_or_else(|| format!("watch {watch_id} not found"))?;
    let (host, user, port) = match &source {
        WatchSource::Ssh {
            host, user, port, ..
        } => (host.clone(), user.clone(), *port),
        WatchSource::Local { .. } => {
            return Err("watch is local — use the OS folder picker instead".into())
        }
    };
    let resolved_cfg = ssh_config::resolve(&host);
    let key = RemoteKey::new(resolved_cfg.host_name.clone(), user, port);
    let conn = state
        .remote_connections
        .get(&key)
        .ok_or_else(|| "remote connection is not active".to_string())?;
    let sftp = conn.open_sftp().await.map_err(|e| e.to_string())?;
    let resolved_path = resolve_remote_path(&sftp, &path).await?;
    // Canonicalize to remove "..", trailing slashes etc. so navigation feels stable.
    let canonical = sftp
        .canonicalize(&resolved_path)
        .await
        .map_err(|e| format!("canonicalize: {e}"))?;
    let entries = sftp
        .read_dir(&canonical)
        .await
        .map_err(|e| format!("read_dir: {e}"))?;
    let skip = [
        ".git",
        "node_modules",
        ".venv",
        "venv",
        "__pycache__",
        "target",
        "dist",
        "build",
        "out",
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".turbo",
        ".cache",
        ".parcel-cache",
        "coverage",
    ];
    let mut dirs: Vec<String> = entries
        .into_iter()
        .filter_map(|e| {
            if !e.metadata().is_dir() {
                return None;
            }
            let name = e.file_name();
            if name.is_empty() || name == "." || name == ".." {
                return None;
            }
            if skip.iter().any(|s| *s == name) {
                return None;
            }
            Some(name)
        })
        .collect();
    dirs.sort();

    let parent = parent_path(&canonical);
    Ok(ssh::dir_paginate::paginate_dir_listing(
        &canonical,
        parent,
        dirs,
        cursor,
        limit.unwrap_or(0),
        None,
    ))
}

fn parent_path(p: &str) -> Option<String> {
    if p == "/" || p.is_empty() {
        return None;
    }
    let trimmed = p.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => None,
    }
}

#[allow(unused_imports)]
use ssh as _ssh_module;

