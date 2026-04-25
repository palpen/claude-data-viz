import { convertFileSrc } from "../lib/tauri";

export function PdfView({ absPath, mtime }: { absPath: string; mtime: number }) {
  return (
    <embed
      src={`${convertFileSrc(absPath)}?v=${mtime}`}
      type="application/pdf"
      className="w-full h-full bg-white"
    />
  );
}
