import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { FolderTree } from "lucide-react";
import { useVizStore } from "../store/vizStore";
import { VizCard } from "./VizCard";
import { ChangeRemoteFolderDialog } from "./ChangeRemoteFolderDialog";
import { itemId } from "../types";

export function Sidebar() {
  const order = useVizStore((s) => s.order);
  const items = useVizStore((s) => s.items);
  const watches = useVizStore((s) => s.watches);
  const activeWatchId = useVizStore((s) => s.activeWatchId);
  const selectedId = useVizStore((s) => s.selectedId);
  const select = useVizStore((s) => s.select);

  const parentRef = useRef<HTMLDivElement>(null);
  const [showChangeFolder, setShowChangeFolder] = useState(false);

  // The "change folder" affordance only makes sense when one SSH watch is unambiguously in
  // focus: either the user explicitly selected a tab, or there's only one watch and it's SSH.
  const focusedSshWatch = useMemo(() => {
    if (activeWatchId) {
      const w = watches.find((x) => x.id === activeWatchId);
      return w && w.source.kind === "ssh" ? w : null;
    }
    if (watches.length === 1 && watches[0].source.kind === "ssh") return watches[0];
    return null;
  }, [watches, activeWatchId]);

  // Filter to (a) items whose watch_id is in the active watches list (defense against orphans
  // from a removed watch) and (b) the focused tab if the user selected one.
  const visibleOrder = useMemo(() => {
    const activeIds = new Set(watches.map((w) => w.id));
    return order.filter((id) => {
      const it = items[id];
      if (it == null || !activeIds.has(it.watch_id)) return false;
      if (activeWatchId && it.watch_id !== activeWatchId) return false;
      return true;
    });
  }, [order, items, watches, activeWatchId]);

  const virtualizer = useVirtualizer({
    count: visibleOrder.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 76,
    overscan: 6,
  });

  return (
    <div className="h-full flex flex-col bg-[color:var(--color-surface)] border-r border-[color:var(--color-border)]">
      <div className="px-3 py-2.5 border-b border-[color:var(--color-border)] text-[11px] uppercase tracking-wider text-[color:var(--color-text-dim)] flex items-center justify-between gap-2">
        <span>Recent ({visibleOrder.length})</span>
        {focusedSshWatch && (
          <button
            onClick={() => setShowChangeFolder(true)}
            title="Change which folder this remote is watching"
            className="flex items-center gap-1 normal-case tracking-normal px-2 py-0.5 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface-2)] hover:border-[color:var(--color-accent)]/60 hover:text-[color:var(--color-text)]"
          >
            <FolderTree className="w-3 h-3" />
            Change folder
          </button>
        )}
      </div>
      <div ref={parentRef} className="flex-1 min-h-0 overflow-auto overscroll-contain">
        {visibleOrder.length === 0 ? (
          <div className="px-3 py-4 text-xs text-[color:var(--color-text-dim)]">
            Waiting for visualizations…
          </div>
        ) : (
          <div
            style={{
              height: `${virtualizer.getTotalSize()}px`,
              position: "relative",
            }}
          >
            {virtualizer.getVirtualItems().map((vRow) => {
              const id = visibleOrder[vRow.index];
              const item = items[id];
              if (!item) return null;
              const sel = selectedId === id;
              return (
                <div
                  key={id}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    right: 0,
                    transform: `translateY(${vRow.start}px)`,
                    padding: "2px 4px",
                  }}
                >
                  <VizCard
                    item={item}
                    selected={sel}
                    onClick={() => select(itemId(item.watch_id, item.abs_path))}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>
      {showChangeFolder && focusedSshWatch && (
        <ChangeRemoteFolderDialog
          watch={focusedSshWatch}
          onClose={() => setShowChangeFolder(false)}
        />
      )}
    </div>
  );
}
