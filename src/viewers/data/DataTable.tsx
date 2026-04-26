import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, ArrowUpDown, AlertTriangle } from "lucide-react";
import type { Column, Dataset, ParseError } from "./types";

const ROW_HEIGHT = 28;
const MIN_COL_WIDTH = 160;
const HEADER_HEIGHT = 32;

type SortDir = "asc" | "desc";
interface SortState {
  column: number;
  dir: SortDir;
}

export function DataTable({ dataset }: { dataset: Dataset }) {
  const [sort, setSort] = useState<SortState | null>(null);
  const [errorsOpen, setErrorsOpen] = useState(false);

  const sortIndex = useMemo(
    () => (sort ? buildSortIndex(dataset, sort) : null),
    [dataset, sort]
  );

  const parentRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: dataset.rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  const minContentWidth = dataset.columns.length * MIN_COL_WIDTH;

  function cycleSort(col: number) {
    setSort((prev) => {
      if (!prev || prev.column !== col) return { column: col, dir: "asc" };
      if (prev.dir === "asc") return { column: col, dir: "desc" };
      return null;
    });
  }

  if (dataset.columns.length === 0) {
    return (
      <div className="p-6 text-sm text-[color:var(--color-text-dim)]">
        Empty file — no rows or columns parsed.
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col bg-[color:var(--color-bg)] text-[12px]">
      {dataset.truncation && (
        <TruncationBanner
          shown={dataset.rows.length}
          total={dataset.truncation.totalRows}
        />
      )}
      {dataset.parseErrors.length > 0 && (
        <ErrorBanner
          errors={dataset.parseErrors}
          open={errorsOpen}
          toggle={() => setErrorsOpen((v) => !v)}
        />
      )}
      {errorsOpen && dataset.parseErrors.length > 0 && (
        <ErrorList errors={dataset.parseErrors} />
      )}
      <div ref={parentRef} className="flex-1 min-h-0 overflow-auto overscroll-contain font-mono">
        <div
          style={{ minWidth: minContentWidth, width: "100%", position: "relative" }}
        >
          <div
            className="sticky top-0 z-10 flex bg-[color:var(--color-surface)] border-b border-[color:var(--color-border)]"
            style={{ height: HEADER_HEIGHT }}
          >
            {dataset.columns.map((col, i) => (
              <HeaderCell
                key={col.key}
                col={col}
                sort={sort?.column === i ? sort.dir : null}
                onClick={() => cycleSort(i)}
              />
            ))}
          </div>
          <div
            style={{
              height: `${virtualizer.getTotalSize()}px`,
              position: "relative",
            }}
          >
            {virtualizer.getVirtualItems().map((vRow) => {
              const rowIdx = sortIndex ? sortIndex[vRow.index] : vRow.index;
              const row = dataset.rows[rowIdx];
              return (
                <div
                  key={vRow.index}
                  className="flex border-b border-[color:var(--color-border)]/50"
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    right: 0,
                    height: ROW_HEIGHT,
                    transform: `translateY(${vRow.start}px)`,
                  }}
                >
                  {dataset.columns.map((_, ci) => (
                    <Cell key={ci} value={row?.[ci] ?? ""} />
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

function HeaderCell({
  col,
  sort,
  onClick,
}: {
  col: Column;
  sort: SortDir | null;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={col.key}
      className="flex items-center gap-1 px-2 border-r border-[color:var(--color-border)]/50 text-[11px] uppercase tracking-wide text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] hover:bg-[color:var(--color-surface-2)] transition-colors text-left"
      style={{ flex: "1 1 0", minWidth: MIN_COL_WIDTH }}
    >
      <span className="truncate flex-1">{col.key}</span>
      {sort === "asc" ? (
        <ArrowUp className="w-3 h-3 flex-shrink-0 text-[color:var(--color-accent)]" />
      ) : sort === "desc" ? (
        <ArrowDown className="w-3 h-3 flex-shrink-0 text-[color:var(--color-accent)]" />
      ) : (
        <ArrowUpDown className="w-3 h-3 flex-shrink-0 opacity-40" />
      )}
    </button>
  );
}

function Cell({ value }: { value: string }) {
  if (value === "") {
    return (
      <div
        className="px-2 flex items-center border-r border-[color:var(--color-border)]/30 text-[color:var(--color-text-dim)]/40 text-left"
        style={{ flex: "1 1 0", minWidth: MIN_COL_WIDTH }}
      >
        —
      </div>
    );
  }
  return (
    <div
      className="px-2 flex items-center border-r border-[color:var(--color-border)]/30 text-[color:var(--color-text)] text-left"
      style={{ flex: "1 1 0", minWidth: MIN_COL_WIDTH }}
      title={value}
    >
      <span className="truncate">{value}</span>
    </div>
  );
}

function TruncationBanner({ shown, total }: { shown: number; total: number | null }) {
  return (
    <div className="px-3 py-1.5 text-[11px] text-[color:var(--color-text-dim)] bg-[color:var(--color-surface)]/50 border-b border-[color:var(--color-border)]">
      Showing first <span className="font-mono">{shown.toLocaleString()}</span>{" "}
      {total !== null ? (
        <>
          of <span className="font-mono">{total.toLocaleString()}</span> rows.
        </>
      ) : (
        <>rows · preview · file is larger</>
      )}
    </div>
  );
}

function ErrorBanner({
  errors,
  open,
  toggle,
}: {
  errors: ParseError[];
  open: boolean;
  toggle: () => void;
}) {
  return (
    <div className="px-3 py-1.5 text-[11px] text-[color:var(--color-text-dim)] bg-[color:var(--color-surface)]/50 border-b border-[color:var(--color-border)] flex gap-3 items-center flex-wrap">
      <button
        onClick={toggle}
        className="inline-flex items-center gap-1 text-[color:var(--color-accent)] hover:underline"
      >
        <AlertTriangle className="w-3 h-3" />
        {errors.length} row{errors.length === 1 ? "" : "s"} skipped
        <span className="opacity-60">({open ? "hide" : "show"})</span>
      </button>
    </div>
  );
}

function ErrorList({ errors }: { errors: ParseError[] }) {
  return (
    <div className="px-3 py-2 text-[11px] font-mono bg-[color:var(--color-surface-2)] border-b border-[color:var(--color-border)] max-h-40 overflow-auto">
      {errors.map((e, i) => (
        <div key={i} className="text-[color:var(--color-text-dim)]">
          <span className="text-[color:var(--color-accent)]">row {e.row}:</span>{" "}
          {e.message}
        </div>
      ))}
    </div>
  );
}

function buildSortIndex(dataset: Dataset, sort: SortState): number[] {
  const colIdx = sort.column;
  const dir = sort.dir === "asc" ? 1 : -1;
  const indices = dataset.rows.map((_, i) => i);
  indices.sort((a, b) => {
    const av = dataset.rows[a]?.[colIdx] ?? "";
    const bv = dataset.rows[b]?.[colIdx] ?? "";
    // Empty values sort last regardless of direction.
    if (av === "" && bv === "") return 0;
    if (av === "") return 1;
    if (bv === "") return -1;
    return av.localeCompare(bv, undefined, { numeric: true }) * dir;
  });
  return indices;
}
