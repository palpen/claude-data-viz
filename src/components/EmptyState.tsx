import { ImageOff } from "lucide-react";

export function EmptyState({ message, hint }: { message: string; hint?: string }) {
  return (
    <div className="flex flex-col items-center justify-center h-full text-center p-8">
      <ImageOff className="w-12 h-12 mb-4 opacity-30" />
      <div className="text-base text-[color:var(--color-text)]">{message}</div>
      {hint && (
        <div className="text-xs mt-2 text-[color:var(--color-text-dim)] max-w-md">
          {hint}
        </div>
      )}
    </div>
  );
}
