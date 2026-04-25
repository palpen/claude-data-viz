use crate::transcript::{self, SharedIndex};
use crate::types::{VizItem, Watch};
use parking_lot::Mutex;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub type ItemKey = (String, String);

pub struct AppState {
    pub items: Mutex<HashMap<ItemKey, VizItem>>,
    pub watches: Mutex<Vec<Watch>>,
    pub watch_handles: Mutex<HashMap<String, WatchHandle>>,
    pub follow_latest: Mutex<bool>,
    pub selected_id: Mutex<Option<ItemKey>>,
    /// Single transcript index fed by every active Claude session under ~/.claude/projects/.
    pub global_index: SharedIndex,
    /// Set whenever the items map mutates; consumed by a periodic background task that flushes
    /// viz-history.json. Cheap atomic so callers don't pay disk I/O on the hot path.
    pub history_dirty: AtomicBool,
}

#[allow(dead_code)]
pub struct WatchHandle {
    pub id: String,
    pub _keepalive: Box<dyn Any + Send + Sync>,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            items: Mutex::new(HashMap::new()),
            watches: Mutex::new(Vec::new()),
            watch_handles: Mutex::new(HashMap::new()),
            follow_latest: Mutex::new(true),
            selected_id: Mutex::new(None),
            global_index: transcript::new_index(),
            history_dirty: AtomicBool::new(false),
        })
    }
}

pub fn mark_history_dirty(state: &Arc<AppState>) {
    state
        .history_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
}
