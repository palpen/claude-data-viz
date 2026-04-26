import { useEffect, useState } from "react";
import { Loader2, AlertTriangle, RotateCcw } from "lucide-react";
import { tauri } from "../lib/tauri";

type Entry =
  | { kind: "pending"; promise: Promise<string> }
  | { kind: "resolved"; localPath: string }
  | { kind: "rejected"; error: string };

const cache = new Map<string, Entry>();

function cacheKey(watchId: string, path: string): string {
  return `${watchId}::${path}`;
}

/// Trigger a fetch (or noop on hit). Used for eager prefetch from outside the gate, e.g.
/// when viz:new fires for a remote watch and followLatest is on.
export function prefetchRemoteFile(watchId: string, absPath: string): void {
  const key = cacheKey(watchId, absPath);
  const existing = cache.get(key);
  if (existing && existing.kind !== "rejected") return;
  startFetch(watchId, absPath);
}

function startFetch(watchId: string, absPath: string): Promise<string> {
  const key = cacheKey(watchId, absPath);
  const promise = tauri
    .fetchRemoteFile(watchId, absPath)
    .then((local) => {
      cache.set(key, { kind: "resolved", localPath: local });
      return local;
    })
    .catch((e) => {
      const msg = String(e);
      cache.set(key, { kind: "rejected", error: msg });
      throw e;
    });
  cache.set(key, { kind: "pending", promise });
  return promise;
}

export function invalidateRemoteAsset(watchId: string, absPath: string): void {
  cache.delete(cacheKey(watchId, absPath));
}

/// Read the cached local path for a remote asset, kicking off a fetch on miss. Returns null
/// while pending or on error — caller decides the fallback (e.g., icon for thumbnails).
/// Pass `enabled=false` for non-SSH items so the hook is a no-op.
export function useRemoteAsset(
  watchId: string,
  absPath: string,
  enabled: boolean,
): string | null {
  const [, setTick] = useState(0);

  if (!enabled) return null;

  const key = cacheKey(watchId, absPath);
  const entry = cache.get(key);

  if (!entry) {
    startFetch(watchId, absPath).then(
      () => setTick((n) => n + 1),
      () => setTick((n) => n + 1),
    );
    return null;
  }
  if (entry.kind === "resolved") return entry.localPath;
  if (entry.kind === "pending") {
    entry.promise.then(
      () => setTick((n) => n + 1),
      () => setTick((n) => n + 1),
    );
    return null;
  }
  return null;
}

export interface RemoteAssetGateProps {
  watchId: string;
  absPath: string;
  /// Re-fetch when this changes (typically `mtime`). Cache hits stay sync — no flash on
  /// re-renders that don't change the version key.
  version: number | string;
  children: (localPath: string) => React.ReactNode;
}

export function RemoteAssetGate({ watchId, absPath, version, children }: RemoteAssetGateProps) {
  const [tick, setTick] = useState(0);

  // Invalidate cache entry when `version` changes (e.g., remote mtime advanced — viz:updated).
  useEffect(() => {
    invalidateRemoteAsset(watchId, absPath);
    setTick((n) => n + 1);
  }, [watchId, absPath, version]);

  const key = cacheKey(watchId, absPath);
  const entry = cache.get(key);

  if (!entry) {
    // Kick off a fetch and re-render when it completes.
    startFetch(watchId, absPath).then(
      () => setTick((n) => n + 1),
      () => setTick((n) => n + 1),
    );
    return <Loading />;
  }

  if (entry.kind === "pending") {
    entry.promise.then(
      () => setTick((n) => n + 1),
      () => setTick((n) => n + 1),
    );
    return <Loading />;
  }

  if (entry.kind === "rejected") {
    return (
      <RemoteFetchError
        error={entry.error}
        onRetry={() => {
          cache.delete(key);
          setTick((n) => n + 1);
        }}
      />
    );
  }

  // resolved
  void tick; // include in deps so re-render triggers consume the new state
  return <>{children(entry.localPath)}</>;
}

function Loading() {
  return (
    <div className="w-full h-full flex items-center justify-center text-[color:var(--color-text-dim)]">
      <div className="flex items-center gap-2 text-[13px]">
        <Loader2 className="w-4 h-4 animate-spin" />
        Loading from remote…
      </div>
    </div>
  );
}

function RemoteFetchError({ error, onRetry }: { error: string; onRetry: () => void }) {
  return (
    <div className="w-full h-full flex items-center justify-center px-6">
      <div className="max-w-md text-center">
        <AlertTriangle className="w-6 h-6 mx-auto mb-2 text-red-300" />
        <div className="text-[13px] text-red-300/90 mb-2">Couldn't fetch remote file</div>
        <div className="text-[12px] text-[color:var(--color-text-dim)] font-mono break-words mb-4">
          {error}
        </div>
        <button
          type="button"
          onClick={onRetry}
          className="px-3 py-1.5 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] hover:border-[color:var(--color-accent)]/60 text-[12px] flex items-center gap-1.5 mx-auto"
        >
          <RotateCcw className="w-3.5 h-3.5" />
          Retry
        </button>
      </div>
    </div>
  );
}
