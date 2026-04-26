import { FileText } from "lucide-react";
import { convertFileSrc } from "../lib/tauri";
import type { ViewerDefinition, ViewerProps } from "./registry";

function PdfView({ item, displayPath }: ViewerProps) {
  return (
    <embed
      src={`${convertFileSrc(displayPath)}?v=${item.mtime}`}
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
