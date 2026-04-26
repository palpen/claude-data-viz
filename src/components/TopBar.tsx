import { useEffect, useState } from "react";
import { Plus, RefreshCw, Sparkles, Trash2, X, AlertTriangle } from "lucide-react";
import { useVizStore } from "../store/vizStore";
import { FollowToggle } from "./FollowToggle";
import { ConnectRemoteDialog } from "./ConnectRemoteDialog";
import { tauri } from "../lib/tauri";
import type { Watch, WatchStatus } from "../types";

export function TopBar() {
  const watches = useVizStore((s) => s.watches);
  const watchStatus = useVizStore((s) => s.watchStatus);
  const followLatest = useVizStore((s) => s.followLatest);
  const toggleFollow = useVizStore((s) => s.toggleFollow);
  const removeWatch = useVizStore((s) => s.removeWatch);
  const clearGalleryStore = useVizStore((s) => s.clearGallery);

  const [showRemote, setShowRemote] = useState(false);

  const onToggleFollow = () => {
    toggleFollow();
    tauri.setFollowLatest(!followLatest).catch(() => {});
  };

  const onClear = async () => {
    clearGalleryStore();
    await tauri.clearGallery().catch(() => {});
  };

  return (
    <div className="flex-shrink-0 h-11 px-3 border-b border-[color:var(--color-border)] flex items-center gap-3 bg-[color:var(--color-surface)]">
      <div className="flex items-center gap-1.5 text-[color:var(--color-accent)]">
        <Sparkles className="w-4 h-4" />
        <span className="text-[13px] font-semibold tracking-wide">Claude Data Viz</span>
      </div>
      <div className="flex-1 flex items-center gap-1.5 overflow-x-auto">
        {watches.map((w) => (
          <WatchTab
            key={w.id}
            watch={w}
            status={watchStatus[w.id] ?? null}
            onRemove={async () => {
              await tauri.removeWatch(w.id);
              removeWatch(w.id);
            }}
          />
        ))}
        <button
          onClick={() => setShowRemote(true)}
          title="Add remote server"
          className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[11px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] hover:bg-[color:var(--color-surface-2)] border border-transparent hover:border-[color:var(--color-border)] flex-shrink-0"
        >
          <Plus className="w-3 h-3" />
          remote
        </button>
      </div>
      <button
        onClick={onClear}
        title="Clear gallery"
        className="flex items-center gap-1.5 px-2 py-1 rounded text-[11px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] hover:bg-[color:var(--color-surface-2)] border border-transparent hover:border-[color:var(--color-border)]"
      >
        <Trash2 className="w-3 h-3" />
        Clear
      </button>
      <FollowToggle active={followLatest} onToggle={onToggleFollow} />
      {showRemote && <ConnectRemoteDialog onClose={() => setShowRemote(false)} />}
    </div>
  );
}

function WatchTab({
  watch,
  status,
  onRemove,
}: {
  watch: Watch;
  status: WatchStatus | null;
  onRemove: () => void;
}) {
  return (
    <div className="flex items-center gap-1.5 px-2 py-0.5 rounded text-[11px] bg-[color:var(--color-surface-2)] border border-[color:var(--color-border)] flex-shrink-0">
      {status && status.kind !== "connected" && (
        <StatusBadge status={status} watchId={watch.id} />
      )}
      <span className="font-mono text-[color:var(--color-text-dim)] truncate max-w-[280px]">
        {watch.source.kind === "local"
          ? watch.source.path
          : `${watch.source.user}@${watch.source.host}:${watch.source.remote_path}`}
      </span>
      <button
        onClick={onRemove}
        title="Stop watching"
        className="opacity-50 hover:opacity-100"
      >
        <X className="w-3 h-3" />
      </button>
    </div>
  );
}

function StatusBadge({ status, watchId }: { status: WatchStatus; watchId: string }) {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (status.kind === "reconnecting" || status.kind === "unreachable") {
      const t = window.setInterval(() => setTick((n) => n + 1), 1000);
      return () => window.clearInterval(t);
    }
  }, [status.kind]);

  const label = (() => {
    switch (status.kind) {
      case "reconnecting":
        return `reconnecting (${formatElapsed(status.since_ms)})`;
      case "unreachable":
        return `unreachable (${formatElapsed(status.since_ms)})`;
      case "auth_failed":
        return "auth failed";
      case "path_invalid":
        return "path invalid";
      case "stopped":
        return "stopped";
      default:
        return "";
    }
  })();

  const tone =
    status.kind === "auth_failed" || status.kind === "path_invalid"
      ? "text-red-300"
      : "text-amber-300/90";

  const canReconnect =
    status.kind === "auth_failed" || status.kind === "path_invalid";

  return (
    <span className={`flex items-center gap-1 ${tone}`} title={errorOf(status) ?? label}>
      <AlertTriangle className="w-3 h-3" />
      {label}
      {canReconnect && (
        <button
          onClick={() => tauri.reconnectWatch(watchId).catch(() => {})}
          title="Reconnect"
          className="ml-0.5 opacity-70 hover:opacity-100"
        >
          <RefreshCw className="w-3 h-3" />
        </button>
      )}
    </span>
  );
}

function formatElapsed(sinceMs: number): string {
  const elapsed = Math.max(0, Math.floor((Date.now() - sinceMs) / 1000));
  if (elapsed < 60) return `${elapsed}s`;
  const m = Math.floor(elapsed / 60);
  const s = elapsed % 60;
  return `${m}m${s.toString().padStart(2, "0")}s`;
}

function errorOf(status: WatchStatus): string | null {
  switch (status.kind) {
    case "reconnecting":
      return status.last_error;
    case "auth_failed":
    case "path_invalid":
    case "unreachable":
      return status.last_error;
    default:
      return null;
  }
}
