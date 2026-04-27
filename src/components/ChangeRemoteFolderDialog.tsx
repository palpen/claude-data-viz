import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { AlertTriangle, ArrowUp, FolderOpen, FolderTree, Loader2, X } from "lucide-react";
import { tauri } from "../lib/tauri";
import { useVizStore } from "../store/vizStore";
import type { Watch } from "../types";

interface Props {
  watch: Watch;
  onClose: () => void;
}

// Trigger the next-page fetch this many rows before the rendered tail. Five rows of buffer
// at ~28px each is plenty to keep the list flowing without slamming the backend.
const PAGE_PREFETCH_BUFFER = 5;

export function ChangeRemoteFolderDialog({ watch, onClose }: Props) {
  if (watch.source.kind !== "ssh") {
    throw new Error("ChangeRemoteFolderDialog only supports SSH watches");
  }
  const replaceWatch = useVizStore((s) => s.replaceWatch);

  const [current, setCurrent] = useState<string>("");
  const [parent, setParent] = useState<string | null>(null);
  const [entries, setEntries] = useState<string[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  // Decoupled from `current`: typed edits shouldn't trigger a network round-trip until the
  // user submits or hits Enter, otherwise every keystroke fires a list_remote_dirs.
  const [draftPath, setDraftPath] = useState(watch.source.remote_path);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Track the path whose pagination is currently in flight so a stale loadMore() callback
  // can't append rows belonging to a directory the user already navigated away from.
  const activePathRef = useRef<string>("");

  const parentRef = useRef<HTMLDivElement | null>(null);

  const load = async (path: string | null) => {
    setLoading(true);
    setLoadingMore(false);
    setErr(null);
    try {
      const r = await tauri.listRemoteDirs(watch.id, path, null, null);
      activePathRef.current = r.current;
      setCurrent(r.current);
      setParent(r.parent);
      setEntries(r.entries);
      setNextCursor(r.next_cursor);
      setDraftPath(r.current);
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoading(false);
    }
  };

  const loadMore = async () => {
    if (loadingMore || !nextCursor) return;
    const pathAtRequest = activePathRef.current;
    setLoadingMore(true);
    try {
      const r = await tauri.listRemoteDirs(watch.id, pathAtRequest, nextCursor, null);
      // If the user navigated elsewhere mid-flight, drop the stale page on the floor.
      if (activePathRef.current !== pathAtRequest) return;
      setEntries((prev) => [...prev, ...r.entries]);
      setNextCursor(r.next_cursor);
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoadingMore(false);
    }
  };

  useEffect(() => {
    void load(watch.source.kind === "ssh" ? watch.source.remote_path : null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const navigate = (path: string) => {
    void load(path);
  };

  const onUseThisFolder = async () => {
    const target = draftPath.trim() || current;
    if (!target) return;
    setBusy(true);
    setErr(null);
    try {
      const updated = await tauri.updateRemoteWatchPath(watch.id, target);
      replaceWatch(updated);
      onClose();
    } catch (e) {
      setErr(String(e));
      setBusy(false);
    }
  };

  // One row in the virtualizer per directory entry. The ".." row and the loading footer
  // sit *outside* the virtualizer so they don't fight for keyboard / scroll position.
  const virtualizer = useVirtualizer({
    count: entries.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 28,
    overscan: 6,
  });

  // Index-based prefetch: when the virtualizer renders any row near the tail, pull the
  // next page. Avoids IntersectionObserver wiring entirely.
  const virtualItems = virtualizer.getVirtualItems();
  useEffect(() => {
    if (loading || loadingMore || !nextCursor) return;
    const last = virtualItems[virtualItems.length - 1];
    if (!last) return;
    if (last.index >= entries.length - PAGE_PREFETCH_BUFFER) {
      void loadMore();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [virtualItems, entries.length, nextCursor, loading, loadingMore]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl rounded-lg border border-[color:var(--color-border)] bg-[color:var(--color-bg)] shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-[color:var(--color-border)]">
          <div className="flex items-center gap-2">
            <FolderTree className="w-4 h-4 text-[color:var(--color-accent)]" />
            <span className="text-[14px] font-semibold">Change folder</span>
            <span className="text-[11px] text-[color:var(--color-text-dim)] ml-1 font-mono">
              {watch.source.user}@{watch.source.host}
            </span>
          </div>
          <button
            onClick={onClose}
            className="text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)]"
            aria-label="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="px-5 py-4 space-y-3">
          <div>
            <label className="text-[11px] uppercase tracking-wider text-[color:var(--color-text-dim)]">
              Path
            </label>
            <div className="mt-1 flex gap-2">
              <input
                type="text"
                value={draftPath}
                onChange={(e) => setDraftPath(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    navigate(draftPath || "");
                  }
                }}
                placeholder="(home directory)"
                className="flex-1 px-3 py-2 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] text-[13px] font-mono placeholder:opacity-40 focus:outline-none focus:border-[color:var(--color-accent)]/60"
              />
              <button
                type="button"
                onClick={() => navigate(draftPath || "")}
                disabled={loading}
                className="px-3 py-2 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] hover:border-[color:var(--color-accent)]/60 text-[12px] disabled:opacity-50"
              >
                Go
              </button>
            </div>
          </div>

          <div className="rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)]">
            {loading && (
              <div className="px-3 py-4 flex items-center gap-2 text-[12px] text-[color:var(--color-text-dim)]">
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                Listing…
              </div>
            )}

            {!loading && (
              <>
                {parent != null && (
                  <button
                    type="button"
                    onClick={() => navigate(parent)}
                    className="w-full flex items-center gap-2 px-3 py-1.5 text-[12px] hover:bg-[color:var(--color-surface-2)] border-b border-[color:var(--color-border)]"
                  >
                    <ArrowUp className="w-3.5 h-3.5 text-[color:var(--color-text-dim)]" />
                    <span className="font-mono text-[color:var(--color-text-dim)]">..</span>
                  </button>
                )}

                {entries.length === 0 && parent == null && (
                  <div className="px-3 py-3 text-[12px] text-[color:var(--color-text-dim)]">
                    No subdirectories.
                  </div>
                )}

                {entries.length > 0 && (
                  <div
                    ref={parentRef}
                    className="max-h-[280px] overflow-y-auto overscroll-contain"
                  >
                    <div
                      style={{
                        height: `${virtualizer.getTotalSize()}px`,
                        position: "relative",
                      }}
                    >
                      {virtualItems.map((vRow) => {
                        const d = entries[vRow.index];
                        if (d === undefined) return null;
                        const next = current.endsWith("/")
                          ? `${current}${d}`
                          : `${current}/${d}`;
                        return (
                          <button
                            key={`${vRow.index}-${d}`}
                            type="button"
                            onClick={() => navigate(next)}
                            style={{
                              position: "absolute",
                              top: 0,
                              left: 0,
                              right: 0,
                              transform: `translateY(${vRow.start}px)`,
                              height: `${vRow.size}px`,
                            }}
                            className="flex items-center gap-2 px-3 text-[12px] hover:bg-[color:var(--color-surface-2)] text-left"
                          >
                            <FolderOpen className="w-3.5 h-3.5 text-[color:var(--color-accent)]/80" />
                            <span className="font-mono truncate">{d}</span>
                          </button>
                        );
                      })}
                    </div>
                    {loadingMore && (
                      <div className="px-3 py-2 flex items-center gap-2 text-[11px] text-[color:var(--color-text-dim)] border-t border-[color:var(--color-border)]">
                        <Loader2 className="w-3 h-3 animate-spin" />
                        Loading more…
                      </div>
                    )}
                  </div>
                )}
              </>
            )}
          </div>

          {err && (
            <div className="text-[12px] text-red-300 flex items-start gap-1.5">
              <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
              <span className="whitespace-pre-wrap break-words">{err}</span>
            </div>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-[color:var(--color-border)]">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="px-3 py-1.5 rounded text-[12px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onUseThisFolder}
            disabled={busy || loading}
            className="px-3 py-1.5 rounded text-[12px] font-medium bg-[color:var(--color-accent)] text-black hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
          >
            {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
            Use this folder
          </button>
        </div>
      </div>
    </div>
  );
}
