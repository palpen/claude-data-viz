import { useEffect, useState } from "react";
import { Sheet } from "lucide-react";
import { convertFileSrc } from "../lib/tauri";
import type { VizItem } from "../types";
import type { ViewerDefinition } from "./registry";
import { DataTable } from "./data/DataTable";
import type { Column, Dataset } from "./data/types";

// Sanity bound. Files over this are rejected outright. Streaming preview means we never load
// the whole file regardless of size, so this is a defense against pathological inputs more than
// a memory ceiling.
const SIZE_LIMIT = 200 * 1024 * 1024;
// How many bytes to read for the preview. ~1MB covers the first 5–15k rows for typical CSVs and
// reads in tens of milliseconds via Tauri's asset protocol regardless of total file size.
const PREVIEW_BYTES = 1 * 1024 * 1024;
// Cap rendered rows. JavaScriptCore + react-virtual hang when the virtualizer's total scroll
// height gets large; 5000 × 28px = 140kpx of scroll height stays well within WebKit's comfort.
const MAX_DISPLAY_ROWS = 5000;

type LoadState =
  | { status: "fetching" }
  | { status: "size-limit" }
  | { status: "error"; message: string }
  | { status: "ready"; dataset: Dataset };

function CsvView({ item }: { item: VizItem }) {
  const { abs_path: absPath, size, mtime } = item;
  const [state, setState] = useState<LoadState>({ status: "fetching" });

  useEffect(() => {
    if (size > SIZE_LIMIT) {
      setState({ status: "size-limit" });
      return;
    }

    let cancelled = false;
    const controller = new AbortController();
    setState({ status: "fetching" });
    const t0 = performance.now();

    const url = `${convertFileSrc(absPath)}?v=${mtime}`;

    (async () => {
      try {
        const { text, byteTruncated } = await readPreviewText(
          url,
          controller.signal,
          PREVIEW_BYTES
        );
        if (cancelled) return;
        const tFetched = performance.now();
        console.log(
          `[csv] fetch: ${(tFetched - t0).toFixed(0)}ms (${text.length} chars, byteTruncated=${byteTruncated})`
        );

        const { rows, totalRows } = parseCsvSimple(text, MAX_DISPLAY_ROWS + 1);
        if (cancelled) return;
        const tParsed = performance.now();
        console.log(
          `[csv] parse: ${(tParsed - tFetched).toFixed(0)}ms (parsed=${rows.length} totalInText=${totalRows})`
        );

        const dataset = buildDataset(rows, totalRows, byteTruncated);
        setState({ status: "ready", dataset });
        console.log(`[csv] total: ${(performance.now() - t0).toFixed(0)}ms`);
      } catch (e) {
        if (cancelled || (e instanceof DOMException && e.name === "AbortError")) return;
        console.error("[csv] error", e);
        setState({ status: "error", message: String(e) });
      }
    })();

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [absPath, size, mtime]);

  if (state.status === "fetching") {
    return <div className="p-6 text-sm opacity-60">Reading file…</div>;
  }
  if (state.status === "size-limit") {
    return (
      <div className="p-6 text-sm text-[color:var(--color-text-dim)]">
        File too large to preview:{" "}
        <span className="font-mono">{formatBytes(size)}</span> exceeds{" "}
        <span className="font-mono">{formatBytes(SIZE_LIMIT)}</span> limit.
      </div>
    );
  }
  if (state.status === "error") {
    return (
      <div className="p-6 text-sm text-red-300">
        Could not read CSV: {state.message}
      </div>
    );
  }
  return <DataTable dataset={state.dataset} />;
}

/**
 * Read up to `maxBytes` of the file via fetch's ReadableStream and stop early. If the stream
 * still has more data when we stop, we trim back to the last newline so the caller never sees
 * a half-row at the tail. Falls back to fetching the full body via res.text() if streams are
 * unavailable in this environment.
 */
async function readPreviewText(
  url: string,
  signal: AbortSignal,
  maxBytes: number
): Promise<{ text: string; byteTruncated: boolean }> {
  const res = await fetch(url, { signal });
  if (!res.ok) throw new Error(`HTTP ${res.status} reading file`);

  const body = res.body;
  if (!body) {
    const fullText = await res.text();
    if (fullText.length > maxBytes) {
      let trimmed = fullText.slice(0, maxBytes);
      const lastNl = trimmed.lastIndexOf("\n");
      if (lastNl > 0) trimmed = trimmed.slice(0, lastNl);
      return { text: trimmed, byteTruncated: true };
    }
    return { text: fullText, byteTruncated: false };
  }

  const reader = body.getReader();
  const decoder = new TextDecoder();
  const chunks: string[] = [];
  let bytesRead = 0;
  let exhausted = false;

  try {
    while (bytesRead < maxBytes) {
      const { value, done } = await reader.read();
      if (done) {
        exhausted = true;
        break;
      }
      bytesRead += value.length;
      chunks.push(decoder.decode(value, { stream: true }));
    }
  } finally {
    reader.cancel().catch(() => {});
  }

  let text = chunks.join("") + decoder.decode();

  // We bailed mid-stream — strip the (likely partial) last line so we don't render a half row.
  if (!exhausted) {
    const lastNl = text.lastIndexOf("\n");
    if (lastNl > 0) text = text.slice(0, lastNl);
  }

  return { text, byteTruncated: !exhausted };
}

/**
 * Hand-rolled splitter, capped at maxRows. Walks the buffer once for parsing and counts any
 * remaining lines without slicing (just newline-byte scan) so the UI can report total rows in
 * the *text it received*. Limitations: splits cells on raw commas, no quote handling.
 */
function parseCsvSimple(
  text: string,
  maxRows: number
): { rows: string[][]; totalRows: number } {
  const rows: string[][] = [];
  let lineStart = 0;
  let i = 0;

  while (i < text.length && rows.length < maxRows) {
    if (text.charCodeAt(i) === 10) {
      let line = text.slice(lineStart, i);
      if (line.length > 0 && line.charCodeAt(line.length - 1) === 13) {
        line = line.slice(0, -1);
      }
      if (line.length > 0) rows.push(line.split(","));
      lineStart = i + 1;
    }
    i++;
  }

  if (rows.length < maxRows && lineStart < text.length) {
    let line = text.slice(lineStart);
    if (line.length > 0 && line.charCodeAt(line.length - 1) === 13) {
      line = line.slice(0, -1);
    }
    if (line.length > 0) rows.push(line.split(","));
    return { rows, totalRows: rows.length };
  }

  let totalRows = rows.length;
  for (let j = i; j < text.length; j++) {
    if (text.charCodeAt(j) === 10) totalRows++;
  }
  if (text.length > 0 && text.charCodeAt(text.length - 1) !== 10) {
    totalRows++;
  }
  return { rows, totalRows };
}

function buildDataset(
  rows: string[][],
  totalRowsInText: number,
  byteTruncated: boolean
): Dataset {
  const headerRow = rows[0] ?? [];
  const columns: Column[] = headerRow.map((key, i) => ({ key: key || `col_${i}` }));
  const dataRows = rows.slice(1);
  // -1 to discount the header row from the data total.
  const totalDataInText = Math.max(0, totalRowsInText - 1);

  let truncation: Dataset["truncation"];
  if (byteTruncated) {
    // We didn't read the whole file. Real total is unknown.
    truncation = { totalRows: null };
  } else if (totalDataInText > dataRows.length) {
    // Whole file fit in the read budget but the row cap clipped display.
    truncation = { totalRows: totalDataInText };
  } else {
    truncation = null;
  }

  return { columns, rows: dataRows, parseErrors: [], truncation };
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export const csvViewer: ViewerDefinition = {
  kinds: ["csv"],
  icon: Sheet,
  Component: CsvView,
};
