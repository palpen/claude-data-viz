use crate::state::{mark_history_dirty, AppState};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

pub const ITEM_CAP: usize = 200;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct VizEvicted {
    pub watch_id: String,
    pub abs_path: String,
}

pub fn enforce_item_cap(app: &AppHandle, state: &Arc<AppState>) {
    let evicted: Vec<(String, String)> = {
        let mut items = state.items.lock();
        if items.len() <= ITEM_CAP {
            return;
        }
        let overflow = items.len() - ITEM_CAP;
        let mut by_age: Vec<((String, String), i64)> =
            items.iter().map(|(k, v)| (k.clone(), v.mtime)).collect();
        by_age.sort_by_key(|(_, m)| *m);
        let to_drop: Vec<(String, String)> =
            by_age.into_iter().take(overflow).map(|(k, _)| k).collect();
        for k in &to_drop {
            items.remove(k);
        }
        to_drop
    };
    if !evicted.is_empty() {
        mark_history_dirty(state);
    }
    for (watch_id, abs_path) in evicted {
        let _ = app.emit(
            "viz:evicted",
            VizEvicted {
                watch_id,
                abs_path,
            },
        );
    }
}
