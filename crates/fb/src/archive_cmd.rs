//! `fb archive [worktree...]` — capture a candidate's work before anything reaps it.
//!
//! A candidate's work lives ONLY in its worktree, uncommitted. Nothing in this harness ever
//! commits it, so `git worktree remove --force` destroys it with no trace -- and dispatch
//! runs that at the start of every run, speccheck runs it, and an adjudicator runs it by
//! hand to clean a field before requeueing.
//!
//! It happened: score-delta/codex-luna wrote 272 lines against its declared target, scored
//! OUT-OF-SCOPE on seventeen cargo-fmt departures, the worktree was reaped to requeue the
//! task, and the work was gone. The branch survived and pointed at the BASE commit, because
//! nothing had ever been committed to it.
//!
//! A measurement harness that cannot reproduce the artefact it measured is not a
//! measurement harness.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Paths the HARNESS removes from every worktree. A diff of these is provisioning, not
/// work, and including them made the first run 413MB across 457 worktrees -- almost all of
/// it the harness talking to itself.
pub const HARNESS_OWNED: [&str; 6] = [
    ".agents",
    ".beads",
    ".cursor",
    ".codex",
    "CLAUDE.md",
    "AGENTS.md",
];

/// The git pathspecs that exclude the harness's own bookkeeping while keeping the one file
/// under `.fb/` the candidate writes.
pub fn exclusions() -> Vec<String> {
    let mut v: Vec<String> = HARNESS_OWNED
        .iter()
        .map(|p| format!(":(exclude){p}"))
        .collect();
    // `.fb/` is the hidden-suite strip plus every other task's prompt.
    //
    // handoff.md is NOT re-included here, and cannot be: git applies exclude pathspecs
    // last, so a later positive `:(glob).fb/handoff.md` does not rescue a path already
    // excluded by `.fb/**`. The shell version claimed in a comment that it kept the
    // handoff and did not -- verified by probing git directly, after a test written to
    // pin the claim failed. It is archived by a second diff instead.
    v.push(":(exclude,glob).fb/**".to_string());
    v
}

/// The one path under `.fb/` the candidate writes, archived separately because it cannot be
/// re-included into a diff that excludes its directory.
pub const HANDOFF: &str = ".fb/handoff.md";

/// What archiving one worktree produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Archived {
    /// A patch was written, with this many lines.
    Wrote(usize),
    /// The run changed nothing worth keeping.
    NoOp,
    /// The worktree is not there.
    Gone,
    /// git refused. NOT a no-op: an empty stdout from a FAILED command and an empty stdout
    /// from a clean worktree are the same bytes and opposite facts, and this command
    /// reported both as "nothing to archive" until a real worktree with 792 lines of work
    /// came back as a no-op run.
    Unreadable(String),
}

/// Archive one worktree's diff, including anything it created untracked.
///
/// An arm that CREATES its target leaves it untracked, and a plain `git diff HEAD` would
/// report nothing at all -- exactly the case where the work is least recoverable. The index
/// is marked intent-to-add first, which is per-worktree and does not touch this repository.
pub fn archive_one(wt: &Path, out: &Path) -> Archived {
    if !wt.is_dir() {
        return Archived::Gone;
    }
    let _ = Command::new("git")
        .current_dir(wt)
        .args(["add", "-AN", "."])
        .output();
    // The leading "." is load bearing. A positive pathspec RESTRICTS the set, so the
    // `:(glob).fb/handoff.md` re-inclusion below would narrow the whole diff to that one
    // file without something matching everything first. Dropping it produced an empty patch
    // for a worktree that had plainly changed, and the empty patch read as a no-op run.
    let mut args: Vec<String> = ["diff", "HEAD", "--binary", "--", "."]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(exclusions());
    let Ok(output) = Command::new("git").current_dir(wt).args(&args).output() else {
        return Archived::Unreadable("cannot run git".to_string());
    };
    if !output.status.success() {
        return Archived::Unreadable(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("git refused")
                .to_string(),
        );
    }
    let mut patch = output.stdout;
    // The handoff, appended. Concatenated patches remain a valid patch.
    if let Ok(h) = Command::new("git")
        .current_dir(wt)
        .args(["diff", "HEAD", "--binary", "--", HANDOFF])
        .output()
    {
        patch.extend_from_slice(&h.stdout);
    }
    if patch.is_empty() {
        return Archived::NoOp;
    }
    if fs::write(out, &patch).is_err() {
        return Archived::Gone;
    }
    Archived::Wrote(patch.iter().filter(|b| **b == b'\n').count())
}

/// Archive every worktree, or the named ones.
pub fn run(names: &[String]) -> i32 {
    let wt_root = crate::paths::worktrees();
    let archive = crate::paths::state().join("archive");
    if fs::create_dir_all(&archive).is_err() {
        eprintln!("cannot create {}", archive.display());
        return 1;
    }
    let targets: Vec<PathBuf> = if names.is_empty() {
        match fs::read_dir(&wt_root) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect(),
            Err(e) => {
                eprintln!("cannot read {}: {e}", wt_root.display());
                return 1;
            }
        }
    } else {
        names.iter().map(|n| wt_root.join(n)).collect()
    };
    let mut wrote = 0;
    let mut rc = 0;
    for wt in targets {
        let name = wt
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let out = archive.join(format!("{name}.patch"));
        match archive_one(&wt, &out) {
            Archived::Wrote(lines) => {
                println!("  {name}: {lines} lines -> {}", out.display());
                wrote += 1;
            }
            Archived::NoOp => println!("  {name}: nothing to archive (no-op run)"),
            Archived::Gone => println!("  {name}: gone"),
            Archived::Unreadable(why) => {
                println!("  {name}: NOT archived -- {why}");
                rc = 1;
            }
        }
    }
    println!("archived from {wrote} worktree(s) -> {}", archive.display());
    rc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fb-archive-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
        d
    }

    fn repo(at: &Path) {
        let run = |args: &[&str]| {
            let _ = Command::new("git").current_dir(at).args(args).output();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        let _ = fs::write(at.join("kept.rs"), "one\n");
        run(&["add", "-A"]);
        run(&["commit", "-qm", "base"]);
    }

    /// A CREATED file is untracked, so `git diff HEAD` alone reports nothing -- which is
    /// exactly the case where the work is least recoverable. It must still be archived.
    #[test]
    fn a_created_file_is_archived_although_the_diff_cannot_see_it() {
        let d = scratch("created");
        let wt = d.join("wt");
        let _ = fs::create_dir_all(&wt);
        repo(&wt);
        let _ = fs::write(wt.join("new.rs"), "a\nb\n");
        let out = d.join("p.patch");
        assert!(matches!(archive_one(&wt, &out), Archived::Wrote(_)));
        let patch = fs::read_to_string(&out).unwrap_or_default();
        assert!(patch.contains("new.rs"), "{patch}");
        let _ = fs::remove_dir_all(&d);
    }

    /// The harness's OWN deletions are not the arm's work. Without the exclusions the first
    /// run was 413MB across 457 worktrees, nearly all of it .beads and .agents.
    #[test]
    fn the_harness_own_bookkeeping_is_excluded() {
        let d = scratch("excl");
        let wt = d.join("wt");
        let _ = fs::create_dir_all(wt.join(".beads"));
        repo(&wt);
        let _ = fs::write(wt.join(".beads/x.json"), "{}\n");
        let out = d.join("p.patch");
        let r = archive_one(&wt, &out);
        assert_eq!(r, Archived::NoOp, "only harness paths changed");
        let _ = fs::remove_dir_all(&d);
    }

    /// `.fb/handoff.md` is the exception: the candidate writes it and it is the only
    /// surviving account of what the arm thought it was doing.
    #[test]
    fn the_handoff_survives_the_fb_exclusion() {
        let d = scratch("handoff");
        let wt = d.join("wt");
        let _ = fs::create_dir_all(wt.join(".fb/prompts"));
        repo(&wt);
        let _ = fs::write(wt.join(".fb/prompts/other.md"), "not mine\n");
        let _ = fs::write(wt.join(".fb/handoff.md"), "what I did\n");
        let out = d.join("p.patch");
        assert!(matches!(archive_one(&wt, &out), Archived::Wrote(_)));
        let patch = fs::read_to_string(&out).unwrap_or_default();
        assert!(patch.contains("handoff.md"), "{patch}");
        assert!(!patch.contains("other.md"), "{patch}");
        let _ = fs::remove_dir_all(&d);
    }

    /// A directory that is not a git repository is UNREADABLE, not a no-op. An empty
    /// stdout from a failed command and an empty stdout from a clean worktree are the same
    /// bytes and opposite facts; conflating them reported a worktree holding 792 lines of
    /// work as "nothing to archive".
    #[test]
    fn a_directory_that_is_not_a_repo_is_unreadable_not_empty() {
        let d = scratch("notrepo");
        let wt = d.join("wt");
        let _ = fs::create_dir_all(&wt);
        let _ = fs::write(wt.join("a.rs"), "x\n");
        assert!(matches!(
            archive_one(&wt, &d.join("p.patch")),
            Archived::Unreadable(_)
        ));
        let _ = fs::remove_dir_all(&d);
    }

    /// A worktree that changed nothing is a NO-OP, distinct from one that is not there.
    /// Both used to print the same thing, and "nothing to archive" is a different fact from
    /// "I could not look".
    #[test]
    fn a_noop_and_a_missing_worktree_are_different_answers() {
        let d = scratch("noop");
        let wt = d.join("wt");
        let _ = fs::create_dir_all(&wt);
        repo(&wt);
        assert_eq!(archive_one(&wt, &d.join("p.patch")), Archived::NoOp);
        assert_eq!(
            archive_one(&d.join("absent"), &d.join("q.patch")),
            Archived::Gone
        );
        let _ = fs::remove_dir_all(&d);
    }
}
