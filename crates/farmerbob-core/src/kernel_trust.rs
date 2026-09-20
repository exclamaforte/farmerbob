//! The KernelBench trust boundary: which task files are pinned, and what
//! tampering looks like.
//!
//! Bead farmerbob-x81s.1: an ingested KernelBench task is one writable
//! directory holding both what the agent edits (`candidate.py`) and what it
//! is measured against (`ref/problem.py`, `verify.sh`, `bench.sh`,
//! `task.toml`, `setup.sh`). Nothing hashed any of it before or after a
//! run, so the cheapest winning edits were rewriting the scorer or slowing
//! the reference -- every downstream check then passes honestly, against a
//! reference the agent moved.
//!
//! The fix pins every task file except `candidate.py`: hash before dispatch,
//! mount the task read-only with the candidate the only writable path (see
//! `fb::isolated_cmd`, where bind ORDER is load-bearing and the writable
//! candidate binds LAST), and re-hash after. A run that altered a pinned
//! file is VOID -- it names the file and both hashes, and it gets NO SCORE.
//!
//! This module is pure: pin membership, snapshot comparison, and the void
//! report. Hashing reuses [`crate::artifact::hash_bytes`] (sha256); a second
//! hash implementation here would be the project's recurring
//! one-decision-two-implementations defect. I/O (walking the task dir,
//! writing the snapshot) lives in `fb::dispatch_cmd`.

/// The one file the agent may write, as a task-relative path.
pub const WRITABLE: &str = "candidate.py";

/// Whether a task-relative path is pinned (everything but the candidate).
///
/// Paths are compared as exact strings: `candidate.py` in a subdirectory is
/// a different file and stays pinned. Normalisation (leading `./`, absolute
/// paths) is the caller's job -- a path that does not spell exactly
/// `candidate.py` is not the writable file.
pub fn is_pinned(relative: &str) -> bool {
    relative != WRITABLE
}

/// One pinned file's content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    /// Task-relative path, e.g. `ref/problem.py`.
    pub file: String,
    /// sha256 hex over the file's bytes, per `artifact::hash_bytes`.
    pub hash: String,
}

/// Snapshot hashes over caller-supplied files.
///
/// `files` carries `(task-relative path, bytes)` pairs; the caller walked
/// the directory. Only pinned paths are hashed, in the order given, so the
/// snapshot is stable for a stable walk and never contains the candidate.
pub fn snapshot(files: &[(String, Vec<u8>)]) -> Vec<Pinned> {
    files
        .iter()
        .filter(|(path, _)| is_pinned(path))
        .map(|(path, bytes)| Pinned {
            file: path.clone(),
            hash: crate::artifact::hash_bytes(bytes),
        })
        .collect()
}

/// A pinned file whose hash moved during a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tamper {
    /// Task-relative path.
    pub file: String,
    /// Hash before dispatch.
    pub before: String,
    /// Hash after the run.
    pub after: String,
}

/// Compare a post-run snapshot against the pre-dispatch one.
///
/// Files present before and missing after (or added after) count as moved:
/// a deleted scorer is not an intact scorer, and a file the harness never
/// pinned appearing mid-run is not covered here but is a smell the caller
/// should surface separately. Order-independent; reports sorted by file so
/// two runs over the same movement produce identical output.
pub fn verify(before: &[Pinned], after: &[Pinned]) -> Vec<Tamper> {
    let mut out = Vec::new();
    for b in before {
        match after.iter().find(|a| a.file == b.file) {
            Some(a) if a.hash == b.hash => {}
            Some(a) => out.push(Tamper {
                file: b.file.clone(),
                before: b.hash.clone(),
                after: a.hash.clone(),
            }),
            None => out.push(Tamper {
                file: b.file.clone(),
                before: b.hash.clone(),
                after: "missing".to_string(),
            }),
        }
    }
    out.sort_by(|x, y| x.file.cmp(&y.file));
    out
}

/// Render one tamper as the VOID line: names the file and both hashes.
///
/// A tampered run is not a weak result and is never ranked: reporting it as
/// a low score would be the project's signature defect (a failure state
/// indistinguishable from a success state) in the place it matters most.
pub fn void_line(t: &Tamper) -> String {
    format!("VOID {} before={} after={}", t.file, t.before, t.after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, body: &str) -> (String, Vec<u8>) {
        (name.to_string(), body.as_bytes().to_vec())
    }

    /// Only the top-level candidate is writable; everything else pins.
    #[test]
    fn only_the_candidate_is_writable() {
        assert!(!is_pinned("candidate.py"));
        for p in [
            "ref/problem.py",
            "ref/baseline.json",
            "verify.sh",
            "bench.sh",
            "task.toml",
            "setup.sh",
        ] {
            assert!(is_pinned(p), "{p}");
        }
    }

    /// A same-named file in a subdirectory is not the writable file.
    #[test]
    fn a_nested_candidate_is_still_pinned() {
        assert!(is_pinned("sub/candidate.py"));
        assert!(is_pinned("./candidate.py"));
    }

    /// The snapshot never contains the candidate, whatever bytes it holds.
    #[test]
    fn the_snapshot_excludes_the_candidate() {
        let snap = snapshot(&[
            file("candidate.py", "agent code"),
            file("verify.sh", "echo"),
        ]);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].file, "verify.sh");
    }

    /// Hashes are the artifact store's sha256, not a second implementation.
    #[test]
    fn hashes_are_the_artifact_hash() {
        let snap = snapshot(&[file("verify.sh", "echo")]);
        assert_eq!(snap[0].hash, crate::artifact::hash_bytes(b"echo"));
    }

    /// An untouched run verifies clean.
    #[test]
    fn an_untouched_run_verifies_clean() {
        let files = vec![file("verify.sh", "echo"), file("ref/problem.py", "ref")];
        let before = snapshot(&files);
        let after = snapshot(&files);
        assert!(verify(&before, &after).is_empty());
    }

    /// The cheapest winning edit -- rewriting verify.sh to print ok -- is
    /// caught and names the file with both hashes.
    #[test]
    fn a_rewritten_scorer_is_void_with_both_hashes() {
        let before = snapshot(&[file("verify.sh", "real scorer")]);
        let after = snapshot(&[file("verify.sh", "{\"correct\":true}")]);
        let tampers = verify(&before, &after);
        assert_eq!(tampers.len(), 1);
        assert_eq!(tampers[0].file, "verify.sh");
        assert_eq!(
            tampers[0].before,
            crate::artifact::hash_bytes(b"real scorer")
        );
        assert_eq!(
            tampers[0].after,
            crate::artifact::hash_bytes(b"{\"correct\":true}")
        );
        let line = void_line(&tampers[0]);
        assert!(
            line.contains("VOID") && line.contains("verify.sh"),
            "{line}"
        );
        assert!(
            line.contains(&tampers[0].before) && line.contains(&tampers[0].after),
            "{line}"
        );
    }

    /// Slowing the reference (the nastiest edit: every downstream check
    /// still passes honestly) is caught the same way.
    #[test]
    fn a_slowed_reference_is_caught() {
        let before = snapshot(&[file("ref/problem.py", "fast ref")]);
        let after = snapshot(&[file("ref/problem.py", "slow ref")]);
        assert_eq!(verify(&before, &after).len(), 1);
    }

    /// A deleted scorer is moved, not intact.
    #[test]
    fn a_deleted_pinned_file_is_tamper() {
        let before = snapshot(&[file("verify.sh", "echo")]);
        let tampers = verify(&before, &[]);
        assert_eq!(tampers.len(), 1);
        assert_eq!(tampers[0].after, "missing");
    }

    /// Reports sort by file, so the VOID lines are deterministic.
    #[test]
    fn reports_sort_by_file() {
        let before = snapshot(&[file("b.sh", "1"), file("a.sh", "1")]);
        let after = snapshot(&[file("b.sh", "2"), file("a.sh", "2")]);
        let tampers = verify(&before, &after);
        assert_eq!(tampers[0].file, "a.sh");
        assert_eq!(tampers[1].file, "b.sh");
    }
}
