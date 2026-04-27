use crate::cap;
use crate::state::{mark_history_dirty, AppState, WatchHandle};
use crate::transcript;
use crate::types::{VizGone, VizItem, VizKind, VizStatus, VizUpdated, WatchStatus};
use crate::watcher::Watcher;
use anyhow::{Context, Result};
use chrono::Utc;
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, DebouncedEvent};
use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::unbounded_channel;

/// Watcher impl for a local folder. Drops the underlying notify debouncer when this struct is
/// dropped, which causes the OS file watcher to detach and the event-drain task's channel to
/// close, exiting it cleanly.
struct LocalWatcher {
    _debouncer: Box<dyn Any + Send + Sync>,
}

impl Watcher for LocalWatcher {
    fn status(&self) -> WatchStatus {
        WatchStatus::Connected
    }
}

const DEBOUNCE_MS: u64 = 250;
const STABLE_RECHECK_MS: u64 = 200;

pub fn start_watch(
    app: AppHandle,
    state: Arc<AppState>,
    watch_id: String,
    root: PathBuf,
) -> Result<()> {
    if !root.is_dir() {
        anyhow::bail!("watch path is not a directory: {}", root.display());
    }

    // Allow Tauri's asset protocol to read from this root.
    if let Err(e) = app
        .asset_protocol_scope()
        .allow_directory(&root, true)
    {
        eprintln!("warn: failed to extend asset protocol scope: {e}");
    }

    let (tx, mut rx) = unbounded_channel::<DebounceEventResult>();

    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_MS),
        None,
        move |result: DebounceEventResult| {
            let _ = tx.send(result);
        },
    )
    .context("creating debouncer")?;
    debouncer
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;

    state.watch_handles.lock().insert(
        watch_id.clone(),
        WatchHandle {
            id: watch_id.clone(),
            watcher: Box::new(LocalWatcher {
                _debouncer: Box::new(debouncer),
            }),
        },
    );

    // Cold-start scan: enumerate existing files (cap to most recent 50 by mtime, last 24h).
    {
        let app = app.clone();
        let state = state.clone();
        let wid = watch_id.clone();
        let root = root.clone();
        tauri::async_runtime::spawn(async move {
            cold_scan(&app, &state, &wid, &root).await;
        });
    }

    // Event drain task.
    {
        let app = app.clone();
        let state = state.clone();
        let wid = watch_id.clone();
        let root = root.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(result) = rx.recv().await {
                match result {
                    Ok(events) => {
                        for ev in events {
                            let app = app.clone();
                            let state = state.clone();
                            let wid = wid.clone();
                            let root = root.clone();
                            tauri::async_runtime::spawn(async move {
                                process_event(&app, &state, &wid, &root, ev).await;
                            });
                        }
                    }
                    Err(errors) => {
                        for err in errors {
                            eprintln!("notify error: {err}");
                        }
                    }
                }
            }
        });
    }

    Ok(())
}

pub fn stop_watch(state: &Arc<AppState>, watch_id: &str) {
    state.watch_handles.lock().remove(watch_id);
    let removed_any = {
        let mut items = state.items.lock();
        let before = items.len();
        items.retain(|(wid, _), _| wid != watch_id);
        items.len() != before
    };
    if removed_any {
        mark_history_dirty(state);
    }
}

pub async fn cold_scan(app: &AppHandle, state: &Arc<AppState>, watch_id: &str, root: &Path) {
    let mut found: Vec<(PathBuf, std::fs::Metadata)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                if !is_skippable_dir(&path) {
                    stack.push(path);
                }
            } else if meta.is_file() && VizKind::from_path(&path).is_some() {
                found.push((path, meta));
            }
        }
    }

    // Cold-scan: take the 50 newest matching files. Older ones still appear in the watch as
    // they're modified — we just don't backfill the entire history on first-add. No time-window
    // filter: a project folder can be weeks old and the user still wants to see its renders.
    found.sort_by_key(|(_, m)| {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });

    let take_from = found.len().saturating_sub(50);
    let mut any_new = false;
    for (path, meta) in found.into_iter().skip(take_from) {
        let Some(fresh) = build_item(watch_id, root, &path, &meta) else {
            continue;
        };
        let key = (watch_id.to_string(), fresh.abs_path.clone());
        let (merged, was_present) = upsert_preserving_enrichment(state, key, fresh);
        let is_new = !was_present;
        if is_new {
            any_new = true;
            let _ = app.emit("viz:new", &merged);
            transcript::try_enrich_now(
                app,
                state,
                watch_id,
                &state.global_index,
                &merged.abs_path,
                merged.mtime,
            );
        }
    }
    if any_new {
        mark_history_dirty(state);
    }
    cap::enforce_item_cap(app, state);
}

/// Insert (or update) an item by key. If an entry already exists, preserve its `prompt` and
/// `tool_use_id` so hydrated/enriched attribution survives cold-scan and process-event cycles.
/// Returns the final stored item and whether an entry existed at this key before the call.
fn upsert_preserving_enrichment(
    state: &Arc<AppState>,
    key: (String, String),
    fresh: VizItem,
) -> (VizItem, bool) {
    let mut items = state.items.lock();
    let was_present = items.contains_key(&key);
    let merged = if let Some(existing) = items.get(&key) {
        VizItem {
            prompt: existing.prompt.clone(),
            tool_use_id: existing.tool_use_id.clone(),
            session_id: existing.session_id.clone(),
            cwd: existing.cwd.clone(),
            ..fresh
        }
    } else {
        fresh
    };
    items.insert(key, merged.clone());
    (merged, was_present)
}

async fn process_event(
    app: &AppHandle,
    state: &Arc<AppState>,
    watch_id: &str,
    root: &Path,
    ev: DebouncedEvent,
) {
    let path = match ev.paths.first() {
        Some(p) => p.clone(),
        None => return,
    };
    let kind = match VizKind::from_path(&path) {
        Some(k) => k,
        None => {
            // Track removals even for non-viz extensions only if we previously knew about them.
            if matches!(ev.event.kind, EventKind::Remove(_)) {
                handle_remove(app, state, watch_id, &path).await;
            }
            return;
        }
    };
    let _ = kind;

    match ev.event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            // Stable-size recheck: avoid emitting half-written files.
            let size1 = match std::fs::metadata(&path) {
                Ok(m) => m.len(),
                Err(_) => return,
            };
            tokio::time::sleep(Duration::from_millis(STABLE_RECHECK_MS)).await;
            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => return,
            };
            if meta.len() != size1 {
                return;
            }
            let Some(fresh) = build_item(watch_id, root, &path, &meta) else {
                eprintln!(
                    "warn: fs_watcher: build_item rejected unexpected path {}",
                    path.display()
                );
                return;
            };
            let key = (watch_id.to_string(), fresh.abs_path.clone());
            let (merged, was_present) = upsert_preserving_enrichment(state, key, fresh);
            if was_present {
                let _ = app.emit(
                    "viz:updated",
                    VizUpdated {
                        watch_id: watch_id.to_string(),
                        abs_path: merged.abs_path.clone(),
                        mtime: merged.mtime,
                        size: merged.size,
                    },
                );
            } else {
                let _ = app.emit("viz:new", &merged);
            }
            transcript::try_enrich_now(
                app,
                state,
                watch_id,
                &state.global_index,
                &merged.abs_path,
                merged.mtime,
            );
            mark_history_dirty(state);
            cap::enforce_item_cap(app, state);
        }
        EventKind::Remove(_) => {
            handle_remove(app, state, watch_id, &path).await;
        }
        _ => {}
    }
}

async fn handle_remove(app: &AppHandle, state: &Arc<AppState>, watch_id: &str, path: &Path) {
    let key = (watch_id.to_string(), path.to_string_lossy().into_owned());
    let removed = state.items.lock().remove(&key).is_some();
    if removed {
        mark_history_dirty(state);
        let _ = app.emit(
            "viz:gone",
            VizGone {
                watch_id: watch_id.to_string(),
                abs_path: path.to_string_lossy().into_owned(),
            },
        );
    }
}

fn build_item(
    watch_id: &str,
    root: &Path,
    path: &Path,
    meta: &std::fs::Metadata,
) -> Option<VizItem> {
    let kind = VizKind::from_path(path)?;
    let abs = path.to_string_lossy().into_owned();
    let rel = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| abs.clone())
        });
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    Some(VizItem {
        watch_id: watch_id.to_string(),
        abs_path: abs,
        rel_path: rel,
        kind,
        size: meta.len(),
        mtime,
        prompt: None,
        tool_use_id: None,
        session_id: None,
        cwd: None,
        status: VizStatus::Active,
    })
}

fn is_skippable_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(
            ".git"
                | "node_modules"
                | ".venv"
                | "venv"
                | "__pycache__"
                | "target"
                | "dist"
                | "build"
                | "out"
                | ".next"
                | ".nuxt"
                | ".svelte-kit"
                | ".turbo"
                | ".cache"
                | ".parcel-cache"
                | "coverage"
                // Common scaffold/asset dirs — usually framework chrome, not user-generated viz.
                | "assets"
                | "public"
                | "icons"
                | "static"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn build_item_returns_none_for_unknown_extension() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("notes.txt");
        fs::write(&path, b"hello").expect("write txt");
        let meta = fs::metadata(&path).expect("metadata");
        assert!(build_item("watch-1", dir.path(), &path, &meta).is_none());
    }

    #[test]
    fn build_item_returns_some_for_png() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("plot.png");
        fs::write(&path, b"\x89PNG\r\n").expect("write png");
        let meta = fs::metadata(&path).expect("metadata");
        let item = build_item("watch-1", dir.path(), &path, &meta).expect("some item");
        assert_eq!(item.kind, VizKind::Png);
        assert_eq!(item.watch_id, "watch-1");
        assert_eq!(item.rel_path, "plot.png");
    }
}
