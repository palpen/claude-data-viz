import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import {
  itemId,
  type ItemId,
  type VizEnriched,
  type VizEvicted,
  type VizGone,
  type VizItem,
  type VizUpdated,
  type Watch,
  type WatchStatus,
} from "../types";

interface State {
  items: Record<ItemId, VizItem>;
  order: ItemId[];
  watches: Watch[];
  watchStatus: Record<string, WatchStatus>;
  selectedId: ItemId | null;
  followLatest: boolean;
  hydrated: boolean;
}

interface Actions {
  hydrate: (s: { items: VizItem[]; watches: Watch[]; selected: [string, string] | null; followLatest: boolean }) => void;
  addItem: (item: VizItem) => void;
  updateItem: (u: VizUpdated) => void;
  enrichItem: (e: VizEnriched) => void;
  removeItem: (g: VizGone) => void;
  evictItem: (e: VizEvicted) => void;
  addWatch: (w: Watch) => void;
  removeWatch: (id: string) => void;
  setWatchStatus: (id: string, status: WatchStatus) => void;
  select: (id: ItemId | null) => void;
  toggleFollow: () => void;
  setFollow: (value: boolean) => void;
  jumpTo: (n: number) => void;
  clearGallery: () => void;
}

const sortByMtimeDesc = (items: Record<ItemId, VizItem>, ids: ItemId[]): ItemId[] => {
  return [...ids].sort((a, b) => (items[b]?.mtime ?? 0) - (items[a]?.mtime ?? 0));
};

export const useVizStore = create<State & Actions>()(
  subscribeWithSelector((set, get) => ({
    items: {},
    order: [],
    watches: [],
    watchStatus: {},
    selectedId: null,
    followLatest: true,
    hydrated: false,

    hydrate: ({ items, watches, selected, followLatest }) => {
      const map: Record<ItemId, VizItem> = {};
      for (const it of items) {
        map[itemId(it.watch_id, it.abs_path)] = it;
      }
      const order = sortByMtimeDesc(map, Object.keys(map));
      const sel = selected ? itemId(selected[0], selected[1]) : null;
      set({
        items: map,
        order,
        watches,
        selectedId: sel && map[sel] ? sel : order[0] ?? null,
        followLatest,
        hydrated: true,
      });
    },

    addItem: (item) => {
      const id = itemId(item.watch_id, item.abs_path);
      const exists = !!get().items[id];
      set((s) => {
        const items = { ...s.items, [id]: item };
        const order = exists ? s.order : [id, ...s.order];
        const next: Partial<State> = { items, order };
        if (s.followLatest) next.selectedId = id;
        else if (!s.selectedId) next.selectedId = id;
        return next;
      });
    },

    updateItem: (u) => {
      const id = itemId(u.watch_id, u.abs_path);
      set((s) => {
        const existing = s.items[id];
        if (!existing) return {};
        const updated: VizItem = { ...existing, mtime: u.mtime, size: u.size };
        const items = { ...s.items, [id]: updated };
        const order = sortByMtimeDesc(items, s.order);
        const next: Partial<State> = { items, order };
        if (s.followLatest) next.selectedId = id;
        return next;
      });
    },

    enrichItem: (e) => {
      const id = itemId(e.watch_id, e.abs_path);
      set((s) => {
        const existing = s.items[id];
        if (!existing) return {};
        return {
          items: {
            ...s.items,
            [id]: {
              ...existing,
              prompt: e.prompt,
              tool_use_id: e.tool_use_id ?? null,
              session_id: e.session_id ?? null,
              cwd: e.cwd ?? null,
            },
          },
        };
      });
    },

    removeItem: (g) => {
      const id = itemId(g.watch_id, g.abs_path);
      set((s) => {
        const existing = s.items[id];
        if (!existing) return {};
        return {
          items: { ...s.items, [id]: { ...existing, status: "deleted" } },
        };
      });
    },

    evictItem: (e) => {
      const id = itemId(e.watch_id, e.abs_path);
      set((s) => {
        if (!s.items[id]) return {};
        const { [id]: _dropped, ...rest } = s.items;
        const order = s.order.filter((k) => k !== id);
        const selectedId = s.selectedId === id ? (order[0] ?? null) : s.selectedId;
        return { items: rest, order, selectedId };
      });
    },

    addWatch: (w) => set((s) => ({ watches: [...s.watches, w] })),
    removeWatch: (id) =>
      set((s) => {
        const { [id]: _dropped, ...remainingStatus } = s.watchStatus;
        return {
          watches: s.watches.filter((w) => w.id !== id),
          watchStatus: remainingStatus,
          items: Object.fromEntries(
            Object.entries(s.items).filter(([_, it]) => it.watch_id !== id),
          ),
          order: s.order.filter((k) => !k.startsWith(`${id}::`)),
        };
      }),
    setWatchStatus: (id, status) =>
      set((s) => ({ watchStatus: { ...s.watchStatus, [id]: status } })),

    select: (id) => set({ selectedId: id, followLatest: false }),

    toggleFollow: () =>
      set((s) => {
        const next = !s.followLatest;
        const selectedId = next && s.order[0] ? s.order[0] : s.selectedId;
        return { followLatest: next, selectedId };
      }),

    setFollow: (value) => set({ followLatest: value }),

    jumpTo: (n) => {
      const { order } = get();
      const id = order[n - 1];
      if (id) set({ selectedId: id, followLatest: false });
    },

    clearGallery: () => set({ items: {}, order: [], selectedId: null }),
  }))
);
