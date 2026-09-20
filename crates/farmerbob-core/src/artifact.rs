//! Content-addressed artifact store.
//!
//! Bead farmerbob-7z6: agent logs, generated kernel source, bench JSON,
//! compiler output, and optional profiles must outlive their worktree --
//! sweeping the losers must not destroy the evidence for why they lost.
//!
//! The store lives under `<state>/artifacts` (`$FB_ARTIFACTS` overrides).
//! Bytes are stored once, named by their SHA-256 hex digest with two-level
//! fanout (`ab/cd/rest`), so identical content deduplicates naturally.
//! Pins (`pins/<name>`, one hash per line) attach artifacts to runs and
//! experiments; retention keeps whatever is pinned and collects the rest
//! past an age threshold (`fb artifact gc`).
//!
//! This module is pure: hashing, path layout, and retention decisions. I/O
//! lives in `fb::artifact_cmd`.

use std::path::PathBuf;

/// What kind of evidence an artifact is. Recorded beside the hash wherever
/// a pin references it; the store itself is kind-agnostic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Agent stdout/stderr logs.
    AgentLog,
    /// Generated kernel source (e.g. a candidate).
    KernelSource,
    /// Bench measurement JSON.
    BenchJson,
    /// Compiler output.
    CompilerLog,
    /// Prospector output (ncu/nsys).
    Profile,
    /// Anything else, with the caller's label.
    Other,
}

impl ArtifactKind {
    /// Parse a kind from the CLI `--kind` flag.
    pub fn parse(s: &str) -> Result<ArtifactKind, String> {
        match s {
            "agent-log" => Ok(ArtifactKind::AgentLog),
            "kernel-source" => Ok(ArtifactKind::KernelSource),
            "bench-json" => Ok(ArtifactKind::BenchJson),
            "compiler-log" => Ok(ArtifactKind::CompilerLog),
            "profile" => Ok(ArtifactKind::Profile),
            _ => Err(format!(
                "unknown artifact kind {s:?}: agent-log, kernel-source, bench-json, compiler-log, profile"
            )),
        }
    }

    /// The stable snake_case name used in pin records.
    pub fn name(self) -> &'static str {
        match self {
            ArtifactKind::AgentLog => "agent_log",
            ArtifactKind::KernelSource => "kernel_source",
            ArtifactKind::BenchJson => "bench_json",
            ArtifactKind::CompilerLog => "compiler_log",
            ArtifactKind::Profile => "profile",
            ArtifactKind::Other => "other",
        }
    }
}

/// SHA-256 hex digest of bytes. The store's only address.
pub fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex_of(&Sha256::digest(bytes))
}

fn hex_of(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// Path of an artifact's bytes under the store root: `<root>/ab/cd/rest`.
/// Empty or short hashes are rejected: an address must be a full digest.
pub fn store_path(root: &std::path::Path, hash: &str) -> Result<PathBuf, String> {
    if hash.len() < 5 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("not a content hash: {hash:?}"));
    }
    Ok(root.join(&hash[..2]).join(&hash[2..4]).join(&hash[4..]))
}

/// Path of a pin record: `<root>/pins/<name>`. Names are restricted to
/// alphanumerics, `-`, `_`, `.` so a pin can never escape the directory.
pub fn pin_path(root: &std::path::Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("invalid pin name: {name:?}"));
    }
    Ok(root.join("pins").join(name))
}

/// Parse a pin file's content: one `<hash> <kind>` per line, `#` comments
/// and blanks skipped. Malformed lines are reported, never skipped
/// silently: a pin that silently drops an entry would let `gc` destroy
/// evidence the operator believes is kept.
pub fn parse_pin(content: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (index, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next(), parts.next()) {
            (Some(hash), Some(kind), None) => {
                if hash.len() < 5 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("pin line {}: not a hash: {hash:?}", index + 1));
                }
                out.push((hash.to_string(), kind.to_string()));
            }
            _ => {
                return Err(format!(
                    "pin line {}: expected `<hash> <kind>`, got {line:?}",
                    index + 1
                ));
            }
        }
    }
    Ok(out)
}

/// Render pin entries back to file form.
pub fn render_pin(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    for (hash, kind) in entries {
        out.push_str(hash);
        out.push(' ');
        out.push_str(kind);
        out.push('\n');
    }
    out
}

/// Retention decision for one stored artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// Pinned, or younger than the threshold: keep.
    Keep,
    /// Unpinned and older than the threshold: collect.
    Collect,
}

/// Decide retention: pinned hashes are always kept; anything else is kept
/// while younger than `threshold_secs` and collected once older.
/// `age_secs` negative (clock skew) keeps: collecting on a broken clock
/// destroys evidence.
pub fn retain(pinned: bool, age_secs: i64, threshold_secs: i64) -> Retention {
    if pinned || age_secs < 0 || age_secs < threshold_secs {
        Retention::Keep
    } else {
        Retention::Collect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_sha256_hex() {
        // Empty-string SHA-256, pinned to catch an algorithm swap.
        assert_eq!(
            hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hash_bytes(b"abc").len(), 64);
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
    }

    #[test]
    fn store_path_fans_out_on_hash_prefix() {
        let root = std::path::Path::new("/store");
        let p = store_path(
            root,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        )
        .expect("valid hash");
        assert_eq!(
            p,
            root.join("e3")
                .join("b0")
                .join("c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn short_or_non_hex_hashes_are_rejected() {
        assert!(store_path(std::path::Path::new("/s"), "ab12").is_err());
        assert!(store_path(std::path::Path::new("/s"), &"zz".repeat(32)).is_err());
    }

    #[test]
    fn pin_names_cannot_escape_the_directory() {
        let root = std::path::Path::new("/store");
        assert_eq!(
            pin_path(root, "wave55").unwrap(),
            root.join("pins").join("wave55")
        );
        assert!(pin_path(root, "../evil").is_err());
        assert!(pin_path(root, "").is_err());
        assert!(pin_path(root, "a/b").is_err());
    }

    #[test]
    fn pin_round_trips_and_rejects_malformed_lines() {
        let entries = vec![(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            "kernel_source".to_string(),
        )];
        let back = parse_pin(&render_pin(&entries)).expect("round trip");
        assert_eq!(back, entries);
        assert!(parse_pin("justahash\n").is_err());
        assert!(parse_pin("zz kernel_source\n").is_err());
        assert!(parse_pin("# comment\n\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 bench_json\n")
            .expect("comments skipped")
            .len()
            == 1);
    }

    #[test]
    fn retention_keeps_pinned_young_and_skewed_collects_old() {
        assert_eq!(retain(true, 999_999, 10), Retention::Keep);
        assert_eq!(retain(false, 5, 10), Retention::Keep);
        assert_eq!(retain(false, 11, 10), Retention::Collect);
        assert_eq!(retain(false, -3, 10), Retention::Keep);
    }

    #[test]
    fn kinds_parse_and_name_stably() {
        assert_eq!(
            ArtifactKind::parse("bench-json"),
            Ok(ArtifactKind::BenchJson)
        );
        assert_eq!(ArtifactKind::BenchJson.name(), "bench_json");
        assert!(ArtifactKind::parse("everything").is_err());
    }
}
