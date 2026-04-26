use crate::ssh::cache::FetchLocks;
use crate::ssh::RemoteConnections;
use crate::transcript::{self, SharedIndex};
use crate::types::{RecentRemote, VizItem, Watch};
use crate::watcher::Watcher;
use parking_lot::Mutex;
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
    /// Single transcript index fed by every active local Claude session under
    /// ~/.claude/projects/. Remote watches share an index per (host, user, port)
    /// owned by the ssh::registry::RemoteConnections registry instead.
    pub global_index: SharedIndex,
    /// Refcounted `(host, user, port) → RemoteConnection` registry. Multiple SSH watches on
    /// the same devbox share one session, one transcript poller, and one transcript index.
    pub remote_connections: RemoteConnections,
    /// Per-key in-flight dedup for SFTP downloads. Shared across all SSH watches: two
    /// concurrent fetches of the same file (e.g., user clicks twice) collapse onto one.
    pub fetch_locks: FetchLocks,
    /// Set whenever the items map mutates; consumed by a periodic background task that flushes
    /// viz-history.json. Cheap atomic so callers don't pay disk I/O on the hot path.
    pub history_dirty: AtomicBool,
    /// Most-recent-first list of past remote connections, persisted via prefs.json. Used to
    /// pre-fill the connect dialog so users don't keep retyping the same host/path.
    pub recent_remotes: Mutex<Vec<RecentRemote>>,
}

pub struct WatchHandle {
    /// Kept for diagnostics — useful when logging which handle a polymorphic watcher belongs
    /// to. Read via debugger / log lines.
    #[allow(dead_code)]
    pub id: String,
    pub watcher: Box<dyn Watcher>,
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
            remote_connections: RemoteConnections::new(),
            fetch_locks: FetchLocks::default(),
            history_dirty: AtomicBool::new(false),
            recent_remotes: Mutex::new(Vec::new()),
        })
    }
}

pub fn mark_history_dirty(state: &Arc<AppState>) {
    state
        .history_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
}
