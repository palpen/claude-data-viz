import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import type {
  InitialState,
  SshAgentProbe,
  SshHostEntry,
  TestResult,
  VizEnriched,
  VizEvicted,
  VizGone,
  VizItem,
  VizUpdated,
  Watch,
  WatchStatus,
  WatchStatusEvent,
} from "../types";

export interface TestSshArgs {
  host: string;
  user?: string | null;
  port?: number | null;
  remote_path: string;
  glob: string;
}

export interface AddRemoteWatchArgs {
  host: string;
  user?: string | null;
  port?: number | null;
  remote_path: string;
  glob: string;
}

export const tauri = {
  getState: () => invoke<InitialState>("get_state"),
  addLocalWatch: (path: string, sessionPath?: string) =>
    invoke<Watch>("add_local_watch", {
      args: { path, session_path: sessionPath ?? null },
    }),
  removeWatch: (watchId: string) =>
    invoke<void>("remove_watch", { watchId }),
  rescanWatch: (watchId: string) =>
    invoke<void>("rescan_watch", { watchId }),
  setFollowLatest: (value: boolean) =>
    invoke<void>("set_follow_latest", { value }),
  setSelected: (watchId: string | null, absPath: string | null) =>
    invoke<void>("set_selected", { watchId, absPath }),
  clearGallery: () => invoke<void>("clear_gallery"),

  // SSH
  probeSshAgent: () => invoke<SshAgentProbe>("probe_ssh_agent"),
  listSshHosts: () => invoke<SshHostEntry[]>("list_ssh_hosts"),
  testSshConnection: (args: TestSshArgs) =>
    invoke<TestResult>("test_ssh_connection", { args }),
  addRemoteWatch: (args: AddRemoteWatchArgs) =>
    invoke<Watch>("add_remote_watch", { args }),
  fetchRemoteFile: (watchId: string, absPath: string) =>
    invoke<string>("fetch_remote_file", { watchId, absPath }),
  getWatchStatus: (watchId: string) =>
    invoke<WatchStatus | null>("get_watch_status", { watchId }),
  reconnectWatch: (watchId: string) =>
    invoke<void>("reconnect_watch", { watchId }),
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
  onWatchStatus: (cb: (e: WatchStatusEvent) => void): Promise<UnlistenFn> =>
    listen<WatchStatusEvent>("viz:watch_status", (e) => cb(e.payload)),
};

export { convertFileSrc };
