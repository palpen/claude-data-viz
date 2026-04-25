import { useEffect, useRef } from "react";
import {
  TransformWrapper,
  TransformComponent,
  type ReactZoomPanPinchRef,
} from "react-zoom-pan-pinch";
import { tinykeys } from "tinykeys";
import { useVizStore } from "../store/vizStore";
import { convertFileSrc } from "../lib/tauri";
import { isImageKind } from "../lib/mime";
import { HtmlView } from "./HtmlView";
import { PdfView } from "./PdfView";
import { EmptyState } from "./EmptyState";
import { formatDistanceToNow } from "date-fns";
import { MessageSquare, ZoomIn, ZoomOut, Maximize2 } from "lucide-react";

export function Viewer() {
  const selectedId = useVizStore((s) => s.selectedId);
  const item = useVizStore((s) => (selectedId ? s.items[selectedId] : null));

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
      </div>
      <div className="flex-1 min-h-0 overflow-hidden">
        {renderBody(item)}
      </div>
    </div>
  );
}

type Item = ReturnType<typeof useVizStore.getState>["items"][string];

function renderBody(item: Item) {
  if (isImageKind(item.kind) || item.kind === "svg") {
    return <ImageView key={item.abs_path} item={item} />;
  }
  if (item.kind === "html") {
    return <HtmlView absPath={item.abs_path} size={item.size} mtime={item.mtime} />;
  }
  if (item.kind === "pdf") {
    return <PdfView absPath={item.abs_path} mtime={item.mtime} />;
  }
  return <EmptyState message={`Unsupported file type: ${item.kind}`} />;
}

function ImageView({ item }: { item: Item }) {
  const ref = useRef<ReactZoomPanPinchRef | null>(null);

  useEffect(() => {
    return tinykeys(window, {
      "0": (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        ref.current?.resetTransform();
      },
      "=": (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        ref.current?.zoomIn();
      },
      "+": (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        ref.current?.zoomIn();
      },
      "-": (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        ref.current?.zoomOut();
      },
    });
  }, []);

  return (
    <div className="relative w-full h-full">
      <TransformWrapper
        ref={ref}
        minScale={0.5}
        maxScale={20}
        centerOnInit
        limitToBounds={false}
        wheel={{ step: 0.05 }}
        pinch={{ step: 2 }}
        doubleClick={{ mode: "zoomIn", step: 1 }}
      >
        {({ zoomIn, zoomOut, resetTransform }) => (
          <>
            <div className="absolute top-2 right-2 z-10 flex gap-1">
              <ZoomButton onClick={() => zoomIn()} label="Zoom in (+)">
                <ZoomIn className="w-3.5 h-3.5" />
              </ZoomButton>
              <ZoomButton onClick={() => zoomOut()} label="Zoom out (-)">
                <ZoomOut className="w-3.5 h-3.5" />
              </ZoomButton>
              <ZoomButton onClick={() => resetTransform()} label="Reset zoom (0)">
                <Maximize2 className="w-3.5 h-3.5" />
              </ZoomButton>
            </div>
            <TransformComponent
              wrapperClass="!w-full !h-full"
              contentClass="!w-full !h-full flex items-center justify-center p-4"
            >
              <img
                src={`${convertFileSrc(item.abs_path)}?v=${item.mtime}`}
                alt={item.rel_path}
                className="max-w-full max-h-full object-contain select-none"
                draggable={false}
              />
            </TransformComponent>
          </>
        )}
      </TransformWrapper>
    </div>
  );
}

function ZoomButton({
  onClick,
  label,
  children,
}: {
  onClick: () => void;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-label={label}
      className="flex items-center justify-center w-7 h-7 rounded bg-[color:var(--color-surface-2)] border border-[color:var(--color-border)] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] hover:bg-[color:var(--color-surface)] transition-colors"
    >
      {children}
    </button>
  );
}

function isTypingTarget(target: EventTarget | null): boolean {
  if (!target || !(target instanceof HTMLElement)) return false;
  const t = target.tagName;
  return t === "INPUT" || t === "TEXTAREA" || target.isContentEditable;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}
