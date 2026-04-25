import { useEffect, useState } from "react";
import { readTextFile } from "@tauri-apps/plugin-fs";
import { convertFileSrc } from "../lib/tauri";

const INLINE_LIMIT = 10 * 1024 * 1024; // 10 MB

export function HtmlView({ absPath, size, mtime }: { absPath: string; size: number; mtime: number }) {
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
        sandbox="allow-scripts allow-same-origin"
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
      sandbox="allow-scripts allow-same-origin"
      className="w-full h-full bg-white"
    />
  );
}
