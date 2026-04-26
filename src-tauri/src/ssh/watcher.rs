//! Remote file watcher: polls a directory tree over SFTP and emits viz:* events.
//!
//! Two-poll stable-size rule (a remote-friendly equivalent of the local 200ms recheck):
//! a file must show the same `(mtime, size)` across two consecutive scans before we emit
//! viz:new — half-written PNGs from `savefig()` look identical to a slow upload otherwise.
//!
//! Recursion is depth-capped (4) and skips well-known junk dirs (`node_modules`, `.git`,
//! `target`, `.venv`, etc.). The reviewer was right that an unfiltered SFTP walk of `~/`
//! over a node_modules tree is thousands of round-trips per poll.

use crate::cap;
use crate::ssh::cache;
use crate::ssh::connection::{now_ms, RemoteConnection};
use crate::ssh::registry::RemoteKey;
use crate::state::{mark_history_dirty, AppState, WatchHandle};
use crate::transcript;
use crate::types::{VizGone, VizItem, VizKind, VizStatus, VizUpdated, WatchStatus, WatchStatusEvent};
use crate::watcher::Watcher;
use anyhow::{Context, Result};
use globset::{Glob, GlobMatcher};
use russh_sftp::client::SftpSession;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::JoinHandle;

const POLL_ACTIVE_MS: u64 = 2000;
const POLL_IDLE_MS: u64 = 10_000;
const RECURSION_DEPTH: usize = 4;
const SKIP_DIRS: &[&str] = &[
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

pub struct SshWatcher {
    pub watch_id: String,
    pub key: RemoteKey,
    /// Kept solely to keep this Arc alive for the lifetime of the watcher. The polling task
    /// holds its own clone; this field is what the registry's refcount semantics implicitly
    /// rely on — when the watcher's Drop runs and calls `release(key)`, the registry drops
    /// its entry; the connection only fully tears down once this and the task's clone go too.
    #[allow(dead_code)]
    pub conn: Arc<RemoteConnection>,
    pub state: Arc<AppState>,
    pub status: Arc<parking_lot::Mutex<WatchStatus>>,
    cancel: Arc<AtomicBool>,
    task: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

impl Watcher for SshWatcher {
    fn status(&self) -> WatchStatus {
        self.status.lock().clone()
    }
}

impl Drop for SshWatcher {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.task.lock().take() {
            h.abort();
        }
        self.state.remote_connections.release(&self.key);
    }
}

#[derive(Clone)]
struct FileTrack {
    mtime_ms: i64,
    size: u64,
    stable_count: u32,
    emitted: bool,
}

/// Spawn the file poller and return the SshWatcher. Caller is responsible for inserting it
/// into `state.watch_handles` under `watch_id`.
pub fn start(
    app: AppHandle,
    state: Arc<AppState>,
    conn: Arc<RemoteConnection>,
    key: RemoteKey,
    watch_id: String,
    remote_path: String,
    glob_pattern: String,
) -> Result<SshWatcher> {
    let matcher = compile_glob(&glob_pattern)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(parking_lot::Mutex::new(WatchStatus::Connected));

    // Extend the asset protocol scope once, at watch start, to cover this watch's cache dir.
    if let Ok(cache_dir) = cache::cache_root(&app, &watch_id) {
        if let Err(e) = app.asset_protocol_scope().allow_directory(&cache_dir, true) {
            eprintln!("warn: extending asset scope for {} failed: {e}", cache_dir.display());
        }
    }

    let task = {
        let app = app.clone();
        let state = state.clone();
        let conn = conn.clone();
        let cancel = cancel.clone();
        let status = status.clone();
        let watch_id = watch_id.clone();
        tokio::spawn(async move {
            run_loop(
                app,
                state,
                conn,
                cancel,
                status,
                watch_id,
                remote_path,
                matcher,
            )
            .await;
        })
    };

    Ok(SshWatcher {
        watch_id,
        key,
        conn,
        state,
        status,
        cancel,
        task: parking_lot::Mutex::new(Some(task)),
    })
}

async fn run_loop(
    app: AppHandle,
    state: Arc<AppState>,
    conn: Arc<RemoteConnection>,
    cancel: Arc<AtomicBool>,
    status: Arc<parking_lot::Mutex<WatchStatus>>,
    watch_id: String,
    remote_path: String,
    matcher: GlobMatcher,
) {
    let mut tracked: HashMap<String, FileTrack> = HashMap::new();
    let mut idle_streak: u32 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }

        let interval_ms = if idle_streak >= 5 {
            POLL_IDLE_MS
        } else {
            POLL_ACTIVE_MS
        };
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;

        let sftp = match conn.open_sftp().await {
            Ok(s) => s,
            Err(e) => {
                set_status(
                    &app,
                    &status,
                    &watch_id,
                    WatchStatus::Reconnecting {
                        since_ms: now_ms(),
                        last_error: Some(e.to_string()),
                    },
                );
                continue;
            }
        };

        let scan = match scan_tree(&sftp, &remote_path, &matcher).await {
            Ok(s) => s,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("No such file") || msg.contains("not found") {
                    set_status(
                        &app,
                        &status,
                        &watch_id,
                        WatchStatus::PathInvalid { last_error: msg },
                    );
                    return; // Stop polling — config is bad. User must edit watch.
                }
                set_status(
                    &app,
                    &status,
                    &watch_id,
                    WatchStatus::Reconnecting {
                        since_ms: now_ms(),
                        last_error: Some(msg),
                    },
                );
                continue;
            }
        };

        // Successful scan ⇒ Connected.
        set_status(&app, &status, &watch_id, WatchStatus::Connected);

        let any_change = reconcile(
            &app,
            &state,
            &conn,
            &watch_id,
            &mut tracked,
            scan,
        )
        .await;

        if any_change {
            idle_streak = 0;
        } else {
            idle_streak = idle_streak.saturating_add(1);
        }
    }
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher> {
    let g = Glob::new(pattern).context("invalid glob pattern")?;
    Ok(g.compile_matcher())
}

struct ScanEntry {
    abs_path: String,
    rel_path: String,
    mtime_ms: i64,
    size: u64,
}

async fn scan_tree(
    sftp: &SftpSession,
    root: &str,
    matcher: &GlobMatcher,
) -> Result<Vec<ScanEntry>> {
    let mut out: Vec<ScanEntry> = Vec::new();
    let mut stack: Vec<(String, usize)> = vec![(root.trim_end_matches('/').to_string(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = match sftp.read_dir(&dir).await {
            Ok(e) => e,
            Err(e) => {
                if dir == root {
                    return Err(e.into()); // bubble up so PathInvalid can be detected
                }
                continue;
            }
        };
        for entry in entries {
            let name = entry.file_name();
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let abs = if dir.ends_with('/') {
                format!("{}{}", dir, name)
            } else {
                format!("{}/{}", dir, name)
            };
            let rel = abs
                .strip_prefix(root)
                .map(|s| s.trim_start_matches('/').to_string())
                .unwrap_or_else(|| name.clone());
            let metadata = entry.metadata();
            if metadata.is_dir() {
                if depth + 1 > RECURSION_DEPTH {
                    continue;
                }
                if SKIP_DIRS.iter().any(|s| *s == name) {
                    continue;
                }
                stack.push((abs, depth + 1));
                continue;
            }
            if !metadata.is_regular() {
                continue;
            }
            if VizKind::from_path(Path::new(&abs)).is_none() {
                continue;
            }
            if !matcher.is_match(&rel) {
                continue;
            }
            let size = metadata.size.unwrap_or(0);
            let mtime_ms = metadata
                .mtime
                .map(|t| (t as i64).saturating_mul(1000))
                .unwrap_or(0);
            out.push(ScanEntry {
                abs_path: abs,
                rel_path: rel,
                mtime_ms,
                size,
            });
        }
    }
    Ok(out)
}

async fn reconcile(
    app: &AppHandle,
    state: &Arc<AppState>,
    conn: &Arc<RemoteConnection>,
    watch_id: &str,
    tracked: &mut HashMap<String, FileTrack>,
    scan: Vec<ScanEntry>,
) -> bool {
    let mut any_change = false;
    let seen: HashSet<String> = scan.iter().map(|e| e.abs_path.clone()).collect();

    for entry in scan {
        let prev = tracked.get(&entry.abs_path).cloned();
        match prev {
            None => {
                tracked.insert(
                    entry.abs_path.clone(),
                    FileTrack {
                        mtime_ms: entry.mtime_ms,
                        size: entry.size,
                        stable_count: 1,
                        emitted: false,
                    },
                );
            }
            Some(track) => {
                if track.mtime_ms == entry.mtime_ms && track.size == entry.size {
                    let stable = track.stable_count.saturating_add(1);
                    let should_emit = !track.emitted && stable >= 2;
                    tracked.insert(
                        entry.abs_path.clone(),
                        FileTrack {
                            mtime_ms: entry.mtime_ms,
                            size: entry.size,
                            stable_count: stable,
                            emitted: track.emitted || should_emit,
                        },
                    );
                    if should_emit {
                        emit_new(app, state, conn, watch_id, &entry).await;
                        any_change = true;
                    }
                } else {
                    // Changed size/mtime — reset stability, emit update if previously emitted.
                    let now_track = FileTrack {
                        mtime_ms: entry.mtime_ms,
                        size: entry.size,
                        stable_count: 1,
                        emitted: track.emitted,
                    };
                    tracked.insert(entry.abs_path.clone(), now_track);
                    if track.emitted {
                        let _ = app.emit(
                            "viz:updated",
                            VizUpdated {
                                watch_id: watch_id.to_string(),
                                abs_path: entry.abs_path.clone(),
                                mtime: entry.mtime_ms,
                                size: entry.size,
                            },
                        );
                        if let Some(item) = state
                            .items
                            .lock()
                            .get_mut(&(watch_id.to_string(), entry.abs_path.clone()))
                        {
                            item.mtime = entry.mtime_ms;
                            item.size = entry.size;
                        }
                        mark_history_dirty(state);
                        any_change = true;
                    }
                }
            }
        }
    }

    // Detect removals — anything tracked-and-emitted that's no longer in scan.
    let removed: Vec<String> = tracked
        .iter()
        .filter(|(p, t)| t.emitted && !seen.contains(*p))
        .map(|(p, _)| p.clone())
        .collect();
    for path in removed {
        tracked.remove(&path);
        let key = (watch_id.to_string(), path.clone());
        let removed_ok = state.items.lock().remove(&key).is_some();
        if removed_ok {
            mark_history_dirty(state);
            let _ = app.emit(
                "viz:gone",
                VizGone {
                    watch_id: watch_id.to_string(),
                    abs_path: path,
                },
            );
            any_change = true;
        }
    }
    // Drop tracking for not-yet-emitted files that vanished too (rapid deletes).
    tracked.retain(|p, t| t.emitted || seen.contains(p));

    any_change
}

async fn emit_new(
    app: &AppHandle,
    state: &Arc<AppState>,
    conn: &Arc<RemoteConnection>,
    watch_id: &str,
    entry: &ScanEntry,
) {
    let kind = match VizKind::from_path(Path::new(&entry.abs_path)) {
        Some(k) => k,
        None => return,
    };
    let item = VizItem {
        watch_id: watch_id.to_string(),
        abs_path: entry.abs_path.clone(),
        rel_path: entry.rel_path.clone(),
        kind,
        size: entry.size,
        mtime: entry.mtime_ms,
        prompt: None,
        tool_use_id: None,
        session_id: None,
        cwd: None,
        status: VizStatus::Active,
    };
    let key = (watch_id.to_string(), item.abs_path.clone());
    let merged = {
        let items = state.items.lock();
        if let Some(existing) = items.get(&key) {
            VizItem {
                prompt: existing.prompt.clone(),
                tool_use_id: existing.tool_use_id.clone(),
                session_id: existing.session_id.clone(),
                cwd: existing.cwd.clone(),
                ..item.clone()
            }
        } else {
            item.clone()
        }
    };
    state.items.lock().insert(key, merged.clone());
    mark_history_dirty(state);
    let _ = app.emit("viz:new", &merged);

    transcript::try_enrich_now(
        app,
        state,
        watch_id,
        &conn.transcript_index,
        &merged.abs_path,
        merged.mtime,
    );
    cap::enforce_item_cap(app, state);
}

fn set_status(
    app: &AppHandle,
    status: &Arc<parking_lot::Mutex<WatchStatus>>,
    watch_id: &str,
    next: WatchStatus,
) {
    let mut cur = status.lock();
    if statuses_equal(&cur, &next) {
        return;
    }
    *cur = next.clone();
    drop(cur);
    let _ = app.emit(
        "viz:watch_status",
        WatchStatusEvent {
            watch_id: watch_id.to_string(),
            status: next,
        },
    );
}

fn statuses_equal(a: &WatchStatus, b: &WatchStatus) -> bool {
    use WatchStatus::*;
    matches!(
        (a, b),
        (Connected, Connected)
            | (Stopped, Stopped)
            | (Reconnecting { .. }, Reconnecting { .. })
            | (AuthFailed { .. }, AuthFailed { .. })
            | (Unreachable { .. }, Unreachable { .. })
            | (PathInvalid { .. }, PathInvalid { .. })
    )
}

/// Convenience for tests / external callers — registers an SshWatcher into watch_handles.
#[allow(dead_code)]
pub fn install(state: &Arc<AppState>, watcher: SshWatcher) {
    state.watch_handles.lock().insert(
        watcher.watch_id.clone(),
        WatchHandle {
            id: watcher.watch_id.clone(),
            watcher: Box::new(watcher),
        },
    );
}

#[allow(dead_code)]
fn _force_use(_: &PathBuf) {}
