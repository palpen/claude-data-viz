import { useEffect, useState } from "react";
import { readTextFile } from "@tauri-apps/plugin-fs";
import { Code2 } from "lucide-react";
import { convertFileSrc } from "../lib/tauri";
import type { ViewerDefinition, ViewerProps } from "./registry";

const INLINE_LIMIT = 10 * 1024 * 1024; // 10 MB

function HtmlView({ item, displayPath }: ViewerProps) {
  const { size, mtime } = item;
  const absPath = displayPath;
  const [srcDoc, setSrcDoc] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (size > INLINE_LIMIT) {
      setSrcDoc(null);
      setErr(null);
      return;
    }
    let cancelled = false;
    setErr(null);
    setSrcDoc(null);
    readTextFile(absPath)
      .then((text) => {
        if (!cancelled) setSrcDoc(text);
      })
      .catch((e) => {
        if (!cancelled) setErr(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [absPath, size, mtime]);

  if (err) {
    return (
      <div className="p-6 text-sm text-red-300">
        Could not read HTML file: {err}
      </div>
    );
  }

  if (size > INLINE_LIMIT) {
    return (
      <iframe
        title="viz-html"
        src={`${convertFileSrc(absPath)}?v=${mtime}`}
        sandbox="allow-scripts"
        className="w-full h-full bg-white"
      />
    );
  }

  if (srcDoc === null) {
    return <div className="p-6 text-sm opacity-60">Loading…</div>;
  }

  return (
    <iframe
      title="viz-html"
      srcDoc={srcDoc}
      sandbox="allow-scripts"
      className="w-full h-full bg-white"
    />
  );
}

export const htmlViewer: ViewerDefinition = {
  kinds: ["html"],
  icon: Code2,
  Component: HtmlView,
};
