import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle,
  FolderOpen,
  Loader2,
  RotateCcw,
  Settings as SettingsIcon,
  X,
} from "lucide-react";
import { tauri } from "../lib/tauri";
import { useVizStore } from "../store/vizStore";
import type { TranscriptsDirInfo } from "../types";

interface Props {
  onClose: () => void;
}

export function SettingsDialog({ onClose }: Props) {
  const transcriptsDir = useVizStore((s) => s.transcriptsDir);
  const setTranscriptsDir = useVizStore((s) => s.setTranscriptsDir);

  const [draft, setDraft] = useState(transcriptsDir?.override_path ?? "");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const onBrowse = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Pick the Claude Code transcripts directory",
    });
    if (typeof picked === "string") setDraft(picked);
  };

  const apply = async (next: string | null) => {
    setErr(null);
    setBusy(true);
    try {
      const info = await tauri.setClaudeHistoryPath(next);
      setTranscriptsDir(info);
      onClose();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onSave = () => {
    const trimmed = draft.trim();
    apply(trimmed.length === 0 ? null : trimmed);
  };

  const onReset = () => {
    setDraft("");
    apply(null);
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl rounded-lg border border-[color:var(--color-border)] bg-[color:var(--color-bg)] shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-[color:var(--color-border)]">
          <div className="flex items-center gap-2">
            <SettingsIcon className="w-4 h-4 text-[color:var(--color-accent)]" />
            <span className="text-[14px] font-semibold">Settings</span>
          </div>
          <button
            onClick={onClose}
            className="text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)]"
            aria-label="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="px-5 py-4 space-y-4">
          <div>
            <div className="flex items-baseline justify-between">
              <label className="text-[11px] uppercase tracking-wider text-[color:var(--color-text-dim)]">
                Claude Code transcripts directory
              </label>
              <ResolvedHint info={transcriptsDir} />
            </div>
            <p className="mt-1 text-[11px] text-[color:var(--color-text-dim)] leading-snug">
              Where this app reads Claude Code session JSONLs from to attribute prompts to
              files. Leave empty to fall back to{" "}
              <code className="font-mono">$CLAUDE_CONFIG_DIR/projects</code> (if set), then{" "}
              <code className="font-mono">~/.claude/projects</code>.
            </p>
            <div className="mt-2 flex items-stretch gap-2">
              <input
                type="text"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                placeholder={placeholderFor(transcriptsDir)}
                className="flex-1 px-3 py-2 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] text-[13px] font-mono placeholder:opacity-40 focus:outline-none focus:border-[color:var(--color-accent)]/60"
              />
              <button
                type="button"
                onClick={onBrowse}
                disabled={busy}
                className="flex items-center gap-1.5 px-3 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] hover:border-[color:var(--color-accent)]/60 text-[12px] disabled:opacity-50"
              >
                <FolderOpen className="w-3.5 h-3.5" />
                Browse…
              </button>
            </div>
            <p className="mt-2 text-[11px] text-[color:var(--color-text-dim)] leading-snug">
              Changes apply within ~2s. Files already being tailed under the previous path
              continue until they go idle (~1h) — restart the app for an immediate cutover.
            </p>
          </div>

          {err && (
            <div className="text-[12px] text-red-300 flex items-start gap-1.5">
              <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
              <span className="whitespace-pre-wrap break-words">{err}</span>
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-2 px-5 py-3 border-t border-[color:var(--color-border)]">
          <button
            type="button"
            onClick={onReset}
            disabled={busy || (transcriptsDir?.override_path == null && draft.trim() === "")}
            className="flex items-center gap-1.5 px-2.5 py-1.5 rounded text-[12px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] disabled:opacity-30 disabled:cursor-not-allowed"
            title="Clear the override and use the precedence default"
          >
            <RotateCcw className="w-3.5 h-3.5" />
            Reset to default
          </button>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onClose}
              disabled={busy}
              className="px-3 py-1.5 rounded text-[12px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] disabled:opacity-50"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={onSave}
              disabled={busy}
              className="px-3 py-1.5 rounded text-[12px] font-medium bg-[color:var(--color-accent)] text-black hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
            >
              {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
              Save
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function placeholderFor(info: TranscriptsDirInfo | null): string {
  if (!info) return "(loading…)";
  // Show the resolved path (post-precedence) so the user sees what the empty-input default
  // would land on right now — env var, home, etc.
  return info.resolved_path;
}

function ResolvedHint({ info }: { info: TranscriptsDirInfo | null }) {
  if (!info) return null;
  const label = (() => {
    switch (info.source) {
      case "override":
        return "Using override";
      case "env":
        return "From CLAUDE_CONFIG_DIR";
      case "default":
        return "Default";
    }
  })();
  return (
    <span
      className="text-[10px] text-[color:var(--color-text-dim)] truncate max-w-[300px]"
      title={info.resolved_path}
    >
      {label}: <span className="font-mono">{info.resolved_path}</span>
    </span>
  );
}
