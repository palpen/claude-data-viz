import { Zap, ZapOff } from "lucide-react";
import { clsx } from "clsx";

export function FollowToggle({
  active,
  onToggle,
}: {
  active: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      onClick={onToggle}
      title={active ? "Following latest (press F)" : "Paused (press F)"}
      className={clsx(
        "flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium border transition-colors",
        active
          ? "bg-[color:var(--color-accent)]/15 border-[color:var(--color-accent)]/40 text-[color:var(--color-accent)]"
          : "bg-[color:var(--color-surface-2)] border-[color:var(--color-border)] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)]"
      )}
    >
      {active ? <Zap className="w-3 h-3" /> : <ZapOff className="w-3 h-3" />}
      {active ? "Following" : "Paused"}
    </button>
  );
}
