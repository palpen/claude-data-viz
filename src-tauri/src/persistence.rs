use crate::state::AppState;
use crate::types::{VizItem, Watch};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedPrefs {
    #[serde(default)]
    pub watches: Vec<Watch>,
    #[serde(default = "default_follow")]
    pub follow_latest: bool,
    #[serde(default)]
    pub selected: Option<(String, String)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedHistory {
    #[serde(default)]
    pub items: Vec<VizItem>,
}

fn default_follow() -> bool {
    true
}

fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("warn: failed to create app data dir: {e}");
        return None;
    }
    Some(dir)
}

fn prefs_path(app: &AppHandle) -> Option<PathBuf> {
    data_dir(app).map(|d| d.join("prefs.json"))
}

fn history_path(app: &AppHandle) -> Option<PathBuf> {
    data_dir(app).map(|d| d.join("viz-history.json"))
}

fn legacy_state_path(app: &AppHandle) -> Option<PathBuf> {
    data_dir(app).map(|d| d.join("state.json"))
}

/// Load preferences. If the legacy `state.json` exists and `prefs.json` doesn't,
/// migrate it (read once, write to new path, remove old).
pub fn load_prefs(app: &AppHandle) -> PersistedPrefs {
    let Some(prefs) = prefs_path(app) else {
        return PersistedPrefs::default();
    };

    if !prefs.exists() {
        if let Some(legacy) = legacy_state_path(app) {
            if legacy.exists() {
                let migrated: PersistedPrefs = match std::fs::read(&legacy)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                {
                    Some(p) => p,
                    None => PersistedPrefs::default(),
                };
                write_json(&prefs, &migrated);
                let _ = std::fs::remove_file(&legacy);
                return migrated;
            }
        }
        return PersistedPrefs::default();
    }

    std::fs::read(&prefs)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn load_history(app: &AppHandle) -> PersistedHistory {
    let Some(path) = history_path(app) else {
        return PersistedHistory::default();
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_prefs(app: &AppHandle, state: &Arc<AppState>) {
    let Some(path) = prefs_path(app) else {
        return;
    };
    let snap = PersistedPrefs {
        watches: state.watches.lock().clone(),
        follow_latest: *state.follow_latest.lock(),
        selected: state.selected_id.lock().clone(),
    };
    write_json(&path, &snap);
}

pub fn save_history(app: &AppHandle, state: &Arc<AppState>) {
    let Some(path) = history_path(app) else {
        return;
    };
    let items: Vec<VizItem> = state.items.lock().values().cloned().collect();
    let snap = PersistedHistory { items };
    write_json(&path, &snap);
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    let bytes = match serde_json::to_vec_pretty(value) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("warn: serialize {} failed: {e}", path.display());
            return;
        }
    };
    // Atomic write: write to .tmp, then rename. Avoids torn reads on crash.
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        eprintln!("warn: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        eprintln!("warn: rename {} -> {} failed: {e}", tmp.display(), path.display());
    }
}
