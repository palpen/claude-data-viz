import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import type {
  InitialState,
  RecentRemote,
  RemoteDirListing,
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
  /** Empty / null → resolve to the user's home directory on the remote. */
  remote_path?: string | null;
  glob: string;
}

export interface AddRemoteWatchArgs {
  host: string;
  user?: string | null;
  port?: number | null;
  /** Empty / null → resolve to the user's home directory on the remote. */
  remote_path?: string | null;
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
  confirmUnknownHost: (host: string, port: number, expectedFingerprint: string) =>
    invoke<void>("confirm_unknown_host", {
      args: { host, port, expected_fingerprint: expectedFingerprint },
    }),
  addRemoteWatch: (args: AddRemoteWatchArgs) =>
    invoke<Watch>("add_remote_watch", { args }),
  fetchRemoteFile: (watchId: string, absPath: string) =>
    invoke<string>("fetch_remote_file", { watchId, absPath }),
  getWatchStatus: (watchId: string) =>
    invoke<WatchStatus | null>("get_watch_status", { watchId }),
  reconnectWatch: (watchId: string) =>
    invoke<void>("reconnect_watch", { watchId }),
  listRecentRemotes: () => invoke<RecentRemote[]>("list_recent_remotes"),
  forgetRecentRemote: (host: string, user: string, port: number, remotePath: string) =>
    invoke<void>("forget_recent_remote", { host, user, port, remotePath }),
  updateRemoteWatchPath: (watchId: string, newPath: string | null) =>
    invoke<Watch>("update_remote_watch_path", { watchId, newPath }),
  listRemoteDirs: (
    watchId: string,
    path: string | null,
    cursor?: string | null,
    limit?: number | null,
  ) =>
    invoke<RemoteDirListing>("list_remote_dirs", {
      watchId,
      path,
      cursor: cursor ?? null,
      limit: limit ?? null,
    }),
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
