import { Sparkles, Trash2, X } from "lucide-react";
import { useVizStore } from "../store/vizStore";
import { FollowToggle } from "./FollowToggle";
import { tauri } from "../lib/tauri";

export function TopBar() {
  const watches = useVizStore((s) => s.watches);
  const followLatest = useVizStore((s) => s.followLatest);
  const toggleFollow = useVizStore((s) => s.toggleFollow);
  const removeWatch = useVizStore((s) => s.removeWatch);
  const clearGalleryStore = useVizStore((s) => s.clearGallery);

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
          <div
            key={w.id}
            className="flex items-center gap-1.5 px-2 py-0.5 rounded text-[11px] bg-[color:var(--color-surface-2)] border border-[color:var(--color-border)] flex-shrink-0"
          >
            <span className="font-mono text-[color:var(--color-text-dim)] truncate max-w-[280px]">
              {w.source.kind === "local"
                ? w.source.path
                : `${w.source.user}@${w.source.host}:${w.source.remote_path}`}
            </span>
            <button
              onClick={async () => {
                await tauri.removeWatch(w.id);
                removeWatch(w.id);
              }}
              title="Stop watching"
              className="opacity-50 hover:opacity-100"
            >
              <X className="w-3 h-3" />
            </button>
          </div>
        ))}
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
    </div>
  );
}
