import type { VizKind } from "../types";

export const isImageKind = (k: VizKind): boolean =>
  k === "png" || k === "jpg" || k === "webp" || k === "gif";

export const kindLabel = (k: VizKind): string =>
  k === "jpg" ? "JPEG" : k.toUpperCase();
