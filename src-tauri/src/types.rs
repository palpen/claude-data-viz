use serde::{Deserialize, Serialize};
use std::path::Path;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../src/types/bindings/")]
pub enum VizKind {
    Png,
    Jpg,
    Webp,
    Gif,
    Svg,
    Html,
    Pdf,
    Csv,
}

impl VizKind {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpg),
            "webp" => Some(Self::Webp),
            "gif" => Some(Self::Gif),
            "svg" => Some(Self::Svg),
            "html" | "htm" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../src/types/bindings/")]
pub enum VizStatus {
    Active,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct VizItem {
    pub watch_id: String,
    pub abs_path: String,
    pub rel_path: String,
    pub kind: VizKind,
    #[ts(type = "number")]
    pub size: u64,
    #[ts(type = "number")]
    pub mtime: i64,
    pub prompt: Option<String>,
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub status: VizStatus,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct VizUpdated {
    pub watch_id: String,
    pub abs_path: String,
    #[ts(type = "number")]
    pub mtime: i64,
    #[ts(type = "number")]
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[allow(dead_code)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct VizEnriched {
    pub watch_id: String,
    pub abs_path: String,
    pub prompt: String,
    pub tool_use_id: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct VizGone {
    pub watch_id: String,
    pub abs_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[ts(export, export_to = "../../src/types/bindings/")]
pub enum WatchSource {
    Local {
        path: String,
    },
    Ssh {
        host: String,
        user: String,
        port: u16,
        remote_path: String,
        glob: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../src/types/bindings/")]
pub enum WatchStatus {
    Connected,
    Reconnecting {
        #[ts(type = "number")]
        since_ms: i64,
        last_error: Option<String>,
    },
    AuthFailed {
        last_error: String,
    },
    Unreachable {
        #[ts(type = "number")]
        since_ms: i64,
        last_error: String,
    },
    PathInvalid {
        last_error: String,
    },
    Stopped,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct WatchStatusEvent {
    pub watch_id: String,
    pub status: WatchStatus,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct SshHostEntry {
    pub alias: String,
    pub host_name: String,
    pub user: Option<String>,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct SshAgentProbe {
    pub available: bool,
    #[ts(type = "number")]
    pub key_count: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct TestStage {
    pub ok: bool,
    pub error: Option<String>,
    #[ts(type = "number | null")]
    pub matched_files: Option<u32>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct UnknownHostInfo {
    pub host: String,
    pub port: u16,
    /// SHA256:<base64> form, ready to display.
    pub fingerprint: String,
    /// e.g. "ssh-ed25519", "ecdsa-sha2-nistp256".
    pub key_type: String,
    /// The full OpenSSH-formatted public key line ("<algo> <base64> [comment]").
    pub raw_openssh: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct HostKeyChangedInfo {
    pub host: String,
    pub port: u16,
    pub fingerprint_offered: String,
    pub fingerprint_known: String,
    /// 1-indexed line number in the known_hosts file where the conflict was found.
    #[ts(type = "number")]
    pub known_hosts_line: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct TestResult {
    pub reachable: TestStage,
    pub authenticated: TestStage,
    pub path_exists: TestStage,
    pub matched: TestStage,
    /// Set when the host key isn't in known_hosts. Frontend should prompt the user to trust.
    pub unknown_host: Option<UnknownHostInfo>,
    /// Set when known_hosts already has a different (or revoked) key for this host. The
    /// frontend should NOT offer a trust path — only `ssh-keygen -R` advice.
    pub host_key_changed: Option<HostKeyChangedInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct Watch {
    pub id: String,
    pub source: WatchSource,
    pub session_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct InitialState {
    pub watches: Vec<Watch>,
    pub items: Vec<VizItem>,
    pub follow_latest: bool,
    pub selected: Option<(String, String)>,
    pub transcripts_dir: TranscriptsDirInfo,
}

/// Where the resolved local transcripts root came from. Drives the Settings dialog placeholder
/// label so users can see at a glance whether their `CLAUDE_CONFIG_DIR` is being honored.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../src/types/bindings/")]
pub enum TranscriptsDirSource {
    Override,
    Env,
    Default,
}

/// Snapshot of the local transcripts directory state. Sent to the frontend on hydrate and
/// after `set_claude_history_path` so the Settings dialog and the missing/empty banner
/// always reflect what the backend resolver actually picked.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct TranscriptsDirInfo {
    /// The user-set override from prefs.json, verbatim. None if no override.
    pub override_path: Option<String>,
    /// The absolute path the resolver picked after running the precedence chain.
    pub resolved_path: String,
    pub source: TranscriptsDirSource,
    /// Filesystem check: does `resolved_path` exist as a directory right now?
    pub exists: bool,
    /// True iff `resolved_path` exists AND contains at least one .jsonl file in any
    /// immediate subdirectory. Used to surface "configured but empty" as a banner.
    pub has_jsonl: bool,
}

/// One past remote connection. The dedup key is the full tuple — same host, different paths
/// stay as separate chips because users routinely watch multiple folders on one box.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct RecentRemote {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub remote_path: String,
    pub glob: String,
    #[ts(type = "number")]
    pub last_used_ms: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/types/bindings/")]
pub struct RemoteDirListing {
    pub current: String,
    pub parent: Option<String>,
    pub entries: Vec<String>,
    pub next_cursor: Option<String>,
    #[ts(type = "number | null")]
    pub total_estimate: Option<u64>,
}

