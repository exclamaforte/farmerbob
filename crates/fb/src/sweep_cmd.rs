//! `fb sweep <task> --winner <arm>` — plan the losers' cleanup.
//!
//! Bead farmerbob-oa5w.2: wire winner selection and sweeping. The pure
//! decisions live in [`farmerbob_core::selection`] (`sweep_list`,
//! `archive_ref`); this module is listing plus rendering. Read-only by
//! design: actual removal belongs to the sweep bead (farmerbob-pxf),
//! which owns worktree deletion and its guards. A plan that cannot
//! remove is still the wiring -- the decisions now run on real
//! worktrees instead of only in unit tests.

use std::io::Write;
use std::path::Path;

/// Arms with worktrees for a task. Directory names are shaped
/// `{task}--{arm}`; anything else -- and empty arm names -- is not a
/// candidate worktree and is ignored. Sorted, like the listing it comes
/// from.
pub fn arms_for_task(dirs: &[String], task: &str) -> Vec<String> {
    let mut arms: Vec<String> = dirs
        .iter()
        .filter_map(|d| d.split_once("--"))
        .filter(|(t, _)| *t == task)
        .map(|(_, a)| a.to_string())
        .filter(|a| !a.is_empty())
        .collect();
    arms.sort();
    arms
}

/// One loser's sweep entry: what to remove, and where it is preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepEntry {
    /// Arm name.
    pub arm: String,
    /// Worktree directory name.
    pub worktree: String,
    /// Archive ref the loser's work is preserved under.
    pub archive: String,
}

/// Plan the sweep: every candidate worktree except the winner's, with
/// archive refs. Unknown winners are refused, not swept around: sweeping
/// everything because of a typo is exactly the failure this names.
pub fn plan(task: &str, winner: &str, dirs: &[String]) -> Result<Vec<SweepEntry>, String> {
    let arms = arms_for_task(dirs, task);
    let candidates: Vec<farmerbob_core::selection::CandidateRef> = arms
        .iter()
        .map(|a| farmerbob_core::selection::CandidateRef(a.clone()))
        .collect();
    let won = farmerbob_core::selection::CandidateRef(winner.to_string());
    let losers = farmerbob_core::selection::sweep_list(&candidates, &won)
        .map_err(|e| format!("{task}: unknown winner {winner} ({e:?}); nothing swept"))?;
    Ok(losers
        .into_iter()
        .map(|loser| {
            let archive = farmerbob_core::selection::archive_ref(task, &loser);
            SweepEntry {
                arm: loser.0.clone(),
                worktree: format!("{task}--{}", loser.0),
                archive: archive.0,
            }
        })
        .collect())
}

/// List the sweep plan for a task's worktrees under `root`. Returns the
/// exit code: 0 with a plan (possibly an empty one -- a lone winner has
/// nothing to sweep), 1 when the worktrees cannot be read, 2 for an
/// unknown winner.
pub fn run(task: &str, winner: &str, root: &Path, out: &mut dyn Write) -> i32 {
    let mut dirs = Vec::new();
    match std::fs::read_dir(root) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && let Some(name) = entry.file_name().to_str()
                {
                    dirs.push(name.to_string());
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "error: cannot read {}: {e}", root.display());
            return 1;
        }
    }
    dirs.sort();
    match plan(task, winner, &dirs) {
        Ok(entries) => {
            if entries.is_empty() {
                let _ = writeln!(
                    out,
                    "{task}: winner {winner} stands alone; nothing to sweep"
                );
                return 0;
            }
            for entry in &entries {
                let _ = writeln!(out, "sweep {} (archive {})", entry.worktree, entry.archive);
            }
            let _ = writeln!(
                out,
                "{task}: sweep {} losers, keep {}",
                entries.len(),
                winner
            );
            0
        }
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// Only `{task}--{arm}` shapes count; arms sort; hyphenated arms and
    /// tasks with hyphens survive the split on the FIRST `--`.
    #[test]
    fn arms_parse_and_sort() {
        let dirs = dirs(&[
            "attention--codex-luna",
            "attention--gemini-38-flash",
            "other--x",
            "notadirectory",
            "attention--",
        ]);
        assert_eq!(
            arms_for_task(&dirs, "attention"),
            vec!["codex-luna", "gemini-38-flash"]
        );
        assert!(arms_for_task(&dirs, "missing").is_empty());
    }

    /// Unknown winners refuse with the name, sweeping nothing.
    #[test]
    fn unknown_winner_sweeps_nothing() {
        let dirs = dirs(&["t--a", "t--b"]);
        let err = plan("t", "ghost", &dirs).expect_err("unknown winner");
        assert!(err.contains("ghost"), "{err}");
    }

    /// Losers carry archive refs under the task.
    #[test]
    fn losers_carry_archive_refs() {
        let dirs = dirs(&["t--a", "t--b", "t--c"]);
        let entries = plan("t", "b", &dirs).expect("plan");
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|e| e.archive.starts_with("refs/fb/archive/t/"))
        );
        assert!(entries.iter().any(|e| e.arm == "a" && e.worktree == "t--a"));
    }

    /// End to end over a scratch worktrees root: plan prints, exit 0.
    #[test]
    fn run_lists_losers() {
        let root = std::env::temp_dir().join(format!("fb-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["t--a", "t--b", "other--x"] {
            std::fs::create_dir_all(root.join(d)).expect("worktree");
        }
        let mut out = Vec::new();
        assert_eq!(run("t", "a", &root, &mut out), 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("sweep t--b"), "{text}");
        assert!(!text.contains("t--a (archive"), "{text}");
        assert!(!text.contains("other--x"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A lone winner is an empty plan, not an error; a missing root is.
    #[test]
    fn lone_winner_and_missing_root() {
        let root = std::env::temp_dir().join(format!("fb-sweep-lone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("t--a")).expect("worktree");
        let mut out = Vec::new();
        assert_eq!(run("t", "a", &root, &mut out), 0);
        assert!(
            String::from_utf8_lossy(&out).contains("nothing to sweep"),
            "{out:?}"
        );
        let mut out = Vec::new();
        assert_eq!(run("t", "a", &root.join("nope"), &mut out), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
