import { useMemo, useState } from "react";
import { useVizStore } from "../store/vizStore";
import { EmptyState } from "./EmptyState";
import { RemoteAssetGate } from "./RemoteAssetGate";
import { pickViewer } from "../viewers";
import { formatDistanceToNow } from "date-fns";
import { Check, Copy, MessageSquare } from "lucide-react";
import type { VizItem, Watch } from "../types";

export function Viewer() {
  const selectedId = useVizStore((s) => s.selectedId);
  const item = useVizStore((s) => (selectedId ? s.items[selectedId] : null));
  const watches = useVizStore((s) => s.watches);
  const watch = useMemo<Watch | null>(
    () => (item ? watches.find((w) => w.id === item.watch_id) ?? null : null),
    [item, watches],
  );

  if (!item) {
    return (
      <EmptyState
        message="No visualization selected"
        hint="When Claude Code writes an image into a watched folder, it'll appear here automatically."
      />
    );
  }

  return (
    <div className="h-full flex flex-col bg-[color:var(--color-bg)]">
      <div className="px-4 py-3 border-b border-[color:var(--color-border)] flex-shrink-0">
        {item.prompt ? (
          <div className="text-[14px] flex items-start gap-1.5 leading-snug">
            <MessageSquare className="w-3.5 h-3.5 text-[color:var(--color-accent)] flex-shrink-0 mt-1" />
            <span className="truncate">{item.prompt}</span>
          </div>
        ) : (
          <div className="text-[13px] italic text-[color:var(--color-text-dim)]">
            no prompt linked
          </div>
        )}
        <div className="text-[11px] text-[color:var(--color-text-dim)] flex gap-2 mt-1 ml-5">
          <span className="font-mono truncate">{item.rel_path}</span>
          <span>·</span>
          <span>{formatBytes(item.size)}</span>
          <span>·</span>
          <span>{formatDistanceToNow(item.mtime, { addSuffix: true })}</span>
        </div>
        <MoreInfo item={item} />
      </div>
      <div className="flex-1 min-h-0 overflow-hidden">{renderBody(item, watch)}</div>
    </div>
  );
}

function renderBody(item: VizItem, watch: Watch | null) {
  const def = pickViewer(item.kind);
  if (!def) {
    return <EmptyState message={`Unsupported file type: ${item.kind}`} />;
  }
  if (watch && watch.source.kind === "ssh") {
    return (
      <RemoteAssetGate watchId={item.watch_id} absPath={item.abs_path} version={item.mtime}>
        {(localPath) => <def.Component item={item} displayPath={localPath} />}
      </RemoteAssetGate>
    );
  }
  return <def.Component item={item} displayPath={item.abs_path} />;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function MoreInfo({ item }: { item: VizItem }) {
  const sessionId = item.session_id;
  const cwd = item.cwd;
  const transcriptsRoot = useVizStore((s) => s.transcriptsDir?.resolved_path ?? null);
  if (!sessionId && !cwd) return null;

  const resumeCmd = sessionId ? `claude --resume ${sessionId}` : null;
  const transcriptPath =
    sessionId && cwd && transcriptsRoot
      ? `${transcriptsRoot}/${cwd.replace(/\//g, "-")}/${sessionId}.jsonl`
      : null;

  return (
    <details className="mt-1.5 ml-5 text-[11px] text-[color:var(--color-text-dim)] group/details">
      <summary className="cursor-pointer hover:text-[color:var(--color-text)] select-none list-none flex items-center gap-1 marker:hidden">
        <span className="inline-block transition-transform group-open/details:rotate-90">▸</span>
        <span>More info</span>
      </summary>
      <dl className="mt-1.5 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 pl-3.5">
        {sessionId && (
          <>
            <dt className="text-[color:var(--color-text-dim)]">session</dt>
            <dd className="font-mono flex items-center gap-1.5 min-w-0">
              <span className="truncate">{sessionId}</span>
              <CopyButton text={sessionId} label="Copy session ID" />
            </dd>
          </>
        )}
        {cwd && (
          <>
            <dt>project</dt>
            <dd className="font-mono truncate" title={cwd}>{cwd}</dd>
          </>
        )}
        {transcriptPath && (
          <>
            <dt>transcript</dt>
            <dd className="font-mono truncate" title={transcriptPath}>{transcriptPath}</dd>
          </>
        )}
        {resumeCmd && (
          <>
            <dt>resume</dt>
            <dd className="font-mono flex items-center gap-1.5 min-w-0">
              <span className="truncate">{resumeCmd}</span>
              <CopyButton text={resumeCmd} label="Copy resume command" />
            </dd>
          </>
        )}
      </dl>
    </details>
  );
}

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={async (e) => {
        e.stopPropagation();
        try {
          await navigator.clipboard.writeText(text);
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1200);
        } catch {
          // noop — clipboard refusal in some webviews; UI just won't flip to "copied"
        }
      }}
      title={label}
      aria-label={label}
      className="flex-shrink-0 text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] transition-colors"
    >
      {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
    </button>
  );
}
