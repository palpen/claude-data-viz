import { useEffect, useState } from "react";
import { AlertTriangle, ArrowUp, FolderOpen, FolderTree, Loader2, X } from "lucide-react";
import { tauri } from "../lib/tauri";
import { useVizStore } from "../store/vizStore";
import type { RemoteDirListing, Watch } from "../types";

interface Props {
  watch: Watch;
  onClose: () => void;
}

export function ChangeRemoteFolderDialog({ watch, onClose }: Props) {
  if (watch.source.kind !== "ssh") {
    throw new Error("ChangeRemoteFolderDialog only supports SSH watches");
  }
  const replaceWatch = useVizStore((s) => s.replaceWatch);

  const [listing, setListing] = useState<RemoteDirListing | null>(null);
  // Decoupled from `listing.current`: typed edits shouldn't trigger a network round-trip until
  // the user submits or hits Enter, otherwise every keystroke fires a list_remote_dirs.
  const [draftPath, setDraftPath] = useState(watch.source.remote_path);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const load = async (path: string | null) => {
    setLoading(true);
    setErr(null);
    try {
      const r = await tauri.listRemoteDirs(watch.id, path);
      setListing(r);
      setDraftPath(r.current);
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load(watch.source.kind === "ssh" ? watch.source.remote_path : null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const navigate = (path: string) => {
    void load(path);
  };

  const onUseThisFolder = async () => {
    const target = draftPath.trim() || (listing?.current ?? "");
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

          <div className="rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] max-h-[280px] overflow-y-auto">
            {loading && (
              <div className="px-3 py-4 flex items-center gap-2 text-[12px] text-[color:var(--color-text-dim)]">
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                Listing…
              </div>
            )}

            {!loading && listing && (
              <div className="py-1">
                {listing.parent != null && (
                  <button
                    type="button"
                    onClick={() => navigate(listing.parent!)}
                    className="w-full flex items-center gap-2 px-3 py-1.5 text-[12px] hover:bg-[color:var(--color-surface-2)]"
                  >
                    <ArrowUp className="w-3.5 h-3.5 text-[color:var(--color-text-dim)]" />
                    <span className="font-mono text-[color:var(--color-text-dim)]">..</span>
                  </button>
                )}
                {listing.dirs.length === 0 && listing.parent == null && (
                  <div className="px-3 py-3 text-[12px] text-[color:var(--color-text-dim)]">
                    No subdirectories.
                  </div>
                )}
                {listing.dirs.map((d) => {
                  const next = listing.current.endsWith("/")
                    ? `${listing.current}${d}`
                    : `${listing.current}/${d}`;
                  return (
                    <button
                      key={d}
                      type="button"
                      onClick={() => navigate(next)}
                      className="w-full flex items-center gap-2 px-3 py-1.5 text-[12px] hover:bg-[color:var(--color-surface-2)] text-left"
                    >
                      <FolderOpen className="w-3.5 h-3.5 text-[color:var(--color-accent)]/80" />
                      <span className="font-mono">{d}</span>
                    </button>
                  );
                })}
              </div>
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
