import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useVizStore } from "../store/vizStore";
import { VizCard } from "./VizCard";
import { itemId } from "../types";

export function Sidebar() {
  const order = useVizStore((s) => s.order);
  const items = useVizStore((s) => s.items);
  const selectedId = useVizStore((s) => s.selectedId);
  const select = useVizStore((s) => s.select);

  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: order.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 76,
    overscan: 6,
  });

  return (
    <div className="h-full flex flex-col bg-[color:var(--color-surface)] border-r border-[color:var(--color-border)]">
      <div className="px-3 py-2.5 border-b border-[color:var(--color-border)] text-[11px] uppercase tracking-wider text-[color:var(--color-text-dim)] flex items-center justify-between">
        <span>Recent ({order.length})</span>
      </div>
      <div ref={parentRef} className="flex-1 min-h-0 overflow-auto overscroll-contain">
        {order.length === 0 ? (
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
              const id = order[vRow.index];
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
    </div>
  );
}
