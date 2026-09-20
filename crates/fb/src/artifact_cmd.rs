//! `fb artifact`: store bytes by content hash, pin them to runs, collect garbage.
//!
//! Bead farmerbob-7z6. The impure end of
//! [`farmerbob_core::artifact`]: `store` reads a file, writes it under its
//! SHA-256 with fanout (skipping the write when the bytes are already
//! there -- that skip IS the dedup), and appends a `<hash> <kind>` line to
//! a pin. `gc` walks the store, keeps whatever any pin references or is
//! younger than the threshold, and deletes the rest (or just reports with
//! `--dry-run`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use farmerbob_core::artifact::{self, ArtifactKind, Retention};

/// Store one file's bytes under `--kind`, pinned to `--pin`.
/// Prints `{"hash":..,"kind":..,"pin":..,"deduplicated":bool}`.
/// Exit 0 stored, 2 usage, 4 unreadable.
pub fn store(file: &Path, kind: &str, pin: &str, root: &Path, out: &mut dyn Write) -> i32 {
    let kind = match ArtifactKind::parse(kind) {
        Ok(k) => k,
        Err(e) => {
            let _ = writeln!(out, "error: {e}");
            return 2;
        }
    };
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            let _ = writeln!(out, "error: cannot read {}: {e}", file.display());
            return 4;
        }
    };
    let hash = artifact::hash_bytes(&bytes);
    let dest = match artifact::store_path(root, &hash) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "error: {e}");
            return 4;
        }
    };
    let pin_file = match artifact::pin_path(root, pin) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "error: {e}");
            return 2;
        }
    };
    let deduplicated = dest.is_file();
    if let Some(parent) = dest.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let _ = writeln!(out, "error: cannot create {}: {e}", parent.display());
        return 4;
    }
    if !deduplicated && let Err(e) = std::fs::write(&dest, &bytes) {
        let _ = writeln!(out, "error: cannot write {}: {e}", dest.display());
        return 4;
    }
    if let Some(parent) = pin_file.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let _ = writeln!(out, "error: cannot create {}: {e}", parent.display());
        return 4;
    }
    let mut entries = read_pin(&pin_file);
    let line = (hash.clone(), kind.name().to_string());
    if !entries.contains(&line) {
        entries.push(line);
        if let Err(e) = std::fs::write(&pin_file, artifact::render_pin(&entries)) {
            let _ = writeln!(out, "error: cannot write {}: {e}", pin_file.display());
            return 4;
        }
    }
    let _ = writeln!(
        out,
        "{}",
        serde_json::json!({"hash": hash, "kind": kind.name(), "pin": pin, "deduplicated": deduplicated})
    );
    0
}

fn read_pin(path: &Path) -> Vec<(String, String)> {
    match std::fs::read_to_string(path) {
        Ok(content) => artifact::parse_pin(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Collect unpinned artifacts older than `--older-than-secs` (default 30d).
/// Prints `{"kept":n,"collected":n,"dry_run":bool}`. Never deletes on a
/// malformed pin: an unreadable pin keeps everything, loudly.
pub fn gc(root: &Path, older_than_secs: i64, dry_run: bool, out: &mut dyn Write) -> i32 {
    let mut pinned: BTreeSet<String> = BTreeSet::new();
    let pins_dir = root.join("pins");
    let mut pin_broken = false;
    if pins_dir.is_dir() {
        let mut names: Vec<PathBuf> = std::fs::read_dir(&pins_dir)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        names.sort();
        for path in names {
            match std::fs::read_to_string(&path) {
                Ok(content) => match artifact::parse_pin(&content) {
                    Ok(entries) => {
                        pinned.extend(entries.into_iter().map(|(h, _)| h));
                    }
                    Err(e) => {
                        eprintln!("warning: ignoring malformed pin {}: {e}", path.display());
                        pin_broken = true;
                    }
                },
                Err(e) => {
                    eprintln!("warning: cannot read pin {}: {e}", path.display());
                    pin_broken = true;
                }
            }
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut kept = 0u64;
    let mut collected = 0u64;
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &pins_dir, &mut files);
    files.sort();
    for path in files {
        let hash = match hash_of_path(root, &path) {
            Some(h) => h,
            None => continue,
        };
        let age = file_age_secs(&path, now);
        let decision = if pin_broken {
            Retention::Keep
        } else {
            artifact::retain(pinned.contains(&hash), age, older_than_secs)
        };
        match decision {
            Retention::Keep => kept += 1,
            Retention::Collect => {
                if dry_run || std::fs::remove_file(&path).is_ok() {
                    collected += 1;
                } else {
                    kept += 1;
                }
            }
        }
    }
    let _ = writeln!(
        out,
        "{}",
        serde_json::json!({"kept": kept, "collected": collected, "dry_run": dry_run})
    );
    0
}

fn collect_files(dir: &Path, pins_dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path == *pins_dir || path.starts_with(pins_dir) {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, pins_dir, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Reconstruct the hash from a fanout path `<root>/ab/cd/rest`.
fn hash_of_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = rel.components();
    let (a, b, rest) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (a, b, rest) = (
        a.as_os_str().to_str()?,
        b.as_os_str().to_str()?,
        rest.as_os_str().to_str()?,
    );
    if a.len() != 2 || b.len() != 2 || rest.is_empty() {
        return None;
    }
    let hash = format!("{a}{b}{rest}");
    if hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hash)
    } else {
        None
    }
}

fn file_age_secs(path: &Path, now: i64) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| now - d.as_secs() as i64)
        .unwrap_or(-1)
}

/// One stored artifact with its retention decision, for reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// Content hash.
    pub hash: String,
    /// Whether any pin references it.
    pub pinned: bool,
    /// What `gc` would do with it.
    pub retention: Retention,
}

/// List every stored artifact with pin state and retention. Pure I/O
/// wrapper over the core decision; used by tests and future reports.
pub fn list(root: &Path, older_than_secs: i64, now: i64) -> Vec<Listing> {
    let mut pinned: BTreeSet<String> = BTreeSet::new();
    let pins_dir = root.join("pins");
    if pins_dir.is_dir()
        && let Ok(rd) = std::fs::read_dir(&pins_dir)
    {
        for entry in rd.flatten() {
            if let Ok(content) = std::fs::read_to_string(entry.path())
                && let Ok(entries) = artifact::parse_pin(&content)
            {
                pinned.extend(entries.into_iter().map(|(h, _)| h));
            }
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, &pins_dir, &mut files);
    files.sort();
    let mut by_hash: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in files {
        if let Some(hash) = hash_of_path(root, &path) {
            by_hash.entry(hash).or_insert(path);
        }
    }
    by_hash
        .into_iter()
        .map(|(hash, path)| {
            let is_pinned = pinned.contains(&hash);
            Listing {
                retention: artifact::retain(is_pinned, file_age_secs(&path, now), older_than_secs),
                hash,
                pinned: is_pinned,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fb-artifact-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn store_deduplicates_identical_bytes() {
        let root = scratch();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        std::fs::write(&a, "same bytes").expect("write");
        std::fs::write(&b, "same bytes").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&a, "kernel-source", "run1", &root, &mut out), 0);
        let first: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(first["deduplicated"], false);
        let mut out = Vec::new();
        assert_eq!(store(&b, "kernel-source", "run1", &root, &mut out), 0);
        let second: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(second["deduplicated"], true);
        assert_eq!(first["hash"], second["hash"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn store_rejects_bad_kind_pin_and_missing_file() {
        let root = scratch();
        let mut out = Vec::new();
        assert_eq!(
            store(&root.join("gone"), "kernel-source", "p", &root, &mut out),
            4
        );
        let f = root.join("f.txt");
        std::fs::write(&f, "x").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&f, "everything", "p", &root, &mut out), 2);
        let mut out = Vec::new();
        assert_eq!(store(&f, "bench-json", "../evil", &root, &mut out), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_collects_old_unpinned_and_keeps_pinned() {
        let root = scratch();
        let f = root.join("k.py");
        std::fs::write(&f, "kernel").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&f, "kernel-source", "keep", &root, &mut out), 0);
        let g = root.join("o.log");
        std::fs::write(&g, "orphan").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&g, "agent-log", "keep", &root, &mut out), 0);
        // Unpin the orphan by rewriting the pin with only the kernel hash.
        let hash_k = artifact::hash_bytes(b"kernel");
        std::fs::write(
            root.join("pins").join("keep"),
            artifact::render_pin(&[(hash_k, "kernel_source".to_string())]),
        )
        .expect("rewrite pin");
        let mut out = Vec::new();
        assert_eq!(gc(&root, -1, false, &mut out), 0);
        let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["collected"], 1);
        assert_eq!(report["kept"], 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_dry_run_collects_nothing() {
        let root = scratch();
        let f = root.join("k.py");
        std::fs::write(&f, "kernel").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&f, "kernel-source", "keep", &root, &mut out), 0);
        let hash_k = artifact::hash_bytes(b"kernel");
        std::fs::write(
            root.join("pins").join("keep"),
            artifact::render_pin(&[(hash_k, "kernel_source".to_string())]),
        )
        .expect("rewrite pin");
        // Age threshold 0 with dry run: would collect the pinned? No --
        // pinned is always kept. Rewrite pin empty to unpin everything.
        std::fs::write(root.join("pins").join("keep"), "").expect("empty pin");
        let mut out = Vec::new();
        assert_eq!(gc(&root, -1, true, &mut out), 0);
        let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["collected"], 1);
        // Dry run: the file is still there.
        assert_eq!(list(&root, -1, 0).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_with_malformed_pin_keeps_everything() {
        let root = scratch();
        let f = root.join("k.py");
        std::fs::write(&f, "kernel").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&f, "kernel-source", "keep", &root, &mut out), 0);
        std::fs::write(root.join("pins").join("keep"), "garbage line here\n").expect("break pin");
        let mut out = Vec::new();
        assert_eq!(gc(&root, -1, false, &mut out), 0);
        let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(report["collected"], 0);
        assert_eq!(report["kept"], 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_reports_pin_state_and_retention() {
        let root = scratch();
        let f = root.join("k.py");
        std::fs::write(&f, "kernel").expect("write");
        let mut out = Vec::new();
        assert_eq!(store(&f, "kernel-source", "keep", &root, &mut out), 0);
        let rows = list(&root, 10_000, i64::MAX / 2);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].pinned);
        assert_eq!(rows[0].retention, Retention::Keep);
        let _ = std::fs::remove_dir_all(&root);
    }
}
