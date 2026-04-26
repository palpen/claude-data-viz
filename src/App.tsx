import { useEffect } from "react";
import { events, tauri } from "./lib/tauri";
import { useVizStore } from "./store/vizStore";
import { Sidebar } from "./components/Sidebar";
import { Viewer } from "./components/Viewer";
import { TopBar } from "./components/TopBar";
import { FirstRunPicker } from "./components/FirstRunPicker";
import { useHotkeys } from "./lib/hotkeys";
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
      });
      unlistens = await Promise.all([
        events.onVizNew(addItem),
        events.onVizUpdated(updateItem),
        events.onVizEnriched(enrichItem),
        events.onVizGone(removeItem),
        events.onVizEvicted(evictItem),
      ]);
    })();
    return () => {
      for (const u of unlistens) u();
    };
  }, [hydrate, addItem, updateItem, enrichItem, removeItem, evictItem]);

  useHotkeys({
    onJumpTo: jumpTo,
    onToggleFollow: () => {
      toggleFollow();
      const next = useVizStore.getState().followLatest;
      tauri.setFollowLatest(next).catch(() => {});
    },
    onToggleFullscreen: () => {
      // Future: actual fullscreen mode. For now: scroll selected into view.
    },
    onRevealOnDisk: () => {
      const { selectedId, items } = useVizStore.getState();
      const item = selectedId ? items[selectedId] : null;
      if (item) revealItemInDir(item.abs_path).catch(() => {});
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
      <div className="flex-1 min-h-0 grid grid-rows-1 grid-cols-[300px_1fr] overflow-hidden">
        <Sidebar />
        <Viewer />
      </div>
    </div>
  );
}
