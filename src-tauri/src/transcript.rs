use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncSeekExt;

use crate::state::{mark_history_dirty, AppState};
use crate::types::VizEnriched;

const TOOL_RING_CAP: usize = 64;
const POLL_INTERVAL_MS: u64 = 500;
const RESCAN_INTERVAL_TICKS: u32 = 4; // rescan dir tree every 4 ticks (~2s) — picks up new sessions fast
const MATCH_WINDOW_MS: i64 = 30_000;
// Near-deterministic window: when a file's mtime is within a few seconds of a tool_result's
// completion timestamp AND the tool's input or output mentions the path, we consider it a
// definite attribution rather than a guess.
const TIGHT_MATCH_WINDOW_MS: i64 = 5_000;
const STALE_TAIL_MS: i64 = 60 * 60 * 1000; // 1 hour: skip JSONLs untouched for this long
// On first discovery of a JSONL, read the last ~64KB so recent user prompts and tool_uses
// land in the index even if they were written before we attached. Without this, opening a
// new Claude session in another terminal loses the prompt for any plot rendered within ~10s
// of the prompt — the discovery happens after the prompt line is already on disk.
const BACKFILL_BYTES: u64 = 64 * 1024;
// Tool_result outputs can be huge (build logs, stdout from long jobs). Cap stored output so
// memory stays bounded; the path-mention check only needs the bytes that mention the file.
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
struct RawEnvelope {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub ts_ms: i64,
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub preceding_prompt: Option<String>,
    /// Timestamp of the tool_result envelope that completed this tool call. None until the
    /// matching tool_result line has been parsed. Once set, file mtimes within
    /// TIGHT_MATCH_WINDOW_MS of this become near-deterministic attributions.
    pub result_ts_ms: Option<i64>,
    /// Combined text of the tool_result content blocks, truncated to MAX_OUTPUT_BYTES. Used
    /// to catch flows like `python plot.py` where the file path appears only in stdout.
    pub result_output: Option<String>,
    /// Claude Code session UUID (filename stem of the JSONL the entry came from).
    pub session_id: Option<String>,
    /// Working directory the Claude Code session was launched in.
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct UserPromptEntry {
    ts_ms: i64,
    text: String,
    session_id: Option<String>,
    cwd: Option<String>,
}

/// Result of a successful prompt-to-file attribution. Always carries a prompt; the rest is
/// best-effort metadata the UI can use for "more info".
#[derive(Debug, Clone)]
pub struct LookupHit {
    pub prompt: String,
    pub tool_use_id: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Default)]
pub struct TranscriptIndex {
    last_user_prompt: Option<UserPromptEntry>,
    tools: VecDeque<ToolEntry>,
}

impl TranscriptIndex {
    /// Four-tier matcher. Tighter tiers run first; we fall through only when nothing better
    /// is available. `LookupHit::tool_use_id` is set when the match came from a specific tool
    /// call (Tiers 1–2); `session_id`/`cwd` are best-effort and propagate from the JSONL
    /// envelope that produced the matched entry.
    pub fn lookup(&self, mtime_ms: i64, abs_path: &str) -> Option<LookupHit> {
        // Tier 1 — Deterministic. Tool result completed close to file mtime AND input/output
        // mentions the path. Strongest possible signal: the tool was active when the file
        // appeared AND the tool explicitly named the file.
        for tool in self.tools.iter().rev() {
            let Some(result_ts) = tool.result_ts_ms else {
                continue;
            };
            if (mtime_ms - result_ts).abs() <= TIGHT_MATCH_WINDOW_MS
                && tool_mentions_path(tool, abs_path)
            {
                if let Some(p) = tool.preceding_prompt.clone() {
                    return Some(hit_from_tool(p, tool));
                }
            }
        }

        // Tier 1.5 — Timing alone. Tool result completed within TIGHT window of file mtime;
        // the file was produced during the tool's execution. Reliable even when stdout
        // doesn't echo the file name — matplotlib scripts, build tools, generators that
        // write quietly. The absence of a path mention doesn't break the link: a Bash
        // running `python plot.py` is overwhelmingly likely to be what wrote the new PNG
        // that appeared while it was running.
        for tool in self.tools.iter().rev() {
            let Some(result_ts) = tool.result_ts_ms else {
                continue;
            };
            if (mtime_ms - result_ts).abs() <= TIGHT_MATCH_WINDOW_MS {
                if let Some(p) = tool.preceding_prompt.clone() {
                    return Some(hit_from_tool(p, tool));
                }
            }
        }

        // Tier 2 — Probable. Path mentioned in input/output, but timing is wider than Tier 1.
        // Catches Write/Edit (file_path in input) where the tool may have completed seconds
        // before the actual file flush, or Bash whose command string includes the path.
        for tool in self.tools.iter().rev() {
            if (mtime_ms - tool.ts_ms).abs() <= MATCH_WINDOW_MS
                && tool_mentions_path(tool, abs_path)
            {
                if let Some(p) = tool.preceding_prompt.clone() {
                    return Some(hit_from_tool(p, tool));
                }
            }
        }

        // Tier 3 — Fallback. Most recent user prompt within ±30s, no tool_use_id. The "guess"
        // path: we know roughly when the user asked, and we know a file appeared, but we
        // can't pin it to a specific tool call.
        if let Some(entry) = &self.last_user_prompt {
            if (mtime_ms - entry.ts_ms).abs() <= MATCH_WINDOW_MS {
                return Some(LookupHit {
                    prompt: entry.text.clone(),
                    tool_use_id: None,
                    session_id: entry.session_id.clone(),
                    cwd: entry.cwd.clone(),
                });
            }
        }
        None
    }
}

fn hit_from_tool(prompt: String, tool: &ToolEntry) -> LookupHit {
    LookupHit {
        prompt,
        tool_use_id: Some(tool.tool_use_id.clone()),
        session_id: tool.session_id.clone(),
        cwd: tool.cwd.clone(),
    }
}

pub type SharedIndex = Arc<Mutex<TranscriptIndex>>;

pub fn new_index() -> SharedIndex {
    Arc::new(Mutex::new(TranscriptIndex::default()))
}

/// Resolve `~/.claude/projects/<encoded-cwd>/` for the given absolute working dir.
#[allow(dead_code)]
pub fn resolve_session_dir(cwd: &Path) -> Option<PathBuf> {
    let home = home_dir()?;
    let encoded = encode_cwd(cwd);
    let dir = home.join(".claude").join("projects").join(encoded);
    if dir.is_dir() { Some(dir) } else { None }
}

/// Pick the most recently modified *.jsonl in a session dir.
#[allow(dead_code)]
pub fn pick_latest_session(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match &best {
            Some((_, prev)) if *prev >= mtime => {}
            _ => best = Some((path, mtime)),
        }
    }
    best.map(|(p, _)| p)
}

/// Global tail task: scans every `~/.claude/projects/*/` for recently-active JSONLs and
/// tails them all into the shared index. Auto-discovers new sessions / new files.
/// Each newly-seen file starts at its current end; only appended lines are processed.
pub fn start_global_tail(app: AppHandle, state: Arc<AppState>) {
    let index = state.global_index.clone();
    tauri::async_runtime::spawn(async move {
        struct FileState {
            offset: u64,
            buffer: String,
        }
        let mut files: HashMap<PathBuf, FileState> = HashMap::new();
        let mut tick: u32 = 0;
        let mut active_jsonls: Vec<PathBuf> = Vec::new();

        loop {
            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
            tick = tick.wrapping_add(1);

            if tick % RESCAN_INTERVAL_TICKS == 1 {
                active_jsonls = scan_active_jsonls();
                // Drop file state for sessions that have gone idle (free memory).
                let live: std::collections::HashSet<&PathBuf> = active_jsonls.iter().collect();
                files.retain(|p, _| live.contains(p));
            }

            let mut dirty = false;
            for path in &active_jsonls {
                let len = match tokio::fs::metadata(path).await {
                    Ok(m) => m.len(),
                    Err(_) => continue,
                };

                let fs_state = files.entry(path.clone()).or_insert_with(|| FileState {
                    offset: len.saturating_sub(BACKFILL_BYTES),
                    buffer: String::new(),
                });

                if len == fs_state.offset {
                    continue;
                }
                if len < fs_state.offset {
                    fs_state.offset = len;
                    continue;
                }

                let mut file = match tokio::fs::File::open(path).await {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                if file
                    .seek(std::io::SeekFrom::Start(fs_state.offset))
                    .await
                    .is_err()
                {
                    continue;
                }
                let mut chunk = Vec::with_capacity((len - fs_state.offset) as usize);
                use tokio::io::AsyncReadExt;
                if file.read_to_end(&mut chunk).await.is_err() {
                    continue;
                }
                fs_state.offset = len;
                fs_state.buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(idx) = fs_state.buffer.find('\n') {
                    let line = fs_state.buffer[..idx].to_string();
                    fs_state.buffer.drain(..=idx);
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    process_line(&index, trimmed);
                    dirty = true;
                }
            }

            if dirty {
                enrich_pending_all(&app, &state, &index);
            }
        }
    });
}

fn scan_active_jsonls() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let projects = home.join(".claude").join("projects");
    let now_ms = Utc::now().timestamp_millis();
    let cutoff_ms = now_ms - STALE_TAIL_MS;

    let mut result = Vec::new();
    let session_dirs = match std::fs::read_dir(&projects) {
        Ok(d) => d,
        Err(_) => return result,
    };
    for sd in session_dirs.flatten() {
        let dir = sd.path();
        if !dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let mtime_ms = match entry.metadata().and_then(|m| m.modified()) {
                Ok(t) => t
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                Err(_) => continue,
            };
            if mtime_ms >= cutoff_ms {
                result.push(path);
            }
        }
    }
    result
}

pub(crate) fn process_line(index: &SharedIndex, line: &str) {
    let envelope: RawEnvelope = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                ?err,
                line_preview = %line.chars().take(120).collect::<String>(),
                "transcript: malformed JSONL line"
            );
            return;
        }
    };
    let ts_ms = envelope
        .timestamp
        .as_deref()
        .and_then(parse_iso8601_ms)
        .unwrap_or_else(|| Utc::now().timestamp_millis());

    let session_id = envelope.session_id.clone();
    let cwd = envelope.cwd.clone();

    match envelope.kind.as_deref() {
        Some("user") => {
            // A "user" envelope is either a typed prompt OR a wrapper for tool_result blocks.
            // Both branches can in theory coexist, so handle them independently.
            if let Some(text) = extract_user_prompt(envelope.message.as_ref()) {
                index.lock().last_user_prompt = Some(UserPromptEntry {
                    ts_ms,
                    text,
                    session_id: session_id.clone(),
                    cwd: cwd.clone(),
                });
            }
            let results = extract_tool_results(envelope.message.as_ref());
            if !results.is_empty() {
                let mut idx = index.lock();
                for (tool_use_id, output) in results {
                    if let Some(tool) = idx
                        .tools
                        .iter_mut()
                        .rev()
                        .find(|t| t.tool_use_id == tool_use_id)
                    {
                        tool.result_ts_ms = Some(ts_ms);
                        tool.result_output = Some(truncate_output(output));
                    }
                }
            }
        }
        Some("assistant") => {
            let prompt = index
                .lock()
                .last_user_prompt
                .as_ref()
                .map(|e| e.text.clone());
            extract_tool_uses(
                envelope.message.as_ref(),
                ts_ms,
                &prompt,
                session_id.as_deref(),
                cwd.as_deref(),
                |entry| {
                    let mut idx = index.lock();
                    if idx.tools.len() == TOOL_RING_CAP {
                        idx.tools.pop_front();
                    }
                    idx.tools.push_back(entry);
                },
            );
        }
        _ => {}
    }
}

fn extract_user_prompt(msg: Option<&RawMessage>) -> Option<String> {
    let m = msg?;
    if let Some(s) = m.content.as_str() {
        if is_meta_user_text(s) {
            return None;
        }
        return Some(s.to_string());
    }
    // content can be an array — for tool_results, skip; for plain text blocks, accept.
    if let Some(arr) = m.content.as_array() {
        for v in arr {
            if v.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                    if !is_meta_user_text(text) {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }
    None
}

fn is_meta_user_text(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("<local-command-")
        || t.starts_with("<command-")
        || t.starts_with("<system-reminder>")
        || t.starts_with("<user-prompt-submit-hook>")
}

fn extract_tool_uses<F: FnMut(ToolEntry)>(
    msg: Option<&RawMessage>,
    ts_ms: i64,
    preceding_prompt: &Option<String>,
    session_id: Option<&str>,
    cwd: Option<&str>,
    mut sink: F,
) {
    let arr = match msg.and_then(|m| m.content.as_array()) {
        Some(a) => a,
        None => return,
    };
    for v in arr {
        if v.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let input = v.get("input").cloned().unwrap_or(serde_json::Value::Null);
        sink(ToolEntry {
            ts_ms,
            tool_use_id: id,
            name,
            input,
            preceding_prompt: preceding_prompt.clone(),
            result_ts_ms: None,
            result_output: None,
            session_id: session_id.map(String::from),
            cwd: cwd.map(String::from),
        });
    }
}

/// Extract tool_result blocks from a "user"-typed envelope. Claude Code wraps tool outputs in
/// user messages whose content array contains `{type: "tool_result", tool_use_id, content}`
/// blocks. The `content` field is either a plain string or an array of content blocks; we
/// accept both and concatenate text-typed entries (skipping image blocks).
fn extract_tool_results(msg: Option<&RawMessage>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(m) = msg else {
        return out;
    };
    let Some(arr) = m.content.as_array() else {
        return out;
    };
    for v in arr {
        if v.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
            continue;
        }
        let Some(id) = v.get("tool_use_id").and_then(|x| x.as_str()) else {
            continue;
        };
        let content = v.get("content");
        let text = if let Some(s) = content.and_then(|c| c.as_str()) {
            s.to_string()
        } else if let Some(a) = content.and_then(|c| c.as_array()) {
            a.iter()
                .filter_map(|b| {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        b.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            continue;
        };
        out.push((id.to_string(), text));
    }
    out
}

fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    // Keep the tail — saved-to-X messages tend to come at the end of script output.
    let start = s.len() - MAX_OUTPUT_BYTES;
    // Walk forward to a char boundary so we don't slice mid-codepoint.
    let mut start = start;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// Substring search with simple word-boundary semantics on byte boundaries.
///
/// A "body" byte is `[A-Za-z0-9_.-]`. A match counts only when both the byte
/// before and the byte after the match are NOT body bytes (or the match sits
/// at the start/end of the haystack). Operating on bytes is safe here because
/// non-ASCII UTF-8 continuation bytes all have the high bit set and are
/// therefore not in the body set, so multi-byte basenames still get a clean
/// boundary check on either side.
fn contains_as_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hb = haystack.as_bytes();
    let nb = needle.as_bytes();
    if nb.len() > hb.len() {
        return false;
    }
    let is_body = |b: u8| -> bool {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-'
    };
    let mut i = 0;
    while i + nb.len() <= hb.len() {
        if &hb[i..i + nb.len()] == nb {
            let left_ok = i == 0 || !is_body(hb[i - 1]);
            let right_idx = i + nb.len();
            let right_ok = right_idx == hb.len() || !is_body(hb[right_idx]);
            if left_ok && right_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn tool_mentions_path(tool: &ToolEntry, abs_path: &str) -> bool {
    // Strongest signal: explicit file_path input on Write/Edit-style tools.
    if let Some(fp) = tool.input.get("file_path").and_then(|v| v.as_str()) {
        if fp == abs_path {
            return true;
        }
    }
    // Bash command body may reference the file by full path or basename.
    if tool.name == "Bash" {
        if let Some(cmd) = tool.input.get("command").and_then(|v| v.as_str()) {
            if cmd.contains(abs_path) {
                return true;
            }
            if let Some(base) = std::path::Path::new(abs_path).file_name().and_then(|n| n.to_str())
            {
                if contains_as_word(cmd, base) {
                    return true;
                }
            }
        }
    }
    // Tool result output: catches `python plot.py` style flows where matplotlib's stdout
    // ("Saved figure to plot.png") names the file even though the input.command does not.
    if let Some(output) = &tool.result_output {
        if output.contains(abs_path) {
            return true;
        }
        if let Some(base) = std::path::Path::new(abs_path).file_name().and_then(|n| n.to_str()) {
            if contains_as_word(output, base) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn enrich_pending_all(app: &AppHandle, state: &Arc<AppState>, index: &SharedIndex) {
    let pending: Vec<(String, String, i64)> = {
        let items = state.items.lock();
        items
            .values()
            .filter(|i| i.prompt.is_none())
            .map(|i| (i.watch_id.clone(), i.abs_path.clone(), i.mtime))
            .collect()
    };
    if pending.is_empty() {
        return;
    }
    let resolved: Vec<(String, String, LookupHit)> = {
        let idx = index.lock();
        pending
            .into_iter()
            .filter_map(|(watch_id, abs_path, mtime)| {
                idx.lookup(mtime, &abs_path)
                    .map(|hit| (watch_id, abs_path, hit))
            })
            .collect()
    };
    let mut any_enriched = false;
    for (watch_id, abs_path, hit) in resolved {
        let already = {
            let mut items = state.items.lock();
            let key = (watch_id.clone(), abs_path.clone());
            match items.get_mut(&key) {
                Some(item) if item.prompt.is_none() => {
                    item.prompt = Some(hit.prompt.clone());
                    item.tool_use_id = hit.tool_use_id.clone();
                    item.session_id = hit.session_id.clone();
                    item.cwd = hit.cwd.clone();
                    false
                }
                _ => true,
            }
        };
        if already {
            continue;
        }
        any_enriched = true;
        let _ = app.emit(
            "viz:enriched",
            VizEnriched {
                watch_id,
                abs_path,
                prompt: hit.prompt,
                tool_use_id: hit.tool_use_id,
                session_id: hit.session_id,
                cwd: hit.cwd,
            },
        );
    }
    if any_enriched {
        mark_history_dirty(state);
    }
}

/// Public hook called by the FS watcher just after emitting viz:new/viz:updated.
pub fn try_enrich_now(
    app: &AppHandle,
    state: &Arc<AppState>,
    watch_id: &str,
    index: &SharedIndex,
    abs_path: &str,
    mtime_ms: i64,
) {
    let Some(hit) = index.lock().lookup(mtime_ms, abs_path) else {
        return;
    };
    let mutated = {
        let key = (watch_id.to_string(), abs_path.to_string());
        let mut items = state.items.lock();
        if let Some(item) = items.get_mut(&key) {
            item.prompt = Some(hit.prompt.clone());
            item.tool_use_id = hit.tool_use_id.clone();
            item.session_id = hit.session_id.clone();
            item.cwd = hit.cwd.clone();
            true
        } else {
            false
        }
    };
    if mutated {
        mark_history_dirty(state);
    }
    let _ = app.emit(
        "viz:enriched",
        VizEnriched {
            watch_id: watch_id.to_string(),
            abs_path: abs_path.to_string(),
            prompt: hit.prompt,
            tool_use_id: hit.tool_use_id,
            session_id: hit.session_id,
            cwd: hit.cwd,
        },
    );
}

fn parse_iso8601_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[allow(dead_code)]
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy().replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw_msg(content: serde_json::Value) -> RawMessage {
        RawMessage { content }
    }

    #[test]
    fn meta_user_text_recognized() {
        assert!(is_meta_user_text("<command-name>foo</command-name>"));
        assert!(is_meta_user_text("<system-reminder>x</system-reminder>"));
        assert!(is_meta_user_text("<local-command-stdout>x"));
        assert!(is_meta_user_text("<user-prompt-submit-hook>x"));
        assert!(is_meta_user_text("  \n\t<command-foo>"));
        assert!(!is_meta_user_text("plot the thing"));
        assert!(!is_meta_user_text(""));
    }

    #[test]
    fn user_prompt_extracted_from_string_content() {
        let m = raw_msg(json!("plot sin(x)"));
        assert_eq!(extract_user_prompt(Some(&m)).as_deref(), Some("plot sin(x)"));
    }

    #[test]
    fn user_prompt_extracted_from_array_text_block() {
        let m = raw_msg(json!([{"type": "text", "text": "plot sin(x)"}]));
        assert_eq!(extract_user_prompt(Some(&m)).as_deref(), Some("plot sin(x)"));
    }

    #[test]
    fn user_prompt_skips_meta() {
        let m = raw_msg(json!("<system-reminder>be helpful</system-reminder>"));
        assert!(extract_user_prompt(Some(&m)).is_none());
    }

    #[test]
    fn user_prompt_skips_tool_results() {
        let m = raw_msg(json!([{"type": "tool_result", "content": "x"}]));
        assert!(extract_user_prompt(Some(&m)).is_none());
    }

    fn mk_tool(name: &str, ts: i64, id: &str, input: serde_json::Value, prompt: Option<&str>) -> ToolEntry {
        ToolEntry {
            ts_ms: ts,
            tool_use_id: id.into(),
            name: name.into(),
            input,
            preceding_prompt: prompt.map(String::from),
            result_ts_ms: None,
            result_output: None,
            session_id: None,
            cwd: None,
        }
    }

    #[test]
    fn tool_uses_extracted() {
        let m = raw_msg(json!([
            {"type": "tool_use", "id": "tu_1", "name": "Write",
             "input": {"file_path": "/tmp/a.png"}},
            {"type": "text", "text": "ignored"},
            {"type": "tool_use", "id": "tu_2", "name": "Bash",
             "input": {"command": "python plot.py"}},
        ]));
        let mut out: Vec<ToolEntry> = vec![];
        extract_tool_uses(
            Some(&m),
            1000,
            &Some("p".into()),
            Some("sess-uuid"),
            Some("/Users/x/proj"),
            |e| out.push(e),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].tool_use_id, "tu_1");
        assert_eq!(out[1].name, "Bash");
        assert_eq!(out[0].preceding_prompt.as_deref(), Some("p"));
        assert!(out[0].result_ts_ms.is_none());
        assert!(out[0].result_output.is_none());
        assert_eq!(out[0].session_id.as_deref(), Some("sess-uuid"));
        assert_eq!(out[1].cwd.as_deref(), Some("/Users/x/proj"));
    }

    #[test]
    fn tool_results_extracted_string_content() {
        let m = raw_msg(json!([
            {"type": "tool_result", "tool_use_id": "tu_1", "content": "Saved figure to plot.png"},
        ]));
        let r = extract_tool_results(Some(&m));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "tu_1");
        assert_eq!(r[0].1, "Saved figure to plot.png");
    }

    #[test]
    fn tool_results_extracted_array_content() {
        let m = raw_msg(json!([
            {"type": "tool_result", "tool_use_id": "tu_1", "content": [
                {"type": "text", "text": "line 1"},
                {"type": "text", "text": "line 2"},
                {"type": "image", "source": {}},
            ]},
        ]));
        let r = extract_tool_results(Some(&m));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "line 1\nline 2");
    }

    #[test]
    fn tool_results_skip_non_results() {
        let m = raw_msg(json!([
            {"type": "text", "text": "just a prompt"},
        ]));
        assert!(extract_tool_results(Some(&m)).is_empty());
    }

    #[test]
    fn tool_mentions_path_by_file_path_input() {
        let t = mk_tool("Write", 0, "x", json!({"file_path": "/tmp/p.png"}), None);
        assert!(tool_mentions_path(&t, "/tmp/p.png"));
        assert!(!tool_mentions_path(&t, "/tmp/q.png"));
    }

    #[test]
    fn tool_mentions_path_by_bash_command_substring() {
        let t = mk_tool(
            "Bash",
            0,
            "x",
            json!({"command": "python plot.py && open /tmp/p.png"}),
            None,
        );
        assert!(tool_mentions_path(&t, "/tmp/p.png"));
    }

    #[test]
    fn tool_mentions_path_by_bash_basename() {
        let t = mk_tool("Bash", 0, "x", json!({"command": "cat p.png"}), None);
        assert!(tool_mentions_path(&t, "/some/dir/p.png"));
    }

    #[test]
    fn tool_mentions_path_by_result_output() {
        let mut t = mk_tool("Bash", 0, "x", json!({"command": "python plot.py"}), None);
        // Input doesn't mention the file — only the script's stdout does.
        assert!(!tool_mentions_path(&t, "/tmp/plot.png"));
        t.result_output = Some("Saved figure to /tmp/plot.png".into());
        assert!(tool_mentions_path(&t, "/tmp/plot.png"));
    }

    #[test]
    fn tool_mentions_path_by_result_output_basename() {
        let mut t = mk_tool("Bash", 0, "x", json!({"command": "python plot.py"}), None);
        t.result_output = Some("matplotlib: written to plot.png".into());
        assert!(tool_mentions_path(&t, "/tmp/plot.png"));
    }

    #[test]
    fn tool_mentions_path_basename_substring_does_not_match() {
        // `myout.png` should not match `out.png` (left side is alphanumeric).
        let t = mk_tool("Bash", 0, "x", json!({"command": "cp myout.png /backup/"}), None);
        assert!(!tool_mentions_path(&t, "/work/out.png"));
        // `out.png.bak` should not match `out.png` (right side is `.`, part of body).
        let t = mk_tool("Bash", 0, "x", json!({"command": "rm out.png.bak"}), None);
        assert!(!tool_mentions_path(&t, "/work/out.png"));
    }

    #[test]
    fn tool_mentions_path_basename_substring_in_result_output_does_not_match() {
        let mut t = mk_tool("Bash", 0, "x", json!({"command": "python plot.py"}), None);
        t.result_output = Some("found myout.png and out.png.bak".into());
        assert!(!tool_mentions_path(&t, "/work/out.png"));
    }

    #[test]
    fn tool_mentions_path_basename_inside_path_segment_does_not_match() {
        // `/data/a.csv.gz` should not match `/work/a.csv`.
        let t = mk_tool("Bash", 0, "x", json!({"command": "gunzip /data/a.csv.gz"}), None);
        assert!(!tool_mentions_path(&t, "/work/a.csv"));
    }

    #[test]
    fn tool_mentions_path_matplotlib_stdout_basename_still_matches() {
        // Regression guard: classic matplotlib stdout still matches via basename.
        let mut t = mk_tool("Bash", 0, "x", json!({"command": "python plot.py"}), None);
        t.result_output = Some("Saved figure to plot.png".into());
        assert!(tool_mentions_path(&t, "/cwd/plot.png"));
    }

    #[test]
    fn tool_mentions_path_basename_at_string_start_matches() {
        let t = mk_tool("Bash", 0, "x", json!({"command": "plot.png"}), None);
        assert!(tool_mentions_path(&t, "/cwd/plot.png"));
    }

    #[test]
    fn tool_mentions_path_basename_at_string_end_matches() {
        let mut t = mk_tool("Bash", 0, "x", json!({"command": "python plot.py"}), None);
        t.result_output = Some("Wrote plot.png".into());
        assert!(tool_mentions_path(&t, "/cwd/plot.png"));
    }

    #[test]
    fn tool_mentions_path_basename_quoted_matches() {
        let t = mk_tool("Bash", 0, "x", json!({"command": "open \"plot.png\""}), None);
        assert!(tool_mentions_path(&t, "/cwd/plot.png"));
    }

    #[test]
    fn tool_mentions_path_dotfile_basename() {
        // `.env` should match a real `.env` reference.
        let t = mk_tool("Bash", 0, "x", json!({"command": "cat .env"}), None);
        assert!(tool_mentions_path(&t, "/proj/.env"));
        // But `my.env` should not match `.env` (left side is alphanumeric).
        let t = mk_tool("Bash", 0, "x", json!({"command": "cat my.env"}), None);
        assert!(!tool_mentions_path(&t, "/proj/.env"));
    }

    #[test]
    fn tool_mentions_path_basename_with_space() {
        let t = mk_tool("Bash", 0, "x", json!({"command": "open \"my plot.png\""}), None);
        assert!(tool_mentions_path(&t, "/cwd/my plot.png"));
    }

    #[test]
    fn tool_mentions_path_unicode_basename() {
        // Non-ASCII basename should still match when word-bounded.
        let t = mk_tool("Bash", 0, "x", json!({"command": "cat sín.png"}), None);
        assert!(tool_mentions_path(&t, "/cwd/sín.png"));
        // But `sín.png.bak` should not match `sín.png`.
        let t = mk_tool("Bash", 0, "x", json!({"command": "cat sín.png.bak"}), None);
        assert!(!tool_mentions_path(&t, "/cwd/sín.png"));
    }

    #[test]
    fn lookup_prefers_tool_match_with_prompt() {
        let mut idx = TranscriptIndex::default();
        idx.last_user_prompt = Some(UserPromptEntry {
            ts_ms: 1000,
            text: "old prompt".into(),
            session_id: None,
            cwd: None,
        });
        idx.tools.push_back(mk_tool(
            "Write",
            5000,
            "tu",
            json!({"file_path": "/tmp/p.png"}),
            Some("the right prompt"),
        ));
        let hit = idx.lookup(5100, "/tmp/p.png").unwrap();
        assert_eq!(hit.prompt, "the right prompt");
        assert_eq!(hit.tool_use_id.as_deref(), Some("tu"));
    }

    #[test]
    fn lookup_tier1_deterministic_via_result_output() {
        // Bash tool_use at t=1000 (input doesn't name the file), tool_result at t=5000 with
        // the file path in stdout. File mtime at 5100 → within TIGHT window of result_ts.
        let mut idx = TranscriptIndex::default();
        let mut tool = mk_tool(
            "Bash",
            1000,
            "tu",
            json!({"command": "python plot.py"}),
            Some("plot sin(x)"),
        );
        tool.result_ts_ms = Some(5000);
        tool.result_output = Some("Saved to /tmp/p.png".into());
        tool.session_id = Some("sess-1".into());
        tool.cwd = Some("/proj".into());
        idx.tools.push_back(tool);

        // Also add a more recent user prompt that would tempt Tier 3.
        idx.last_user_prompt = Some(UserPromptEntry {
            ts_ms: 5050,
            text: "different prompt".into(),
            session_id: None,
            cwd: None,
        });

        let hit = idx.lookup(5100, "/tmp/p.png").unwrap();
        assert_eq!(hit.prompt, "plot sin(x)");
        assert_eq!(hit.tool_use_id.as_deref(), Some("tu"));
        // session_id and cwd propagate from the matched tool entry.
        assert_eq!(hit.session_id.as_deref(), Some("sess-1"));
        assert_eq!(hit.cwd.as_deref(), Some("/proj"));
    }

    #[test]
    fn lookup_tier2_when_result_too_far_for_tier1() {
        // Same as above but mtime is 20s after result_ts — too far for Tier 1, but the path
        // mention via output still beats the fallback prompt.
        let mut idx = TranscriptIndex::default();
        let mut tool = mk_tool(
            "Bash",
            1000,
            "tu",
            json!({"command": "python plot.py"}),
            Some("plot sin(x)"),
        );
        tool.result_ts_ms = Some(5000);
        tool.result_output = Some("Saved to /tmp/p.png".into());
        idx.tools.push_back(tool);

        idx.last_user_prompt = Some(UserPromptEntry {
            ts_ms: 24_000,
            text: "different prompt".into(),
            session_id: None,
            cwd: None,
        });

        let hit = idx.lookup(25_000, "/tmp/p.png").unwrap();
        assert_eq!(hit.prompt, "plot sin(x)");
        assert_eq!(hit.tool_use_id.as_deref(), Some("tu"));
    }

    #[test]
    fn lookup_tier1_5_timing_only_attribution() {
        // Real scenario: user asked to plot sin(x), Claude went through retries (file path
        // changed mid-flight, sandbox rejected, etc.), final Bash ran a script that wrote
        // the file but neither the command nor the stdout mention the file name. Time-wise
        // the file mtime coincides with the tool_result. We should still attribute correctly.
        let mut idx = TranscriptIndex::default();
        let mut tool = mk_tool(
            "Bash",
            10_000,
            "tu",
            json!({"command": "MPLCONFIGDIR=$TMPDIR python3 sin_plot.py"}),
            Some("plot sin(x) and save to /tmp/plot.png. Run it"),
        );
        tool.result_ts_ms = Some(15_000);
        // Output says nothing about the PNG — just unrelated runtime warnings.
        tool.result_output = Some("NSXPCSharedListener noise\nMatplotlib font cache".into());
        idx.tools.push_back(tool);

        // The user's original prompt is OLD (90s before the file) — outside Tier 3 window.
        idx.last_user_prompt = Some(UserPromptEntry {
            ts_ms: 10_000 - 90_000,
            text: "different older prompt".into(),
            session_id: None,
            cwd: None,
        });

        // File appeared 22ms before result_ts (essentially during tool execution).
        let hit = idx.lookup(14_978, "/cwd/sin_plot.png").unwrap();
        assert_eq!(hit.prompt, "plot sin(x) and save to /tmp/plot.png. Run it");
        assert_eq!(hit.tool_use_id.as_deref(), Some("tu"));
    }

    #[test]
    fn lookup_tier1_5_does_not_match_outside_tight_window() {
        // Tool completed 20s before file mtime — outside TIGHT, no path mention, so Tier 1.5
        // shouldn't fire. Falls through to Tier 3 (user prompt) if available, else None.
        let mut idx = TranscriptIndex::default();
        let mut tool = mk_tool("Bash", 0, "tu", json!({"command": "python plot.py"}), Some("p"));
        tool.result_ts_ms = Some(0);
        tool.result_output = Some("done".into());
        idx.tools.push_back(tool);
        // No user prompt; the only candidate is the (out-of-tight-window) tool, no path match.
        assert!(idx.lookup(20_000, "/tmp/foo.png").is_none());
    }

    #[test]
    fn lookup_truncate_output_keeps_tail() {
        let big = "x".repeat(MAX_OUTPUT_BYTES + 100);
        let s = format!("{}saved to /tmp/p.png", big);
        let truncated = truncate_output(s);
        assert!(truncated.len() <= MAX_OUTPUT_BYTES);
        assert!(truncated.contains("/tmp/p.png"));
    }

    #[test]
    fn lookup_falls_back_to_last_user_prompt_within_window() {
        let mut idx = TranscriptIndex::default();
        idx.last_user_prompt = Some(UserPromptEntry {
            ts_ms: 10_000,
            text: "user said this".into(),
            session_id: Some("sess-fallback".into()),
            cwd: Some("/proj".into()),
        });
        let hit = idx.lookup(15_000, "/tmp/p.png").unwrap();
        assert_eq!(hit.prompt, "user said this");
        assert!(hit.tool_use_id.is_none());
        // Tier 3 also forwards session_id/cwd from the user-prompt envelope.
        assert_eq!(hit.session_id.as_deref(), Some("sess-fallback"));
        assert_eq!(hit.cwd.as_deref(), Some("/proj"));
    }

    #[test]
    fn lookup_returns_none_outside_window() {
        let mut idx = TranscriptIndex::default();
        idx.last_user_prompt = Some(UserPromptEntry {
            ts_ms: 0,
            text: "stale".into(),
            session_id: None,
            cwd: None,
        });
        // mtime - ts = 60s > 30s window
        assert!(idx.lookup(60_000, "/tmp/p.png").is_none());
    }

    #[test]
    #[tracing_test::traced_test]
    fn process_line_warns_on_malformed_json() {
        let index = new_index();
        process_line(&index, "{not json");
        assert!(logs_contain("transcript: malformed JSONL line"));
    }
}
