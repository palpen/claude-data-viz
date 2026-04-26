mod cap;
mod commands;
mod fs_watcher;
mod persistence;
mod ssh;
mod state;
mod transcript;
mod types;
mod watcher;

use ssh::registry::RemoteKey;
use ssh::{config as ssh_config, watcher as ssh_watcher};
use state::{AppState, WatchHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use types::WatchSource;
// re-exported so the setup callback can call into transcript::start_global_tail.
#[allow(unused_imports)]
use transcript as _transcript_module;

const HISTORY_FLUSH_INTERVAL_MS: u64 = 500;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Idempotent + non-fatal: a Result-returning init means double-calls (e.g. from a future
    // integration test that exercises run()) won't panic, and a subscriber failure won't kill
    // the app. RUST_LOG overrides the default. Default keeps our crate at info, everything else
    // at warn — quiet by default, but our own warnings are visible.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| "claude_data_viz=info,warn".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();
    tracing::info!("claude-data-viz starting");

    let app_state = AppState::new();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(app_state)
        .setup(|app| {
            let handle = app.handle().clone();
            let state = handle.state::<std::sync::Arc<AppState>>().inner().clone();

            // One global tail across every active Claude session under ~/.claude/projects/*
            transcript::start_global_tail(handle.clone(), state.clone());

            let prefs = persistence::load_prefs(&handle);
            *state.follow_latest.lock() = prefs.follow_latest;
            *state.selected_id.lock() = prefs.selected.clone();
            *state.recent_remotes.lock() = prefs.recent_remotes.clone();
            for w in prefs.watches.iter() {
                state.watches.lock().push(w.clone());
            }

            // Hydrate items from viz-history.json. Skip any whose backing file no longer exists
            // (local only — SSH abs_paths are remote). Drop items orphaned from removed watches:
            // a stale viz-history.json from an earlier session can reference watch_ids no longer
            // in prefs. Items keep their persisted prompts / tool_use_ids so reopening the app
            // restores attribution.
            let history = persistence::load_history(&handle);
            let watch_local_map: std::collections::HashMap<String, bool> = state
                .watches
                .lock()
                .iter()
                .map(|w| (w.id.clone(), matches!(w.source, WatchSource::Local { .. })))
                .collect();
            let dropped = hydrate_items(&state, &history.items, &watch_local_map);
            // Only rewrite history if we actually filtered something out — otherwise we'd churn
            // the file on every clean launch.
            if dropped > 0 {
                state::mark_history_dirty(&state);
            }

            for w in prefs.watches.into_iter() {
                match &w.source {
                    WatchSource::Local { path } => {
                        let root = PathBuf::from(path);
                        if !root.is_dir() {
                            continue;
                        }
                        if let Err(e) = fs_watcher::start_watch(
                            handle.clone(),
                            state.clone(),
                            w.id.clone(),
                            root,
                        ) {
                            eprintln!("warn: restart local watch {} failed: {}", w.id, e);
                        }
                    }
                    WatchSource::Ssh {
                        host,
                        user,
                        port,
                        remote_path,
                        glob,
                    } => {
                        // Best-effort SSH reconnect: failures emit a status event but do not
                        // block app boot.
                        let handle_c = handle.clone();
                        let state_c = state.clone();
                        let host = host.clone();
                        let user = user.clone();
                        let port = *port;
                        let remote_path = remote_path.clone();
                        let glob = glob.clone();
                        let watch_id = w.id.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = restart_ssh_watch(
                                handle_c,
                                state_c,
                                watch_id.clone(),
                                host,
                                user,
                                port,
                                remote_path,
                                glob,
                            )
                            .await
                            {
                                eprintln!("warn: restart ssh watch {} failed: {}", watch_id, e);
                            }
                        });
                    }
                }
            }

            // Background flusher: at most one disk write per HISTORY_FLUSH_INTERVAL_MS, regardless
            // of how many viz events fired. Cheap idle cost; no spinning.
            spawn_history_flusher(handle.clone(), state.clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::add_local_watch,
            commands::rescan_watch,
            commands::remove_watch,
            commands::set_follow_latest,
            commands::set_selected,
            commands::clear_gallery,
            commands::probe_ssh_agent,
            commands::list_ssh_hosts,
            commands::test_ssh_connection,
            commands::confirm_unknown_host,
            commands::add_remote_watch,
            commands::fetch_remote_file,
            commands::get_watch_status,
            commands::reconnect_watch,
            commands::list_recent_remotes,
            commands::forget_recent_remote,
            commands::update_remote_watch_path,
            commands::list_remote_dirs,
        ]);

    if let Err(e) = builder.run(tauri::generate_context!()) {
        eprintln!("fatal: tauri runtime exited with error: {e:#}");
        std::process::exit(1);
    }
}

/// Returns the number of persisted items that were dropped (orphaned from a removed watch, or
/// a local file that no longer exists on disk). Caller uses this to decide whether to rewrite
/// history.
fn hydrate_items(
    state: &Arc<AppState>,
    persisted: &[types::VizItem],
    watch_local_map: &std::collections::HashMap<String, bool>,
) -> usize {
    let mut items = state.items.lock();
    let mut dropped = 0usize;
    for it in persisted {
        let Some(&is_local) = watch_local_map.get(&it.watch_id) else {
            dropped += 1;
            continue;
        };
        if is_local && !Path::new(&it.abs_path).exists() {
            dropped += 1;
            continue;
        }
        let key = (it.watch_id.clone(), it.abs_path.clone());
        items.insert(key, it.clone());
    }
    dropped
}

async fn restart_ssh_watch(
    app: tauri::AppHandle,
    state: Arc<AppState>,
    watch_id: String,
    host: String,
    user: String,
    port: u16,
    remote_path: String,
    glob: String,
) -> anyhow::Result<()> {
    let mut resolved = ssh_config::resolve(&host);
    resolved.port = port;
    let key = RemoteKey::new(resolved.host_name.clone(), user.clone(), port);
    let conn = state
        .remote_connections
        .acquire(&key, resolved, &app, &state)
        .await?;
    let watcher = ssh_watcher::start(
        app,
        state.clone(),
        conn,
        key.clone(),
        watch_id.clone(),
        remote_path,
        glob,
    )
    .map_err(|e| {
        state.remote_connections.release(&key);
        e
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

fn spawn_history_flusher(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(HISTORY_FLUSH_INTERVAL_MS)).await;
            if state.history_dirty.swap(false, Ordering::Relaxed) {
                persistence::save_history(&app, &state);
            }
        }
    });
}
