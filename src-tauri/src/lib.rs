mod cap;
mod commands;
mod fs_watcher;
mod persistence;
mod state;
mod transcript;
mod types;

use state::AppState;
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
    let app_state = AppState::new();

    tauri::Builder::default()
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
            for w in prefs.watches.iter() {
                state.watches.lock().push(w.clone());
            }

            // Hydrate items from viz-history.json. Skip any whose backing file no longer exists,
            // so deleted-while-app-was-closed images don't ghost-haunt the sidebar. Items keep
            // their persisted prompts / tool_use_ids so reopening the app restores attribution.
            let history = persistence::load_history(&handle);
            hydrate_items(&state, &history.items);

            for w in prefs.watches.into_iter() {
                if let WatchSource::Local { path } = &w.source {
                    let root = PathBuf::from(path);
                    if !root.is_dir() {
                        continue;
                    }
                    if let Err(e) =
                        fs_watcher::start_watch(handle.clone(), state.clone(), w.id.clone(), root)
                    {
                        eprintln!("warn: restart watch {} failed: {}", w.id, e);
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
            commands::remove_watch,
            commands::set_follow_latest,
            commands::set_selected,
            commands::clear_gallery,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn hydrate_items(state: &Arc<AppState>, persisted: &[types::VizItem]) {
    let mut items = state.items.lock();
    for it in persisted {
        if !Path::new(&it.abs_path).exists() {
            continue;
        }
        let key = (it.watch_id.clone(), it.abs_path.clone());
        items.insert(key, it.clone());
    }
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
