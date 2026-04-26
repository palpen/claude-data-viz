import { clsx } from "clsx";
import { formatDistanceToNow } from "date-fns";
import { MessageSquare } from "lucide-react";
import { convertFileSrc } from "../lib/tauri";
import { useVizStore } from "../store/vizStore";
import type { VizItem } from "../types";
import { iconForKind, labelForKind, rendersInlineImagePreview } from "../viewers";
import { useRemoteAsset } from "./RemoteAssetGate";

function basename(p: string): string {
  const parts = p.split("/");
  return parts[parts.length - 1] || p;
}

export function VizCard({
  item,
  selected,
  onClick,
}: {
  item: VizItem;
  selected: boolean;
  onClick: () => void;
}) {
  const Icon = iconForKind(item.kind);
  const isImage = rendersInlineImagePreview(item.kind);
  const isDeleted = item.status === "deleted";
  const isRemote = useVizStore((s) =>
    s.watches.find((w) => w.id === item.watch_id)?.source.kind === "ssh",
  );
  // For SSH items we can't read abs_path directly — route through the fetch cache. Cache miss
  // returns null; we fall back to the kind icon until the fetch completes.
  const remoteLocalPath = useRemoteAsset(item.watch_id, item.abs_path, isRemote && isImage);
  const thumbSrc = isImage
    ? isRemote
      ? remoteLocalPath
        ? `${convertFileSrc(remoteLocalPath)}?v=${item.mtime}`
        : null
      : `${convertFileSrc(item.abs_path)}?v=${item.mtime}`
    : null;

  return (
    <button
      onClick={onClick}
      className={clsx(
        "w-full text-left p-2 flex gap-2.5 rounded transition-colors border",
        selected
          ? "bg-[color:var(--color-accent)]/10 border-[color:var(--color-accent)]/40"
          : "bg-transparent border-transparent hover:bg-[color:var(--color-surface-2)] hover:border-[color:var(--color-border)]",
        isDeleted && "opacity-40"
      )}
    >
      <div className="w-12 h-12 flex-shrink-0 rounded bg-[color:var(--color-surface-2)] border border-[color:var(--color-border)] overflow-hidden flex items-center justify-center">
        {thumbSrc ? (
          <img
            src={thumbSrc}
            alt=""
            className="w-full h-full object-cover"
            loading="lazy"
          />
        ) : (
          <Icon className="w-5 h-5 opacity-50" />
        )}
      </div>
      <div className="flex-1 min-w-0">
        {item.prompt ? (
          <div className="text-[12px] truncate text-[color:var(--color-text)] flex items-center gap-1">
            <MessageSquare className="w-3 h-3 text-[color:var(--color-accent)] flex-shrink-0" />
            <span className="truncate">{item.prompt}</span>
          </div>
        ) : (
          <div className="text-[12px] truncate text-[color:var(--color-text-dim)] italic">
            no prompt linked
          </div>
        )}
        <div className="text-[11px] text-[color:var(--color-text-dim)] truncate font-mono mt-0.5">
          {basename(item.rel_path)}
        </div>
        <div className="text-[10px] text-[color:var(--color-text-dim)] truncate flex gap-1.5 items-center mt-0.5 opacity-70">
          <span className="uppercase tracking-wide">{labelForKind(item.kind)}</span>
          <span>·</span>
          <span>{formatDistanceToNow(item.mtime, { addSuffix: true })}</span>
          {isDeleted && <span className="text-red-400">· deleted</span>}
        </div>
      </div>
    </button>
  );
}
