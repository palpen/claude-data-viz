import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { FolderOpen, Server, Sparkles } from "lucide-react";
import { tauri } from "../lib/tauri";
import { useVizStore } from "../store/vizStore";

export function FirstRunPicker() {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const addWatch = useVizStore((s) => s.addWatch);

  const pickLocal = async () => {
    setErr(null);
    const path = await open({
      directory: true,
      multiple: false,
      title: "Pick a folder to watch",
    });
    if (!path || typeof path !== "string") return;
    setBusy(true);
    try {
      const watch = await tauri.addLocalWatch(path);
      addWatch(watch);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="h-full flex items-center justify-center">
      <div className="max-w-lg w-full px-8">
        <div className="flex items-center gap-2 mb-2 text-[color:var(--color-accent)]">
          <Sparkles className="w-5 h-5" />
          <span className="font-semibold tracking-wide">Claude Data Viz</span>
        </div>
        <h1 className="text-2xl font-semibold mb-2">Live viewer for Claude Code visualizations.</h1>
        <p className="text-sm text-[color:var(--color-text-dim)] mb-8 leading-relaxed">
          Point at a folder you're working in. As Claude Code writes plots, dashboards, and PDFs,
          they'll appear here — labeled with the prompt that asked for them.
        </p>
        <div className="grid grid-cols-1 gap-3">
          <button
            onClick={pickLocal}
            disabled={busy}
            className="flex items-start gap-3 p-4 rounded-lg border border-[color:var(--color-border)] bg-[color:var(--color-surface)] hover:border-[color:var(--color-accent)]/60 hover:bg-[color:var(--color-surface-2)] transition-colors text-left disabled:opacity-50"
          >
            <FolderOpen className="w-5 h-5 mt-0.5 text-[color:var(--color-accent)]" />
            <div>
              <div className="font-medium">Watch a local folder</div>
              <div className="text-xs text-[color:var(--color-text-dim)] mt-0.5">
                Anything Claude writes inside it shows up here.
              </div>
            </div>
          </button>
          <button
            disabled
            title="Coming soon"
            className="flex items-start gap-3 p-4 rounded-lg border border-[color:var(--color-border)] bg-[color:var(--color-surface)] opacity-50 cursor-not-allowed text-left"
          >
            <Server className="w-5 h-5 mt-0.5 text-[color:var(--color-text-dim)]" />
            <div>
              <div className="font-medium flex items-center gap-2">
                Connect to a remote server
                <span className="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-[color:var(--color-surface-2)] border border-[color:var(--color-border)]">
                  Coming soon
                </span>
              </div>
              <div className="text-xs text-[color:var(--color-text-dim)] mt-0.5">
                Stream plots from any SSH host without copying files locally.
              </div>
            </div>
          </button>
        </div>
        {err && <div className="mt-4 text-sm text-red-300">{err}</div>}
      </div>
    </div>
  );
}
