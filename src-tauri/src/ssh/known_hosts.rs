//! Strict known_hosts verification + learn (TOFU on user confirmation).
//!
//! We deliberately reuse `russh_keys::known_hosts::check_known_hosts_path` for the core
//! match (it handles both plaintext and `|1|salt|hash` HMAC-SHA1 forms correctly), but we
//! layer extra semantics on top:
//!
//! - `@revoked <hosts>` lines short-circuit the file scan with `Revoked` regardless of any
//!   later matching key. OpenSSH treats revoked as a hard "never trust this key for this
//!   host", so we surface it as a mismatch-equivalent.
//! - `@cert-authority` lines are ignored for raw-key match — we don't support CA-signed
//!   host trust in this PR (would need x509 / certificate validation logic).
//! - We consult an ordered list of files (typically `~/.ssh/known_hosts` then
//!   `~/.ssh/known_hosts2`); the first verdict that isn't `Absent` wins. Revoked from any
//!   file overrides everything.
//! - `learn_host` re-reads the target file before appending and rejects if a different key
//!   for the same host has appeared since the original probe (`Conflict`). That guards
//!   against the user running `ssh devbox` in another terminal between probe and confirm.

use data_encoding::BASE64_MIME;
use hmac::{Hmac, Mac};
use russh_keys::ssh_key::{HashAlg, PublicKey};
use sha1::Sha1;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Verdict from a single host-key lookup against one or more known_hosts files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostVerdict {
    /// Some file contains a matching `(host, key)` pair.
    Match,
    /// Some file contains a different key for this host. `line_no` is 1-indexed.
    Mismatch { line_no: usize, file: PathBuf },
    /// Some file revokes this host (any key) via `@revoked`. `line_no` is 1-indexed.
    Revoked { line_no: usize, file: PathBuf },
    /// No file mentions this host at all.
    Absent,
}

/// Errors from `learn_host`.
#[derive(Debug, thiserror::Error)]
pub enum LearnError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("conflicting entry already present at line {0}")]
    Conflict(usize),
    #[error("ssh-key encode: {0}")]
    Encode(String),
}

/// `~/.ssh/known_hosts` and `~/.ssh/known_hosts2` if they exist. Always returns the primary
/// path first even if it doesn't exist on disk — `learn_host` will create it on append.
pub fn default_files() -> Vec<PathBuf> {
    let Some(home) = std::env::var("HOME").ok().map(PathBuf::from) else {
        return Vec::new();
    };
    let primary = home.join(".ssh").join("known_hosts");
    let secondary = home.join(".ssh").join("known_hosts2");
    let mut out = vec![primary];
    if secondary.exists() {
        out.push(secondary);
    }
    out
}

/// Compute the SHA256:base64 fingerprint OpenSSH uses ("SHA256:..." form).
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Verify a host's presented key against an ordered list of known_hosts files.
///
/// Semantics:
/// - Revoked anywhere → Revoked (overrides everything).
/// - Match anywhere (and not Revoked) → Match.
/// - Mismatch in any file (and no Match, no Revoked) → Mismatch (first one wins).
/// - Otherwise Absent.
pub fn verify_host(
    files: &[&Path],
    host: &str,
    port: u16,
    presented: &PublicKey,
) -> HostVerdict {
    let mut first_mismatch: Option<HostVerdict> = None;
    let mut any_match = false;

    for file in files {
        let Ok(contents) = std::fs::read_to_string(file) else {
            // Missing / unreadable file → treat as absent for that file. Log once if we
            // grow real telemetry; for now the failure mode is silent (matches OpenSSH).
            continue;
        };
        match scan_file(&contents, host, port, presented) {
            FileScan::Revoked { line_no } => {
                return HostVerdict::Revoked {
                    line_no,
                    file: file.to_path_buf(),
                };
            }
            FileScan::Match => {
                any_match = true;
            }
            FileScan::Mismatch { line_no } => {
                if first_mismatch.is_none() {
                    first_mismatch = Some(HostVerdict::Mismatch {
                        line_no,
                        file: file.to_path_buf(),
                    });
                }
            }
            FileScan::Absent => {}
        }
    }

    if any_match {
        HostVerdict::Match
    } else if let Some(mm) = first_mismatch {
        mm
    } else {
        HostVerdict::Absent
    }
}

#[derive(Debug, Clone)]
enum FileScan {
    Match,
    Mismatch { line_no: usize },
    Revoked { line_no: usize },
    Absent,
}

fn scan_file(contents: &str, host: &str, port: u16, presented: &PublicKey) -> FileScan {
    let host_port = if port == 22 {
        host.to_string()
    } else {
        format!("[{}]:{}", host, port)
    };

    let mut any_match = false;
    let mut first_mismatch: Option<usize> = None;

    for (idx, raw_line) in contents.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Marker handling. `@revoked <hosts> <type> <key>`, `@cert-authority <hosts> ...`
        let (marker, rest) = if let Some(rest) = line.strip_prefix('@') {
            // Marker is the first whitespace-separated token after '@'.
            let mut parts = rest.splitn(2, char::is_whitespace);
            let m = parts.next().unwrap_or("");
            let r = parts.next().unwrap_or("");
            (Some(m), r.trim_start())
        } else {
            (None, line)
        };

        // Now `rest` should be `<host_field> <key_type> <base64key> [comment]`.
        let mut parts = rest.splitn(3, char::is_whitespace);
        let Some(host_field) = parts.next() else {
            continue;
        };
        let _key_type = parts.next();
        let key_b64_and_rest = parts.next();

        let host_match = match_hostname(&host_port, host_field);
        if !host_match {
            continue;
        }

        match marker {
            Some("revoked") => {
                // Revoked applies regardless of key value — return immediately.
                return FileScan::Revoked { line_no };
            }
            Some("cert-authority") => {
                // Skip — we don't support CA-based trust here.
                continue;
            }
            Some(_) => {
                // Unknown marker — be conservative and ignore.
                continue;
            }
            None => {}
        }

        // Compare key. Re-extract the key_type token because `rest`'s first token is the
        // host_field; we want the second whitespace-token as key_type and the third as the
        // base64 key body.
        let Some(key_b64) = key_b64_and_rest.and_then(|s| s.split_whitespace().next()) else {
            // Malformed line — ignore.
            continue;
        };

        let parsed = match russh_keys::parse_public_key_base64(key_b64) {
            Ok(k) => k,
            Err(_) => continue,
        };

        if parsed.algorithm() == presented.algorithm() && parsed == *presented {
            any_match = true;
        } else if first_mismatch.is_none() {
            first_mismatch = Some(line_no);
        }
    }

    if any_match {
        FileScan::Match
    } else if let Some(line_no) = first_mismatch {
        FileScan::Mismatch { line_no }
    } else {
        FileScan::Absent
    }
}

/// Returns true if the candidate `pattern` (a single host_field from a known_hosts line) matches
/// `host_port`. Handles plaintext, comma-separated lists, and `|1|salt|hash` HMAC-SHA1.
fn match_hostname(host_port: &str, pattern: &str) -> bool {
    for entry in pattern.split(',') {
        if entry.starts_with("|1|") {
            let mut parts = entry.split('|').skip(2);
            let Some(Ok(salt)) = parts.next().map(|p| BASE64_MIME.decode(p.as_bytes())) else {
                continue;
            };
            let Some(Ok(hash)) = parts.next().map(|p| BASE64_MIME.decode(p.as_bytes())) else {
                continue;
            };
            if let Ok(hmac) = Hmac::<Sha1>::new_from_slice(&salt) {
                if hmac.chain_update(host_port).verify_slice(&hash).is_ok() {
                    return true;
                }
            }
        } else if entry == host_port {
            return true;
        }
    }
    false
}

/// Append a host-key entry to `file` after re-verifying nothing conflicting was added since
/// the caller probed. Creates parent dirs and the file itself if absent.
pub fn learn_host(
    file: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), LearnError> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // TOCTOU guard: read existing file, scan for conflict on the same host.
    let existing = std::fs::read_to_string(file).unwrap_or_default();
    match scan_file(&existing, host, port, key) {
        FileScan::Match => {
            // Already learned (race with another `ssh` invocation). Treat as success.
            return Ok(());
        }
        FileScan::Mismatch { line_no } | FileScan::Revoked { line_no } => {
            return Err(LearnError::Conflict(line_no));
        }
        FileScan::Absent => {}
    }

    let mut handle = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(file)?;

    // Detect whether the file ends with a newline so we can prepend one if needed.
    let mut buf = [0u8; 1];
    let mut ends_in_newline = true; // empty file → no need for leading \n
    if let Ok(pos) = handle.seek(SeekFrom::End(0)) {
        if pos > 0 {
            handle.seek(SeekFrom::End(-1))?;
            handle.read_exact(&mut buf)?;
            ends_in_newline = buf[0] == b'\n';
        }
    }
    handle.seek(SeekFrom::End(0))?;

    let openssh_line = key
        .to_openssh()
        .map_err(|e| LearnError::Encode(e.to_string()))?;

    let host_field = if port == 22 {
        host.to_string()
    } else {
        format!("[{}]:{}", host, port)
    };

    let mut out = String::new();
    if !ends_in_newline {
        out.push('\n');
    }
    out.push_str(&host_field);
    out.push(' ');
    out.push_str(&openssh_line);
    out.push('\n');

    handle.write_all(out.as_bytes())?;
    handle.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_keys::parse_public_key_base64;
    use std::fs;
    use tempfile::TempDir;

    // Real ed25519 wire-format public keys — generated via `ssh-keygen` and verified to parse.
    // These are arbitrary fixtures, not credentials for any reachable host.
    const KEY_A_B64: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B_B64: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    const KEY_C_B64: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    fn key_a() -> PublicKey {
        parse_public_key_base64(KEY_A_B64).unwrap()
    }
    fn key_b() -> PublicKey {
        parse_public_key_base64(KEY_B_B64).unwrap()
    }
    fn key_c() -> PublicKey {
        parse_public_key_base64(KEY_C_B64).unwrap()
    }

    fn write_kh(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn lookup_plaintext_hit() {
        let dir = TempDir::new().unwrap();
        let p = write_kh(
            &dir,
            "known_hosts",
            &format!("box.example ssh-ed25519 {}\n", KEY_A_B64),
        );
        let v = verify_host(&[p.as_path()], "box.example", 22, &key_a());
        assert_eq!(v, HostVerdict::Match);
    }

    #[test]
    fn lookup_plaintext_miss() {
        let dir = TempDir::new().unwrap();
        let p = write_kh(
            &dir,
            "known_hosts",
            &format!("box.example ssh-ed25519 {}\n", KEY_A_B64),
        );
        let v = verify_host(&[p.as_path()], "other.example", 22, &key_a());
        assert_eq!(v, HostVerdict::Absent);
    }

    #[test]
    fn lookup_hashed_hit() {
        // `|1|<salt_b64>|<hmac_sha1(salt, host)_b64>` — generated via the same routine as
        // ssh-keyscan -H. We construct it inline so the fixture is reproducible without
        // running an external binary.
        let host = "box.example";
        let salt: [u8; 20] = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
        ];
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
        mac.update(host.as_bytes());
        let digest = mac.finalize().into_bytes();
        let salt_b64 = BASE64_MIME.encode(&salt).replace('\n', "").replace('\r', "");
        let hash_b64 = BASE64_MIME
            .encode(&digest)
            .replace('\n', "")
            .replace('\r', "");
        let line = format!(
            "|1|{}|{} ssh-ed25519 {}\n",
            salt_b64, hash_b64, KEY_A_B64
        );

        let dir = TempDir::new().unwrap();
        let p = write_kh(&dir, "known_hosts", &line);
        let v = verify_host(&[p.as_path()], host, 22, &key_a());
        assert_eq!(v, HostVerdict::Match);
    }

    #[test]
    fn lookup_hashed_miss_for_other_host() {
        let host = "box.example";
        let salt: [u8; 20] = [0xaa; 20];
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
        mac.update(host.as_bytes());
        let digest = mac.finalize().into_bytes();
        let salt_b64 = BASE64_MIME.encode(&salt).replace('\n', "");
        let hash_b64 = BASE64_MIME.encode(&digest).replace('\n', "");
        let line = format!(
            "|1|{}|{} ssh-ed25519 {}\n",
            salt_b64, hash_b64, KEY_A_B64
        );

        let dir = TempDir::new().unwrap();
        let p = write_kh(&dir, "known_hosts", &line);
        let v = verify_host(&[p.as_path()], "different.example", 22, &key_a());
        assert_eq!(v, HostVerdict::Absent);
    }

    #[test]
    fn lookup_port_namespaced_hit() {
        let dir = TempDir::new().unwrap();
        let p = write_kh(
            &dir,
            "known_hosts",
            &format!("[box.example]:2222 ssh-ed25519 {}\n", KEY_A_B64),
        );
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 2222, &key_a()),
            HostVerdict::Match
        );
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Absent
        );
    }

    #[test]
    fn lookup_comma_list_hit() {
        let dir = TempDir::new().unwrap();
        let p = write_kh(
            &dir,
            "known_hosts",
            &format!("box.example,1.2.3.4 ssh-ed25519 {}\n", KEY_A_B64),
        );
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
        assert_eq!(
            verify_host(&[p.as_path()], "1.2.3.4", 22, &key_a()),
            HostVerdict::Match
        );
    }

    #[test]
    fn lookup_comments_and_blanks_ignored() {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "# a comment\n\n   \n# box.example with stale key\nbox.example ssh-ed25519 {}\n",
            KEY_A_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
    }

    #[test]
    fn revoked_line_short_circuits_to_mismatch() {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "@revoked box.example ssh-ed25519 {}\n",
            KEY_A_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        match verify_host(&[p.as_path()], "box.example", 22, &key_a()) {
            HostVerdict::Revoked { line_no, .. } => assert_eq!(line_no, 1),
            v => panic!("expected Revoked, got {:?}", v),
        }
    }

    #[test]
    fn revoked_takes_priority_over_later_match() {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "@revoked box.example ssh-ed25519 {}\nbox.example ssh-ed25519 {}\n",
            KEY_B_B64, KEY_A_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        match verify_host(&[p.as_path()], "box.example", 22, &key_a()) {
            HostVerdict::Revoked { .. } => {}
            v => panic!("expected Revoked, got {:?}", v),
        }
    }

    #[test]
    fn cert_authority_lines_ignored_for_raw_key_match() {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "@cert-authority box.example ssh-ed25519 {}\nbox.example ssh-ed25519 {}\n",
            KEY_B_B64, KEY_A_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        // The cert-authority line is for KEY_B but should be skipped; the raw line for KEY_A
        // matches.
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
        // KEY_B should NOT match anything (the only mention is the ignored CA line).
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_b()),
            HostVerdict::Mismatch {
                line_no: 2,
                file: p.clone(),
            }
        );
    }

    #[test]
    fn multi_line_with_first_mismatch_then_match() {
        // OpenSSH semantics: any-match wins (you might rotate keys without removing the old
        // entry). We model that by returning Match if any line for this host matches.
        let dir = TempDir::new().unwrap();
        let body = format!(
            "box.example ssh-ed25519 {}\nbox.example ssh-ed25519 {}\n",
            KEY_B_B64, KEY_A_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
    }

    #[test]
    fn missing_file_treated_as_absent() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("does_not_exist");
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Absent
        );
    }

    #[test]
    fn malformed_lines_do_not_panic() {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "garbage\nbox.example garbage\nbox.example ssh-ed25519 NOT_VALID_BASE64==\nbox.example ssh-ed25519 {}\n",
            KEY_A_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
    }

    #[test]
    fn learn_appends_with_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("known_hosts");
        // File starts non-existent.
        learn_host(&p, "box.example", 22, &key_a()).unwrap();
        let body = fs::read_to_string(&p).unwrap();
        assert!(body.ends_with('\n'));
        assert!(body.contains("box.example"));
        assert!(body.contains("ssh-ed25519"));
        // Round-trip: verify_host should now Match.
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
    }

    #[test]
    fn learn_handles_missing_terminal_newline() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("known_hosts");
        // Pre-existing line WITHOUT a trailing newline.
        fs::write(&p, format!("other.example ssh-ed25519 {}", KEY_B_B64)).unwrap();
        learn_host(&p, "box.example", 22, &key_a()).unwrap();
        let body = fs::read_to_string(&p).unwrap();
        // Must still parse: the prior line and our new line must both be on their own line.
        assert!(body.contains("\nbox.example"));
        assert_eq!(
            verify_host(&[p.as_path()], "box.example", 22, &key_a()),
            HostVerdict::Match
        );
        assert_eq!(
            verify_host(&[p.as_path()], "other.example", 22, &key_b()),
            HostVerdict::Match
        );
    }

    #[test]
    fn learn_writes_port_bracketed_form_for_nonstandard_port() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("known_hosts");
        learn_host(&p, "box.example", 2222, &key_a()).unwrap();
        let body = fs::read_to_string(&p).unwrap();
        assert!(
            body.contains("[box.example]:2222"),
            "expected bracketed port form, got: {}",
            body
        );
    }

    #[test]
    fn learn_rejects_conflict_with_existing_different_key() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("known_hosts");
        fs::write(
            &p,
            format!("box.example ssh-ed25519 {}\n", KEY_B_B64),
        )
        .unwrap();
        match learn_host(&p, "box.example", 22, &key_a()) {
            Err(LearnError::Conflict(line)) => assert_eq!(line, 1),
            other => panic!("expected Conflict, got {:?}", other),
        }
    }

    #[test]
    fn learn_idempotent_when_same_key_already_present() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("known_hosts");
        fs::write(
            &p,
            format!("box.example ssh-ed25519 {}\n", KEY_A_B64),
        )
        .unwrap();
        // Should not error and should not append a duplicate line.
        learn_host(&p, "box.example", 22, &key_a()).unwrap();
        let body = fs::read_to_string(&p).unwrap();
        let lines = body.lines().filter(|l| l.contains("box.example")).count();
        assert_eq!(lines, 1, "expected single line, got: {}", body);
    }

    #[test]
    fn known_hosts2_consulted_when_primary_misses() {
        let dir = TempDir::new().unwrap();
        let primary = write_kh(&dir, "known_hosts", "# empty\n");
        let secondary = write_kh(
            &dir,
            "known_hosts2",
            &format!("box.example ssh-ed25519 {}\n", KEY_A_B64),
        );
        assert_eq!(
            verify_host(
                &[primary.as_path(), secondary.as_path()],
                "box.example",
                22,
                &key_a()
            ),
            HostVerdict::Match
        );
    }

    #[test]
    fn primary_revoked_overrides_secondary_match() {
        let dir = TempDir::new().unwrap();
        let primary = write_kh(
            &dir,
            "known_hosts",
            &format!("@revoked box.example ssh-ed25519 {}\n", KEY_B_B64),
        );
        let secondary = write_kh(
            &dir,
            "known_hosts2",
            &format!("box.example ssh-ed25519 {}\n", KEY_A_B64),
        );
        match verify_host(
            &[primary.as_path(), secondary.as_path()],
            "box.example",
            22,
            &key_a(),
        ) {
            HostVerdict::Revoked { .. } => {}
            v => panic!("expected Revoked, got {:?}", v),
        }
    }

    #[test]
    fn fingerprint_is_sha256_form() {
        let fp = fingerprint(&key_a());
        assert!(fp.starts_with("SHA256:"), "got: {}", fp);
    }

    #[test]
    fn mismatch_reports_correct_line_number() {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "# header\n# another comment\nbox.example ssh-ed25519 {}\n",
            KEY_B_B64
        );
        let p = write_kh(&dir, "known_hosts", &body);
        match verify_host(&[p.as_path()], "box.example", 22, &key_a()) {
            HostVerdict::Mismatch { line_no, .. } => assert_eq!(line_no, 3),
            v => panic!("expected Mismatch, got {:?}", v),
        }
    }

    // Anchor for KEY_C so the test compiles even though no match-test uses it directly;
    // helps catch a future refactor breaking the constant.
    #[test]
    fn key_c_parses() {
        let _ = key_c();
    }
}
