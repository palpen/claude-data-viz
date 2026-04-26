//! Refcounted registry of `(host, user, port) → RemoteConnection`. Multiple watches on the
//! same devbox share one SSH session, one transcript poller, and one transcript index. When
//! the last watch on a host stops, the connection drops and its tasks abort.

use crate::ssh::config::ResolvedHost;
use crate::ssh::connection::{self, RemoteConnection};
use crate::ssh::transcript;
use crate::state::AppState;
use anyhow::Result;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::AppHandle;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteKey {
    pub host: String,
    pub user: String,
    pub port: u16,
}

impl RemoteKey {
    pub fn new(host: impl Into<String>, user: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            user: user.into(),
            port,
        }
    }
}

struct Entry {
    conn: Arc<RemoteConnection>,
    refcount: u32,
}

#[derive(Default)]
pub struct RemoteConnections {
    map: Mutex<HashMap<RemoteKey, Entry>>,
}

impl RemoteConnections {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire a (possibly shared) connection for `key`. If one already exists, bump refcount
    /// and return it. Otherwise establish a new connection, kick off the transcript poller,
    /// and insert with refcount = 1.
    pub async fn acquire(
        &self,
        key: &RemoteKey,
        resolved: ResolvedHost,
        app: &AppHandle,
        state: &Arc<AppState>,
    ) -> Result<Arc<RemoteConnection>> {
        // Fast path: existing entry.
        {
            let mut map = self.map.lock();
            if let Some(entry) = map.get_mut(key) {
                entry.refcount += 1;
                return Ok(entry.conn.clone());
            }
        }

        // Slow path: connect outside the lock.
        let conn = Arc::new(connection::connect(resolved, Some(key.user.clone())).await?);
        let task = transcript::start_poller(conn.clone(), app.clone(), state.clone());
        conn.store_transcript_task(task);

        // Race window: another task may have inserted while we were connecting.
        let mut map = self.map.lock();
        if let Some(entry) = map.get_mut(key) {
            entry.refcount += 1;
            // The conn we just built will drop here, aborting its transcript task. Wasted
            // work but correct.
            return Ok(entry.conn.clone());
        }
        map.insert(
            key.clone(),
            Entry {
                conn: conn.clone(),
                refcount: 1,
            },
        );
        Ok(conn)
    }

    /// Decrement refcount. When it reaches zero, drop the entry — which causes the
    /// `RemoteConnection`'s Drop impl to abort its transcript task and close the SSH session.
    pub fn release(&self, key: &RemoteKey) {
        let mut map = self.map.lock();
        if let Some(entry) = map.get_mut(key) {
            entry.refcount = entry.refcount.saturating_sub(1);
            if entry.refcount == 0 {
                map.remove(key);
            }
        }
    }

    /// Look up a connection without changing refcount. Used for status queries / fetch_remote.
    pub fn get(&self, key: &RemoteKey) -> Option<Arc<RemoteConnection>> {
        self.map.lock().get(key).map(|e| e.conn.clone())
    }
}
