//! Adapter over `ssh2-config` that yields TS-friendly host entries.
//!
//! We deliberately delegate parsing to the crate (Include / Match / Host * defaults / percent
//! expansion / etc. are real and silent failures hurt users). The adapter only filters the
//! parsed `Host` blocks down to concrete aliases (no wildcards, no negations) and resolves
//! each via `query()` so `Host *` defaults compose correctly.

use crate::types::SshHostEntry;
use ssh2_config::{ParseRule, SshConfig};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

pub fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

pub fn default_config_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".ssh").join("config"))
}

pub fn known_hosts_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".ssh").join("known_hosts"))
}

/// Parse `~/.ssh/config` if it exists. Returns an empty config (not an error) when absent so
/// the UI just shows the manual-entry path.
pub fn load_config() -> SshConfig {
    let Some(path) = default_config_path() else {
        return SshConfig::default();
    };
    if !path.exists() {
        return SshConfig::default();
    }
    match File::open(&path) {
        Ok(f) => {
            let mut reader = BufReader::new(f);
            // Use lax parsing — strict bails on a single unsupported keyword and the failure
            // mode "your hosts disappeared" is hostile.
            SshConfig::default()
                .parse(&mut reader, ParseRule::ALLOW_UNKNOWN_FIELDS)
                .unwrap_or_default()
        }
        Err(_) => SshConfig::default(),
    }
}

pub fn list_hosts() -> Vec<SshHostEntry> {
    let cfg = load_config();
    let mut out: Vec<SshHostEntry> = Vec::new();

    for host in cfg.get_hosts() {
        // Take the first non-negated, non-wildcard pattern as the user-facing alias.
        let Some(clause) = host
            .pattern
            .iter()
            .find(|c| !c.negated && !is_wildcard(&c.pattern))
        else {
            continue;
        };
        let alias = clause.pattern.clone();
        if alias.is_empty() {
            continue;
        }
        // Resolve via query() so Host * defaults (User, Port, IdentityFile) are folded in.
        let params = cfg.query(&alias);
        let host_name = params.host_name.clone().unwrap_or_else(|| alias.clone());
        let port = params.port.unwrap_or(22);
        let user = params.user.clone();
        out.push(SshHostEntry {
            alias,
            host_name,
            user,
            port,
        });
    }

    out
}

pub fn resolve(alias: &str) -> ResolvedHost {
    let cfg = load_config();
    let params = cfg.query(alias);
    ResolvedHost {
        alias: alias.to_string(),
        host_name: params.host_name.unwrap_or_else(|| alias.to_string()),
        user: params.user,
        port: params.port.unwrap_or(22),
        identity_files: params.identity_file.unwrap_or_default(),
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // alias / identity_files are read by future v1.5 known_hosts + key-file paths
pub struct ResolvedHost {
    pub alias: String,
    pub host_name: String,
    pub user: Option<String>,
    pub port: u16,
    pub identity_files: Vec<PathBuf>,
}

fn is_wildcard(pat: &str) -> bool {
    pat.contains('*') || pat.contains('?') || pat.contains('!')
}

/// Best-effort lookup of an entry in `~/.ssh/known_hosts`. Returns `Some(line)` if any line
/// targets `host` (or `[host]:port`), else `None`. Caller compares the parsed key against the
/// server-presented one. Slated for use in v1.5 host-key verification.
#[allow(dead_code)]
pub fn lookup_known_host(host: &str, port: u16) -> Option<Vec<String>> {
    let path = known_hosts_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let needle_default = host.to_string();
    let needle_ported = format!("[{}]:{}", host, port);
    let mut out: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let Some(host_field) = parts.next() else {
            continue;
        };
        // host_field can be a comma-separated list, can be hashed (|1|salt|hash). We do plain
        // matching only; hashed entries are skipped (the user can `ssh devbox` to add a plain
        // entry, or we fall back to TOFU at the connection layer).
        if host_field.starts_with("|") {
            continue;
        }
        for entry in host_field.split(',') {
            if entry == needle_default || entry == needle_ported {
                out.push(line.to_string());
                break;
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// True if the file `~/.ssh/config` exists (signal for the UI: "we have hosts to show").
#[allow(dead_code)]
pub fn config_exists() -> bool {
    default_config_path().map(|p| p.exists()).unwrap_or(false)
}

#[allow(dead_code)]
pub fn known_hosts_exists() -> bool {
    known_hosts_path().map(|p| p.exists()).unwrap_or(false)
}

#[allow(dead_code)]
pub fn _force_use(_: &Path) {} // suppress dead_code warnings during incremental dev
