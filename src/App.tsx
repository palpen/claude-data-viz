import { useEffect } from "react";
import { events, tauri } from "./lib/tauri";
import { useVizStore } from "./store/vizStore";
import { Sidebar } from "./components/Sidebar";
import { Viewer } from "./components/Viewer";
import { TopBar } from "./components/TopBar";
import { TranscriptsDirBanner } from "./components/TranscriptsDirBanner";
import { FirstRunPicker } from "./components/FirstRunPicker";
import { prefetchRemoteFile, invalidateRemoteAsset } from "./components/RemoteAssetGate";
import { useHotkeys } from "./lib/hotkeys";
import { swallowWithLog } from "./lib/log";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

export default function App() {
  const hydrated = useVizStore((s) => s.hydrated);
  const watches = useVizStore((s) => s.watches);
  const hydrate = useVizStore((s) => s.hydrate);
  const addItem = useVizStore((s) => s.addItem);
  const updateItem = useVizStore((s) => s.updateItem);
  const enrichItem = useVizStore((s) => s.enrichItem);
  const removeItem = useVizStore((s) => s.removeItem);
  const evictItem = useVizStore((s) => s.evictItem);
  const setWatchStatus = useVizStore((s) => s.setWatchStatus);
  const jumpTo = useVizStore((s) => s.jumpTo);
  const toggleFollow = useVizStore((s) => s.toggleFollow);

  useEffect(() => {
    let unlistens: Array<() => void> = [];
    (async () => {
      const initial = await tauri.getState();
      hydrate({
        items: initial.items,
        watches: initial.watches,
        selected: initial.selected,
        followLatest: initial.follow_latest,
        transcriptsDir: initial.transcripts_dir,
      });
      unlistens = await Promise.all([
        events.onVizNew((item) => {
          addItem(item);
          maybePrefetch(item.watch_id, item.abs_path);
        }),
        events.onVizUpdated((u) => {
          updateItem(u);
          // Remote file changed: drop cached copy so next render re-fetches.
          invalidateRemoteAsset(u.watch_id, u.abs_path);
          maybePrefetch(u.watch_id, u.abs_path);
        }),
        events.onVizEnriched(enrichItem),
        events.onVizGone((g) => {
          removeItem(g);
          invalidateRemoteAsset(g.watch_id, g.abs_path);
        }),
        events.onVizEvicted(evictItem),
        events.onWatchStatus((e) => setWatchStatus(e.watch_id, e.status)),
      ]);
    })();
    return () => {
      for (const u of unlistens) u();
    };
  }, [
    hydrate,
    addItem,
    updateItem,
    enrichItem,
    removeItem,
    evictItem,
    setWatchStatus,
  ]);

  function maybePrefetch(watchId: string, absPath: string) {
    const s = useVizStore.getState();
    if (!s.followLatest) return;
    const watch = s.watches.find((w) => w.id === watchId);
    if (watch?.source.kind === "ssh") {
      prefetchRemoteFile(watchId, absPath);
    }
  }

  useHotkeys({
    onJumpTo: jumpTo,
    onToggleFollow: () => {
      toggleFollow();
      const next = useVizStore.getState().followLatest;
      tauri.setFollowLatest(next).catch(swallowWithLog("App: hotkey toggle setFollowLatest"));
    },
    onToggleFullscreen: () => {
      // Future: actual fullscreen mode. For now: scroll selected into view.
    },
    onRevealOnDisk: () => {
      const { selectedId, items } = useVizStore.getState();
      const item = selectedId ? items[selectedId] : null;
      if (item) revealItemInDir(item.abs_path).catch(swallowWithLog("App: hotkey revealItemInDir"));
    },
  });

  if (!hydrated) {
    return (
      <div className="h-screen flex items-center justify-center text-sm opacity-60">
        Loading…
      </div>
    );
  }

  if (watches.length === 0) {
    return <FirstRunPicker />;
  }

  return (
    <div className="h-screen flex flex-col overflow-hidden">
      <TopBar />
      <TranscriptsDirBanner />
      <div className="flex-1 min-h-0 grid grid-rows-1 grid-cols-[300px_minmax(0,1fr)] overflow-hidden">
        <Sidebar />
        <Viewer />
      </div>
    </div>
  );
}
