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
#[allow(dead_code)]
const BACKOFF_INITIAL_MS: u64 = 2_000;
#[allow(dead_code)]
const BACKOFF_MAX_MS: u64 = 300_000; // 5 min
#[allow(dead_code)]
const BACKOFF_JITTER: f64 = 0.2; // ±20%
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

/// Exponential-backoff-with-jitter state machine.
///
/// Pure (no I/O, no tokio dependency). Doubles `current_ms` after each `next_delay()` call,
/// caps at `max_ms`, applies symmetric ±`jitter_frac` jitter, and resets to `min_ms` on
/// `reset()`. Uses an inline LCG so behavior is deterministic given a seed — no `rand` crate
/// pulled in for tests, no flakes.
#[derive(Debug)]
pub(crate) struct BackoffState {
    current_ms: u64,
    min_ms: u64,
    max_ms: u64,
    jitter_frac: f64,
    rng_state: u64,
}

impl BackoffState {
    pub fn new(min_ms: u64, max_ms: u64, jitter_frac: f64, seed: u64) -> Self {
        Self {
            current_ms: min_ms,
            min_ms,
            max_ms,
            jitter_frac,
            rng_state: seed.max(1),
        }
    }

    pub fn reset(&mut self) {
        self.current_ms = self.min_ms;
    }

    pub fn next_delay(&mut self) -> Duration {
        let base = self.current_ms;
        // LCG (Knuth/MMIX constants) — fine for jitter, not for crypto.
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (self.rng_state >> 32) as u32 as f64 / u32::MAX as f64; // 0..1
        let frac = (r * 2.0 - 1.0) * self.jitter_frac; // -jitter..+jitter
        let jittered = ((base as f64) * (1.0 + frac)).max(0.0) as u64;
        self.current_ms = self.current_ms.saturating_mul(2).min(self.max_ms);
        Duration::from_millis(jittered)
    }
}

/// Pure helper that mirrors `set_status_and_maybe_emit`'s decision logic without side
/// effects, so it can be unit-tested independently of Tauri's `AppHandle::emit`.
pub(crate) struct StatusTransition;

impl StatusTransition {
    /// Returns the status that should be emitted, or `None` if the new status is equal to
    /// the previous one. Equality is full structural `PartialEq` — payload differences in
    /// `Reconnecting` (e.g. updated `last_error`) are treated as a transition.
    pub fn diff(prev: &WatchStatus, next: &WatchStatus) -> Option<WatchStatus> {
        if prev == next {
            None
        } else {
            Some(next.clone())
        }
    }
}

/// Derive a deterministic LCG seed from a watch_id so two watchers picking concurrent
/// jitter samples don't synchronize their reconnect storms.
#[allow(dead_code)]
pub(crate) fn seed_from_watch_id(watch_id: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    watch_id.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WatchStatus;

    #[test]
    fn backoff_initial_delay_is_min() {
        let mut b = BackoffState::new(2000, 300_000, 0.0, 1);
        assert_eq!(b.next_delay(), Duration::from_millis(2000));
    }

    #[test]
    fn backoff_doubles_on_consecutive_errors() {
        let mut b = BackoffState::new(2000, 300_000, 0.0, 1);
        assert_eq!(b.next_delay(), Duration::from_millis(2000));
        assert_eq!(b.next_delay(), Duration::from_millis(4000));
        assert_eq!(b.next_delay(), Duration::from_millis(8000));
        assert_eq!(b.next_delay(), Duration::from_millis(16000));
    }

    #[test]
    fn backoff_caps_at_max() {
        let mut b = BackoffState::new(2000, 10_000, 0.0, 1);
        assert_eq!(b.next_delay(), Duration::from_millis(2000));
        assert_eq!(b.next_delay(), Duration::from_millis(4000));
        assert_eq!(b.next_delay(), Duration::from_millis(8000));
        // Capped from here on.
        for _ in 0..10 {
            assert_eq!(b.next_delay(), Duration::from_millis(10_000));
        }
    }

    #[test]
    fn backoff_resets_on_success() {
        let mut b = BackoffState::new(2000, 300_000, 0.0, 1);
        b.next_delay(); // 2000
        b.next_delay(); // 4000
        b.next_delay(); // 8000
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_millis(2000));
    }

    #[test]
    fn backoff_jitter_within_bounds() {
        // jitter=0.2, base=4000 ⇒ delays must fall in [3200, 4800].
        // Force base by stepping past initial: 2000 → step → 4000.
        let lo = 3200u64;
        let hi = 4800u64;
        for sample_seed in 1..=50u64 {
            let mut b = BackoffState::new(2000, 300_000, 0.2, sample_seed);
            // Skip the 2000-base sample.
            let _ = b.next_delay();
            let d = b.next_delay().as_millis() as u64;
            assert!(
                d >= lo && d <= hi,
                "seed {sample_seed}: delay {d} not in [{lo}, {hi}]"
            );
        }
    }

    #[test]
    fn backoff_jitter_deterministic_with_same_seed() {
        let mut a = BackoffState::new(2000, 300_000, 0.2, 7);
        let mut b = BackoffState::new(2000, 300_000, 0.2, 7);
        assert_eq!(a.next_delay(), b.next_delay());
        assert_eq!(a.next_delay(), b.next_delay());
        assert_eq!(a.next_delay(), b.next_delay());
    }

    #[test]
    fn transition_emits_on_change() {
        let prev = WatchStatus::Reconnecting {
            since_ms: 100,
            last_error: Some("boom".into()),
        };
        let next = WatchStatus::Connected;
        assert_eq!(
            StatusTransition::diff(&prev, &next),
            Some(WatchStatus::Connected)
        );
    }

    #[test]
    fn transition_no_emit_on_same_status() {
        // Two identical Connected statuses → no emit.
        assert_eq!(
            StatusTransition::diff(&WatchStatus::Connected, &WatchStatus::Connected),
            None
        );
        // Two identical Reconnecting payloads → no emit.
        let a = WatchStatus::Reconnecting {
            since_ms: 100,
            last_error: Some("x".into()),
        };
        let b = WatchStatus::Reconnecting {
            since_ms: 100,
            last_error: Some("x".into()),
        };
        assert_eq!(StatusTransition::diff(&a, &b), None);
    }

    #[test]
    fn transition_emits_with_correct_payload() {
        // Reconnecting → Connected emits Connected.
        let prev = WatchStatus::Reconnecting {
            since_ms: 100,
            last_error: Some("a".into()),
        };
        let next = WatchStatus::Connected;
        assert_eq!(
            StatusTransition::diff(&prev, &next),
            Some(WatchStatus::Connected)
        );

        // Reconnecting payload mutation (last_error a → b) emits the new Reconnecting.
        let prev = WatchStatus::Reconnecting {
            since_ms: 100,
            last_error: Some("a".into()),
        };
        let next = WatchStatus::Reconnecting {
            since_ms: 100,
            last_error: Some("b".into()),
        };
        assert_eq!(StatusTransition::diff(&prev, &next), Some(next));
    }

    #[test]
    fn seed_from_watch_id_is_stable_and_distinct() {
        let a1 = seed_from_watch_id("watch-a");
        let a2 = seed_from_watch_id("watch-a");
        let b = seed_from_watch_id("watch-b");
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
    }
}
