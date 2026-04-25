import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import type {
  InitialState,
  VizEnriched,
  VizEvicted,
  VizGone,
  VizItem,
  VizUpdated,
  Watch,
} from "../types";

export const tauri = {
  getState: () => invoke<InitialState>("get_state"),
  addLocalWatch: (path: string, sessionPath?: string) =>
    invoke<Watch>("add_local_watch", {
      args: { path, session_path: sessionPath ?? null },
    }),
  removeWatch: (watchId: string) =>
    invoke<void>("remove_watch", { watchId }),
  setFollowLatest: (value: boolean) =>
    invoke<void>("set_follow_latest", { value }),
  setSelected: (watchId: string | null, absPath: string | null) =>
    invoke<void>("set_selected", { watchId, absPath }),
  clearGallery: () => invoke<void>("clear_gallery"),
};

export const events = {
  onVizNew: (cb: (item: VizItem) => void): Promise<UnlistenFn> =>
    listen<VizItem>("viz:new", (e) => cb(e.payload)),
  onVizUpdated: (cb: (u: VizUpdated) => void): Promise<UnlistenFn> =>
    listen<VizUpdated>("viz:updated", (e) => cb(e.payload)),
  onVizEnriched: (cb: (e: VizEnriched) => void): Promise<UnlistenFn> =>
    listen<VizEnriched>("viz:enriched", (e) => cb(e.payload)),
  onVizGone: (cb: (g: VizGone) => void): Promise<UnlistenFn> =>
    listen<VizGone>("viz:gone", (e) => cb(e.payload)),
  onVizEvicted: (cb: (e: VizEvicted) => void): Promise<UnlistenFn> =>
    listen<VizEvicted>("viz:evicted", (e) => cb(e.payload)),
};

export { convertFileSrc };
