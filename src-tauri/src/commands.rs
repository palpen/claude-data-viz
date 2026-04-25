use crate::fs_watcher;
use crate::persistence;
use crate::state::{mark_history_dirty, AppState};
use crate::types::{InitialState, VizItem, Watch, WatchSource};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
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
    fs_watcher::stop_watch(&state, &watch_id);
    state.watches.lock().retain(|w| w.id != watch_id);
    persistence::save_prefs(&app, state.inner());
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

