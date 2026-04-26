import { useEffect, useRef } from "react";
import {
  TransformWrapper,
  TransformComponent,
  type ReactZoomPanPinchRef,
} from "react-zoom-pan-pinch";
import { tinykeys } from "tinykeys";
import { FileImage, Maximize2, ZoomIn, ZoomOut } from "lucide-react";
import { convertFileSrc } from "../lib/tauri";
import type { VizItem } from "../types";
import type { ViewerDefinition } from "./registry";

function ImageView({ item }: { item: VizItem }) {
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
        key={item.abs_path}
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

export const imageViewer: ViewerDefinition = {
  kinds: ["png", "jpg", "webp", "gif", "svg"],
  icon: FileImage,
  label: (kind) => (kind === "jpg" ? "JPEG" : kind.toUpperCase()),
  Component: ImageView,
};
