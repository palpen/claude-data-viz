//! Remote transcript poller. Periodically lists `~/.claude/projects/*/*.jsonl` over SFTP,
//! tails each active file by reading delta bytes, and feeds parsed lines into the per-host
//! `SharedIndex`. Mirror of `crate::transcript::start_global_tail` but over SFTP, with a
//! cancellation flag so the registry can shut us down on connection drop.

use crate::ssh::connection::RemoteConnection;
use crate::state::AppState;
use crate::transcript;
use anyhow::Result;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::AsyncReadExt;
use tokio::task::JoinHandle;

const POLL_INTERVAL_MS: u64 = 1000;
const RESCAN_INTERVAL_TICKS: u32 = 4;
const STALE_TAIL_MS: i64 = 60 * 60 * 1000;
const BACKFILL_BYTES: u64 = 64 * 1024;
const READ_CHUNK_BYTES: usize = 32 * 1024;

struct TailState {
    offset: u64,
    buffer: String,
}

pub fn start_poller(
    conn: Arc<RemoteConnection>,
    app: AppHandle,
    state: Arc<AppState>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run(conn, app, state).await {
            eprintln!("ssh transcript poller exited: {e}");
        }
    })
}

async fn run(conn: Arc<RemoteConnection>, app: AppHandle, state: Arc<AppState>) -> Result<()> {
    let cancel = conn.cancel_flag();
    let mut tails: HashMap<PathBuf, TailState> = HashMap::new();
    let mut tick: u32 = 0;
    let mut active: Vec<PathBuf> = Vec::new();
    let mut projects_dir: Option<PathBuf> = None;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
        tick = tick.wrapping_add(1);

        let sftp = match conn.open_sftp().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ssh transcript: open_sftp failed: {e}");
                continue;
            }
        };

        if projects_dir.is_none() {
            projects_dir = resolve_projects_dir(&sftp).await;
        }
        let Some(projects) = projects_dir.clone() else {
            continue;
        };

        if tick % RESCAN_INTERVAL_TICKS == 1 {
            active = scan_active(&sftp, &projects).await.unwrap_or_default();
            let live: std::collections::HashSet<&PathBuf> = active.iter().collect();
            tails.retain(|p, _| live.contains(p));
        }

        let mut dirty = false;
        for path in &active {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let path_str = path.to_string_lossy();
            let Ok(meta) = sftp.metadata(path_str.as_ref()).await else {
                continue;
            };
            let len = meta.size.unwrap_or(0);
            let tail = tails.entry(path.clone()).or_insert_with(|| TailState {
                offset: len.saturating_sub(BACKFILL_BYTES),
                buffer: String::new(),
            });
            if len == tail.offset {
                continue;
            }
            if len < tail.offset {
                tail.offset = len;
                continue;
            }
            match read_delta(&sftp, &path_str, tail.offset, len).await {
                Ok(chunk) => {
                    tail.offset = len;
                    tail.buffer.push_str(&chunk);
                    while let Some(idx) = tail.buffer.find('\n') {
                        let line = tail.buffer[..idx].to_string();
                        tail.buffer.drain(..=idx);
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        transcript::process_line(&conn.transcript_index, trimmed);
                        dirty = true;
                    }
                }
                Err(e) => {
                    eprintln!("ssh transcript: read_delta {} failed: {e}", path.display());
                }
            }
        }
        if dirty {
            transcript::enrich_pending_all(&app, &state, &conn.transcript_index);
        }
    }
}

async fn resolve_projects_dir(sftp: &SftpSession) -> Option<PathBuf> {
    let home = sftp.canonicalize(".").await.ok()?;
    let mut p = PathBuf::from(home);
    p.push(".claude");
    p.push("projects");
    Some(p)
}

async fn scan_active(sftp: &SftpSession, projects: &PathBuf) -> Result<Vec<PathBuf>> {
    use chrono::Utc;
    let now_ms = Utc::now().timestamp_millis();
    let cutoff_ms = now_ms - STALE_TAIL_MS;

    let mut out: Vec<PathBuf> = Vec::new();
    let projects_str = projects.to_string_lossy();
    let session_dirs = match sftp.read_dir(projects_str.as_ref()).await {
        Ok(d) => d,
        Err(_) => return Ok(out),
    };
    for sd in session_dirs {
        if !sd.metadata().is_dir() {
            continue;
        }
        let dir = projects.join(sd.file_name());
        let dir_str = dir.to_string_lossy();
        let entries = match sftp.read_dir(dir_str.as_ref()).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            let name = entry.file_name();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let mtime_ms = entry
                .metadata()
                .mtime
                .map(|t| (t as i64).saturating_mul(1000))
                .unwrap_or(0);
            if mtime_ms >= cutoff_ms {
                out.push(dir.join(name));
            }
        }
    }
    Ok(out)
}

async fn read_delta(
    sftp: &SftpSession,
    path: &str,
    start: u64,
    end: u64,
) -> Result<String> {
    use russh_sftp::protocol::OpenFlags;
    use tokio::io::AsyncSeekExt;
    let mut file = sftp.open_with_flags(path, OpenFlags::READ).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let want = (end - start) as usize;
    let mut out = Vec::with_capacity(want);
    let mut buf = vec![0u8; READ_CHUNK_BYTES];
    let mut got = 0usize;
    while got < want {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let take = n.min(want - got);
        out.extend_from_slice(&buf[..take]);
        got += take;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}
