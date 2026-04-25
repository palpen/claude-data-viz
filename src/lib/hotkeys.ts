import { tinykeys } from "tinykeys";
import { useEffect } from "react";

interface Handlers {
  onJumpTo: (n: number) => void;
  onToggleFollow: () => void;
  onToggleFullscreen: () => void;
  onRevealOnDisk: () => void;
}

export function useHotkeys(handlers: Handlers) {
  useEffect(() => {
    const bindings: Record<string, (e: KeyboardEvent) => void> = {
      f: (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        handlers.onToggleFollow();
      },
      Space: (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        handlers.onToggleFullscreen();
      },
      "$mod+o": (e) => {
        e.preventDefault();
        handlers.onRevealOnDisk();
      },
    };
    for (let i = 1; i <= 9; i++) {
      bindings[String(i)] = (e) => {
        if (isTypingTarget(e.target)) return;
        e.preventDefault();
        handlers.onJumpTo(i);
      };
    }
    return tinykeys(window, bindings);
  }, [handlers]);
}

function isTypingTarget(target: EventTarget | null): boolean {
  if (!target || !(target instanceof HTMLElement)) return false;
  const t = target.tagName;
  return t === "INPUT" || t === "TEXTAREA" || target.isContentEditable;
}
