import { FileText } from "lucide-react";
import { convertFileSrc } from "../lib/tauri";
import type { VizItem } from "../types";
import type { ViewerDefinition } from "./registry";

function PdfView({ item }: { item: VizItem }) {
  return (
    <embed
      src={`${convertFileSrc(item.abs_path)}?v=${item.mtime}`}
      type="application/pdf"
      className="w-full h-full bg-white"
    />
  );
}

export const pdfViewer: ViewerDefinition = {
  kinds: ["pdf"],
  icon: FileText,
  Component: PdfView,
};
