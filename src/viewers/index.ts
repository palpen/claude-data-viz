import type { LucideIcon } from "lucide-react";
import { FileQuestion } from "lucide-react";
import type { VizKind } from "../types";
import type { ViewerDefinition } from "./registry";
import { imageViewer } from "./ImageViewer";
import { htmlViewer } from "./HtmlViewer";
import { pdfViewer } from "./PdfViewer";
import { csvViewer } from "./CsvViewer";

export type { ViewerDefinition } from "./registry";

const VIEWERS: ViewerDefinition[] = [imageViewer, htmlViewer, pdfViewer, csvViewer];

export function pickViewer(kind: VizKind): ViewerDefinition | null {
  return VIEWERS.find((v) => v.kinds.includes(kind)) ?? null;
}

export function iconForKind(kind: VizKind): LucideIcon {
  return pickViewer(kind)?.icon ?? FileQuestion;
}

export function labelForKind(kind: VizKind): string {
  const def = pickViewer(kind);
  return def?.label?.(kind) ?? kind.toUpperCase();
}

const IMAGE_PREVIEW_KINDS: ReadonlySet<VizKind> = new Set(imageViewer.kinds);

export function rendersInlineImagePreview(kind: VizKind): boolean {
  return IMAGE_PREVIEW_KINDS.has(kind);
}
