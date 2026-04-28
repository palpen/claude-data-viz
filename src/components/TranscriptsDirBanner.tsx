import { useState } from "react";
import { AlertTriangle, X } from "lucide-react";
import { useVizStore } from "../store/vizStore";
import { SettingsDialog } from "./SettingsDialog";

/// Surfaces the silent-failure mode where the resolved Claude Code transcripts directory is
/// missing on disk or contains no .jsonl files. Without this, a user who set
/// `CLAUDE_CONFIG_DIR` to a non-existent path (or has no Claude sessions yet) just sees
/// "no prompt linked" forever and never finds Settings.
export function TranscriptsDirBanner() {
  const info = useVizStore((s) => s.transcriptsDir);
  const [dismissed, setDismissed] = useState(false);
  const [showSettings, setShowSettings] = useState(false);

  if (!info || dismissed) return null;
  if (info.exists && info.has_jsonl) return null;

  const message = info.exists
    ? `No Claude Code session transcripts found in ${info.resolved_path}.`
    : `Claude Code transcripts directory does not exist: ${info.resolved_path}.`;

  return (
    <>
      <div className="flex items-center gap-2 px-3 py-2 border-b border-amber-500/40 bg-amber-500/10 text-[12px]">
        <AlertTriangle className="w-3.5 h-3.5 text-amber-300 flex-shrink-0" />
        <span className="flex-1 truncate text-amber-100/90" title={message}>
          {message}{" "}
          <span className="text-[color:var(--color-text-dim)]">
            Prompt attribution will not work.
          </span>
        </span>
        <button
          type="button"
          onClick={() => setShowSettings(true)}
          className="px-2 py-0.5 rounded text-[11px] font-medium bg-amber-500/80 text-black hover:bg-amber-400 flex-shrink-0"
        >
          Open Settings
        </button>
        <button
          type="button"
          onClick={() => setDismissed(true)}
          aria-label="Dismiss"
          className="text-amber-200/70 hover:text-amber-100 flex-shrink-0"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      </div>
      {showSettings && <SettingsDialog onClose={() => setShowSettings(false)} />}
    </>
  );
}
