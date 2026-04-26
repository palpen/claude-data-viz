//! On-disk cache of files fetched over SFTP. Path-preserving layout (debuggable, no opaque
//! sha1), atomic writes (.tmp → fsync → rename), in-flight dedup (concurrent fetches of the
//! same path collapse onto one SFTP download), and LRU eviction with a 1GB total cap.

use crate::ssh::connection::RemoteConnection;
use anyhow::{anyhow, Context, Result};
use filetime::{set_file_atime, FileTime};
use parking_lot::Mutex;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tauri::{AppHandle, Manager};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;

const TOTAL_CAP_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB
const FETCH_CHUNK_BYTES: usize = 64 * 1024;

/// Per-watch cache root: `<app_cache_dir>/remote/<watch_id>/`.
pub fn cache_root(app: &AppHandle, watch_id: &str) -> Result<PathBuf> {
    let base = app
        .path()
        .app_cache_dir()
        .context("could not resolve app cache dir")?;
    let dir = base.join("remote").join(watch_id);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Map a remote absolute path to a local cache path. Path-preserving (slashes become double
/// underscores) so a user inspecting the cache dir can tell what came from where.
pub fn local_path(app: &AppHandle, watch_id: &str, remote_path: &str) -> Result<PathBuf> {
    let root = cache_root(app, watch_id)?;
    Ok(root.join(sanitize(remote_path)))
}

fn sanitize(remote_path: &str) -> String {
    let trimmed = remote_path.trim_start_matches('/');
    trimmed
        .chars()
        .map(|c| match c {
            '/' => '_',
            ':' | '?' | '*' | '"' | '<' | '>' | '|' | '\\' => '_',
            _ => c,
        })
        .collect()
}

/// In-flight fetch dedup: per-key async mutex. Second concurrent caller waits on the same
/// lock; once it acquires, the cache check sees the freshly-downloaded file and short-circuits.
#[derive(Default)]
pub struct FetchLocks {
    inner: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl FetchLocks {
    fn lock_for(&self, key: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self.inner.lock();
        map.entry(key.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

/// Fetch (or no-op cache hit) a single file. Returns the local path the asset protocol can
/// serve. The remote `metadata` (mtime, size) is used for cache validation — if local matches,
/// no SFTP download.
pub async fn fetch_file(
    app: &AppHandle,
    locks: &FetchLocks,
    watch_id: &str,
    conn: &Arc<RemoteConnection>,
    remote_path: &str,
    remote_mtime_ms: i64,
    remote_size: u64,
) -> Result<PathBuf> {
    let local = local_path(app, watch_id, remote_path)?;
    let lock_key = format!("{}::{}", watch_id, remote_path);
    let lock = locks.lock_for(&lock_key);
    let _guard = lock.lock().await;

    if cache_hit(&local, remote_mtime_ms, remote_size) {
        let _ = touch_atime(&local);
        return Ok(local);
    }

    let sftp = conn.open_sftp().await?;
    download_atomic(&sftp, remote_path, &local).await?;
    set_local_mtime(&local, remote_mtime_ms);
    let _ = touch_atime(&local);

    // Eviction is best-effort and non-fatal.
    if let Some(root) = local.parent().and_then(|p| p.parent()) {
        let _ = enforce_cap(root, TOTAL_CAP_BYTES);
    }
    Ok(local)
}

/// Optionally fetch HTML siblings (depth=1, same dir, capped). Best-effort: failures don't
/// abort the primary fetch.
pub async fn fetch_html_siblings(
    app: &AppHandle,
    locks: &FetchLocks,
    watch_id: &str,
    conn: &Arc<RemoteConnection>,
    html_remote_path: &str,
) -> Result<usize> {
    const SIBLING_TOTAL_CAP_BYTES: u64 = 50 * 1024 * 1024;
    const SIBLING_FILE_CAP: usize = 100;

    let parent_remote = match Path::new(html_remote_path).parent() {
        Some(p) => p.to_string_lossy().into_owned(),
        None => return Ok(0),
    };
    if parent_remote.is_empty() {
        return Ok(0);
    }

    let sftp = conn.open_sftp().await?;
    let entries = sftp
        .read_dir(&parent_remote)
        .await
        .with_context(|| format!("read_dir {}", parent_remote))?;

    let mut fetched = 0usize;
    let mut bytes = 0u64;
    for entry in entries {
        if fetched >= SIBLING_FILE_CAP || bytes >= SIBLING_TOTAL_CAP_BYTES {
            break;
        }
        let name = entry.file_name();
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let remote_path = if parent_remote.ends_with('/') {
            format!("{}{}", parent_remote, name)
        } else {
            format!("{}/{}", parent_remote, name)
        };
        if remote_path == html_remote_path {
            continue; // already fetched as primary
        }
        let metadata = entry.metadata();
        if !metadata.is_regular() {
            continue;
        }
        let size = metadata.size.unwrap_or(0);
        if bytes + size > SIBLING_TOTAL_CAP_BYTES {
            continue;
        }
        let mtime_ms = metadata
            .mtime
            .map(|t| (t as i64).saturating_mul(1000))
            .unwrap_or(0);
        match fetch_file(app, locks, watch_id, conn, &remote_path, mtime_ms, size).await {
            Ok(_) => {
                fetched += 1;
                bytes += size;
            }
            Err(e) => {
                eprintln!("ssh: sibling fetch {remote_path} failed: {e}");
            }
        }
    }
    Ok(fetched)
}

/// Lightweight, pure view of the metadata bits we use for cache validation. Keeps
/// `is_cache_valid` testable without touching the filesystem.
pub(crate) struct CacheMetaView {
    pub size: u64,
    pub mtime_ms: i64,
}

/// Pure cache-validity predicate. Sized-then-mtime check that tolerates coarse-grained mtime
/// reporting (NFS at 2s, FAT/SSHFS combos) while still bailing on truly stale or
/// implausibly-far-future local files.
///
/// Rules:
///   1. size must match exactly,
///   2. local mtime must be at least the remote mtime (so we never serve a stale local copy),
///   3. local mtime must not exceed remote mtime by more than 24h (forward-skew cap so a
///      `touch` far in the future doesn't pin the cache forever).
pub(crate) fn is_cache_valid(local: &CacheMetaView, remote: &CacheMetaView) -> bool {
    const FORWARD_SKEW_CAP_MS: i64 = 24 * 3600 * 1000;
    if local.size != remote.size {
        return false;
    }
    if local.mtime_ms < remote.mtime_ms {
        return false;
    }
    if local.mtime_ms.saturating_sub(remote.mtime_ms) > FORWARD_SKEW_CAP_MS {
        return false;
    }
    true
}

fn cache_hit(local: &Path, remote_mtime_ms: i64, remote_size: u64) -> bool {
    let Ok(meta) = std::fs::metadata(local) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let local_ms = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    is_cache_valid(
        &CacheMetaView {
            size: meta.len(),
            mtime_ms: local_ms,
        },
        &CacheMetaView {
            size: remote_size,
            mtime_ms: remote_mtime_ms,
        },
    )
}

async fn download_atomic(sftp: &SftpSession, remote_path: &str, local: &Path) -> Result<()> {
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = local.with_extension("tmp.dl");
    {
        let mut remote = sftp
            .open(remote_path)
            .await
            .with_context(|| format!("sftp open {remote_path}"))?;
        let mut out = tokio::fs::File::create(&tmp)
            .await
            .with_context(|| format!("creating {}", tmp.display()))?;
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; FETCH_CHUNK_BYTES];
        loop {
            let n = remote
                .read(&mut buf)
                .await
                .context("reading from sftp file")?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).await.context("writing to tmp")?;
        }
        out.flush().await.ok();
        out.sync_all().await.ok();
    }
    tokio::fs::rename(&tmp, local)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), local.display()))?;
    Ok(())
}

fn set_local_mtime(local: &Path, mtime_ms: i64) {
    let secs = mtime_ms / 1000;
    let nanos = ((mtime_ms % 1000) * 1_000_000) as u32;
    let ft = FileTime::from_unix_time(secs, nanos);
    let _ = filetime::set_file_mtime(local, ft);
}

fn touch_atime(local: &Path) -> Result<()> {
    let now = FileTime::from_system_time(SystemTime::now());
    set_file_atime(local, now).map_err(|e| anyhow!("set_file_atime: {e}"))
}

/// Walk the per-watch cache root tree, evicting oldest-atime files until total ≤ cap.
fn enforce_cap(root: &Path, cap: u64) -> Result<()> {
    let mut all: Vec<(PathBuf, u64, FileTime)> = Vec::new();
    let mut total: u64 = 0;
    walk(root, &mut |path, meta| {
        let len = meta.len();
        let atime = FileTime::from_last_access_time(meta);
        all.push((path.to_path_buf(), len, atime));
        total += len;
    })?;
    if total <= cap {
        return Ok(());
    }
    all.sort_by_key(|(_, _, atime)| (atime.unix_seconds(), atime.nanoseconds()));
    let mut over = total.saturating_sub(cap);
    for (path, len, _) in all {
        if over == 0 {
            break;
        }
        let _ = std::fs::remove_file(&path);
        over = over.saturating_sub(len);
    }
    Ok(())
}

fn walk<F: FnMut(&Path, &std::fs::Metadata)>(root: &Path, f: &mut F) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                f(&path, &meta);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_cache_valid (pure) -----------------------------------------------------------

    #[test]
    fn is_cache_valid_size_mismatch() {
        let local = CacheMetaView {
            size: 100,
            mtime_ms: 1_000_000,
        };
        let remote = CacheMetaView {
            size: 200,
            mtime_ms: 1_000_000,
        };
        assert!(!is_cache_valid(&local, &remote));
    }

    #[test]
    fn is_cache_valid_mtime_equal() {
        let local = CacheMetaView {
            size: 4096,
            mtime_ms: 1_700_000_000_000,
        };
        let remote = CacheMetaView {
            size: 4096,
            mtime_ms: 1_700_000_000_000,
        };
        assert!(is_cache_valid(&local, &remote));
    }

    #[test]
    fn is_cache_valid_local_newer() {
        // Local mtime up to 24h ahead of remote is fine — covers coarse mtime + small skew.
        let remote_ms: i64 = 1_700_000_000_000;
        let local = CacheMetaView {
            size: 1024,
            mtime_ms: remote_ms + 5_000, // 5s ahead
        };
        let remote = CacheMetaView {
            size: 1024,
            mtime_ms: remote_ms,
        };
        assert!(is_cache_valid(&local, &remote));
    }

    #[test]
    fn is_cache_valid_local_older() {
        // Local strictly older than remote — file changed upstream, must re-fetch.
        let remote_ms: i64 = 1_700_000_000_000;
        let local = CacheMetaView {
            size: 1024,
            mtime_ms: remote_ms - 1,
        };
        let remote = CacheMetaView {
            size: 1024,
            mtime_ms: remote_ms,
        };
        assert!(!is_cache_valid(&local, &remote));
    }

    #[test]
    fn is_cache_valid_size_zero_edge_case() {
        // Both zero, equal mtime → valid.
        let local = CacheMetaView {
            size: 0,
            mtime_ms: 1_700_000_000_000,
        };
        let remote = CacheMetaView {
            size: 0,
            mtime_ms: 1_700_000_000_000,
        };
        assert!(is_cache_valid(&local, &remote));

        // Zero local against non-zero remote → invalid.
        let local_zero = CacheMetaView {
            size: 0,
            mtime_ms: 1_700_000_000_000,
        };
        let remote_nonzero = CacheMetaView {
            size: 10,
            mtime_ms: 1_700_000_000_000,
        };
        assert!(!is_cache_valid(&local_zero, &remote_nonzero));
    }

    #[test]
    fn is_cache_valid_local_implausibly_far_ahead() {
        // Local more than 24h ahead of remote → stale (someone touched it into the future).
        let remote_ms: i64 = 1_700_000_000_000;
        let twenty_five_hours_ms: i64 = 25 * 3600 * 1000;
        let local = CacheMetaView {
            size: 1024,
            mtime_ms: remote_ms + twenty_five_hours_ms,
        };
        let remote = CacheMetaView {
            size: 1024,
            mtime_ms: remote_ms,
        };
        assert!(!is_cache_valid(&local, &remote));
    }
}
