import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  FolderOpen,
  RefreshCw,
  Server,
  Sparkles,
  Trash2,
  X,
  AlertTriangle,
} from "lucide-react";
import { useVizStore } from "../store/vizStore";
import { FollowToggle } from "./FollowToggle";
import { ConnectRemoteDialog } from "./ConnectRemoteDialog";
import { tauri } from "../lib/tauri";
import { swallowWithLog } from "../lib/log";
import type { Watch, WatchStatus } from "../types";

export function TopBar() {
  const watches = useVizStore((s) => s.watches);
  const watchStatus = useVizStore((s) => s.watchStatus);
  const activeWatchId = useVizStore((s) => s.activeWatchId);
  const setActiveWatchId = useVizStore((s) => s.setActiveWatchId);
  const followLatest = useVizStore((s) => s.followLatest);
  const toggleFollow = useVizStore((s) => s.toggleFollow);
  const addWatch = useVizStore((s) => s.addWatch);
  const removeWatch = useVizStore((s) => s.removeWatch);
  const clearGalleryStore = useVizStore((s) => s.clearGallery);

  const [showRemote, setShowRemote] = useState(false);
  const [addLocalErr, setAddLocalErr] = useState<string | null>(null);

  const onAddLocal = async () => {
    setAddLocalErr(null);
    const path = await open({
      directory: true,
      multiple: false,
      title: "Pick a folder to watch",
    });
    if (!path || typeof path !== "string") return;
    try {
      const watch = await tauri.addLocalWatch(path);
      addWatch(watch);
    } catch (e) {
      setAddLocalErr(String(e));
    }
  };

  const onToggleFollow = () => {
    toggleFollow();
    tauri.setFollowLatest(!followLatest).catch(swallowWithLog("TopBar: setFollowLatest"));
  };

  const onClear = async () => {
    clearGalleryStore();
    await tauri.clearGallery().catch(swallowWithLog("TopBar: clearGallery"));
  };

  return (
    <div className="flex-shrink-0 h-11 px-3 border-b border-[color:var(--color-border)] flex items-center gap-3 bg-[color:var(--color-surface)]">
      <div className="flex items-center gap-1.5 text-[color:var(--color-accent)]">
        <Sparkles className="w-4 h-4" />
        <span className="text-[13px] font-semibold tracking-wide">Claude Data Viz</span>
      </div>
      <div className="flex-1 flex items-center gap-1.5 overflow-x-auto">
        {watches.length > 1 && (
          <button
            onClick={() => setActiveWatchId(null)}
            title="Show items from all watches"
            className={`flex items-center px-2 py-0.5 rounded text-[11px] flex-shrink-0 border ${
              activeWatchId == null
                ? "bg-[color:var(--color-accent)]/15 border-[color:var(--color-accent)]/50 text-[color:var(--color-text)]"
                : "bg-[color:var(--color-surface-2)] border-[color:var(--color-border)] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)]"
            }`}
          >
            All
          </button>
        )}
        {watches.map((w) => (
          <WatchTab
            key={w.id}
            watch={w}
            status={watchStatus[w.id] ?? null}
            active={activeWatchId === w.id}
            onClick={() => {
              const next = activeWatchId === w.id ? null : w.id;
              setActiveWatchId(next);
              // Focusing a tab is the user signaling "show me this folder now" — kick off
              // a rescan so any files added since the last scan appear without a restart.
              // No-op for SSH (the SFTP poller handles its own refresh cadence).
              if (next === w.id && w.source.kind === "local") {
                tauri.rescanWatch(w.id).catch(swallowWithLog(`TopBar: rescanWatch(${w.id})`));
              }
            }}
            onRemove={async () => {
              await tauri.removeWatch(w.id);
              removeWatch(w.id);
            }}
          />
        ))}
        <button
          onClick={onAddLocal}
          title="Watch a local folder"
          className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[11px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] hover:bg-[color:var(--color-surface-2)] border border-transparent hover:border-[color:var(--color-border)] flex-shrink-0"
        >
          <FolderOpen className="w-3 h-3" />
          local
        </button>
        <button
          onClick={() => setShowRemote(true)}
          title="Connect to a remote server"
          className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[11px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] hover:bg-[color:var(--color-surface-2)] border border-transparent hover:border-[color:var(--color-border)] flex-shrink-0"
        >
          <Server className="w-3 h-3" />
          remote
        </button>
      </div>
      {addLocalErr && (
        <span
          className="text-[11px] text-red-300 truncate max-w-[260px]"
          title={addLocalErr}
        >
          {addLocalErr}
        </span>
      )}
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
  active,
  onClick,
  onRemove,
}: {
  watch: Watch;
  status: WatchStatus | null;
  active: boolean;
  onClick: () => void;
  onRemove: () => void;
}) {
  const label =
    watch.source.kind === "local"
      ? watch.source.path
      : `${watch.source.user}@${watch.source.host}:${watch.source.remote_path}`;
  return (
    <div
      className={`flex items-center gap-1.5 px-2 py-0.5 rounded text-[11px] border flex-shrink-0 ${
        active
          ? "bg-[color:var(--color-accent)]/15 border-[color:var(--color-accent)]/50"
          : "bg-[color:var(--color-surface-2)] border-[color:var(--color-border)]"
      }`}
    >
      {status && status.kind !== "connected" && (
        <StatusBadge status={status} watchId={watch.id} />
      )}
      <button
        onClick={onClick}
        title={active ? "Show all watches" : "Filter sidebar to this watch"}
        className={`font-mono truncate max-w-[280px] hover:text-[color:var(--color-text)] ${
          active ? "text-[color:var(--color-text)]" : "text-[color:var(--color-text-dim)]"
        }`}
      >
        {label}
      </button>
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
          onClick={() => tauri.reconnectWatch(watchId).catch(swallowWithLog(`TopBar: reconnectWatch(${watchId})`))}
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
