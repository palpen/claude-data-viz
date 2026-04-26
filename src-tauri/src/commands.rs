use crate::fs_watcher;
use crate::persistence;
use crate::ssh::{self, cache, config as ssh_config, connection, registry::RemoteKey, watcher as ssh_watcher};
use crate::state::{mark_history_dirty, AppState, WatchHandle};
use crate::types::{
    InitialState, SshAgentProbe, SshHostEntry, TestResult, TestStage, VizItem, Watch, WatchSource,
    WatchStatus,
};
use globset::Glob;
use russh_keys::agent::client::AgentClient;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, State};
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
    pub remote_path: String,
    pub glob: String,
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
            };
        }
        Err(_) => {
            return TestResult {
                reachable: err_stage("tcp connect timed out"),
                authenticated: skip_stage(),
                path_exists: skip_stage(),
                matched: skip_stage(),
            };
        }
    };

    // Stage 2 — Auth via ssh-agent.
    let user_resolved = match user {
        Some(u) => u,
        None => {
            return TestResult {
                reachable,
                authenticated: err_stage("no user — set User in ~/.ssh/config or pass explicitly"),
                path_exists: skip_stage(),
                matched: skip_stage(),
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
            };
        }
    };
    if let Err(e) = sftp.metadata(&args.remote_path).await {
        return TestResult {
            reachable,
            authenticated,
            path_exists: err_stage(format!("path stat: {e}")),
            matched: skip_stage(),
        };
    }
    let path_exists = ok_stage();

    // Stage 4 — Matched files. Light scan: just the immediate readdir + glob filter, no
    // recursion — same pattern won't catch subdirs but the count is illustrative.
    let matched = match Glob::new(&args.glob) {
        Err(e) => err_stage(format!("invalid glob: {e}")),
        Ok(g) => {
            let matcher = g.compile_matcher();
            match sftp.read_dir(&args.remote_path).await {
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
    }
}

#[derive(Deserialize)]
pub struct AddRemoteWatchArgs {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub remote_path: String,
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

    let watch_id = Uuid::new_v4().to_string();
    let watcher = ssh_watcher::start(
        app.clone(),
        state.inner().clone(),
        conn,
        key.clone(),
        watch_id.clone(),
        args.remote_path.clone(),
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
            user,
            port,
            remote_path: args.remote_path.clone(),
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
    persistence::save_prefs(&app, state.inner());
    Ok(watch)
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

#[allow(unused_imports)]
use ssh as _ssh_module;

