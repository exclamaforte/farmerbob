//! Git hooks, with no shell anywhere in them.
//!
//! WHY THEY ARE SHAPED LIKE THIS. beads sets `core.hooksPath` to `.beads/hooks` and MANAGES
//! the files there -- it rewrites `.beads/hooks/pre-commit` whenever it likes. A section
//! appended after its "END BEADS INTEGRATION" marker was silently erased within minutes, and
//! the commit that followed went in unformatted while everything reported success. Editing a
//! file another tool regenerates is not installation.
//!
//! So `fb hooks install` takes ownership of `core.hooksPath` and forwards TO beads. Each
//! installed hook is a SYMLINK to this binary; git execs it, and `fb` recognises the hook by
//! the name it was invoked under. There is no generated shell in between, so there is
//! nothing in between for a bug to live in, and beads can regenerate its own directory as
//! often as it likes without touching ours.

use std::path::Path;

/// The git hooks this installs. A name outside this list is not a hook, so `fb` invoked
/// under any other name parses its arguments normally.
pub const HOOKS: &[&str] = &[
    "applypatch-msg",
    "commit-msg",
    "post-checkout",
    "post-commit",
    "post-merge",
    "post-rewrite",
    "pre-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "pre-push",
    "pre-rebase",
    "prepare-commit-msg",
];

/// The hook this binary was invoked as, if any.
pub fn invoked_as(argv0: &str) -> Option<&'static str> {
    let base = Path::new(argv0).file_name()?.to_str()?;
    HOOKS.iter().copied().find(|h| *h == base)
}

/// Which files rustfmt would rewrite, from its own `--check` output, repo-relative.
///
/// rustfmt prints `Diff in <absolute path>:<line>:` -- one line per hunk, so several per
/// file. It does NOT print "at line N", which an earlier version of this hook matched on:
/// the match silently produced nothing, the dirty list was empty, the hook exited 0 and
/// formatted nothing while reporting success. A check whose failure looks exactly like its
/// success, in the hook written to stop exactly that.
pub fn dirty_files(check_output: &str, repo_root: &str) -> Vec<String> {
    let prefix = format!("{}/", repo_root.trim_end_matches('/'));
    let mut v: Vec<String> = check_output
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("Diff in ")?;
            // Trailing `:<line>:` -- trim from the LAST colon pair, since a path may contain
            // a colon of its own.
            let rest = rest.strip_suffix(':')?;
            let (path, line) = rest.rsplit_once(':')?;
            line.parse::<u32>().ok()?;
            Some(path.strip_prefix(&prefix).unwrap_or(path).to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Split the rewritten files into the ones this commit already touches and the ones it does
/// not.
///
/// A rewritten file that was ALREADY staged is this commit's own work being normalised:
/// stage it and carry on. A rewritten file that was NOT staged means HEAD had already
/// drifted, and that belongs in its own normalisation commit rather than smuggled into this
/// one. Silently widening a merge commit to touch files it never meant to is how the drift
/// got invisible in the first place.
pub fn split_drift(dirty: &[String], staged: &[String]) -> (Vec<String>, Vec<String>) {
    dirty
        .iter()
        .cloned()
        .partition(|f| staged.iter().any(|s| s == f))
}

/// The staged Rust files a formatting gate applies to.
pub fn staged_rs(name_only: &str) -> Vec<String> {
    name_only
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("crates/") && l.ends_with(".rs"))
        .map(str::to_string)
        .collect()
}

fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(crate::paths::repo())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Keep the workspace rustfmt-clean, mechanically.
///
/// rustfmt.toml has demanded a permanently clean workspace since 2026-09-17. It drifted
/// dirty twice in the two days after, for one reason: every merge takes ONE file from ONE
/// arm, written in that arm's style, and nothing normalised it afterwards. That drift is not
/// cosmetic -- an arm handed a dirty base that then runs `cargo fmt`, which is ordinary
/// Rust, has every dirty file its build touched rewritten, and the scope gate counts each
/// one as a departure. One arm lost four runs that way.
fn pre_commit_fmt() -> i32 {
    let Some(names) = git(&["diff", "--cached", "--name-only", "--diff-filter=ACM"]) else {
        eprintln!(
            "pre-commit: git could not list the staged files -- refusing rather than guessing"
        );
        return 1;
    };
    let staged = staged_rs(&names);
    if staged.is_empty() {
        return 0;
    }
    let repo = crate::paths::repo();
    let check = std::process::Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(&repo)
        .output();
    let Ok(check) = check else {
        // Cargo could not RUN. Exiting 0 would let a missing toolchain read as a clean tree.
        eprintln!(
            "pre-commit: cargo is not runnable -- cannot verify formatting, refusing rather than guessing"
        );
        return 1;
    };
    let root = repo.to_string_lossy().into_owned();
    let dirty = dirty_files(&String::from_utf8_lossy(&check.stdout), &root);
    if dirty.is_empty() {
        return 0;
    }
    let fmt = std::process::Command::new("cargo")
        .args(["fmt", "--all"])
        .current_dir(&repo)
        .status();
    if !matches!(fmt, Ok(s) if s.success()) {
        eprintln!("pre-commit: cargo fmt failed");
        return 1;
    }
    let (restage, drifted) = split_drift(&dirty, &staged);
    for f in &restage {
        let _ = git(&["add", "--", f]);
    }
    if !drifted.is_empty() {
        eprintln!(
            "pre-commit: HEAD had drifted rustfmt-dirty in files this commit does not touch:"
        );
        for f in &drifted {
            eprintln!("  {f}");
        }
        eprintln!("They are now formatted in your working tree. Commit them on their own:");
        eprintln!("    git add -u && git commit -m 'Format the workspace'");
        eprintln!("then repeat this commit. Drift belongs in its own commit, not inside a merge.");
        return 1;
    }
    0
}

/// Run one hook: beads' hook of the same name first, with the same arguments, then ours.
pub fn run_hook(name: &str, args: &[String]) -> i32 {
    let beads = crate::paths::repo().join(".beads/hooks").join(name);
    if beads.is_file() {
        match std::process::Command::new(&beads)
            .args(args)
            .current_dir(crate::paths::repo())
            .status()
        {
            Ok(s) if s.success() => {}
            Ok(s) => return s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("{name}: the beads hook exists but could not be run: {e}");
                return 1;
            }
        }
    }
    if name == "pre-commit" {
        pre_commit_fmt()
    } else {
        0
    }
}

/// Every hook to install: the ones beads has, plus pre-commit whether or not it does.
pub fn to_install(beads_hooks: &[String]) -> Vec<String> {
    let mut v: Vec<String> = beads_hooks
        .iter()
        .filter(|n| HOOKS.contains(&n.as_str()))
        .cloned()
        .collect();
    v.push("pre-commit".to_string());
    v.sort();
    v.dedup();
    v
}

/// Install the hooks as symlinks to this binary and take `core.hooksPath`.
pub fn install() -> i32 {
    let repo = crate::paths::repo();
    let dir = repo.join("hooks");
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("cannot create {}", dir.display());
        return 1;
    }
    let beads: Vec<String> = std::fs::read_dir(repo.join(".beads/hooks"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| !n.starts_with('.'))
        .collect();
    let Ok(me) = std::env::current_exe() else {
        eprintln!("cannot locate this binary to link the hooks to");
        return 1;
    };
    let names = to_install(&beads);
    for n in &names {
        let link = dir.join(n);
        let _ = std::fs::remove_file(&link);
        if let Err(e) = std::os::unix::fs::symlink(&me, &link) {
            eprintln!("cannot link {}: {e}", link.display());
            return 1;
        }
    }
    if git(&["config", "core.hooksPath", "hooks"]).is_none() {
        eprintln!("cannot set core.hooksPath");
        return 1;
    }
    println!("core.hooksPath -> hooks");
    println!(
        "{} hook(s) linked to {}: {}",
        names.len(),
        me.display(),
        names.join(" ")
    );
    println!("forwarding to beads: {}", beads.join(" "));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rustfmt prints "Diff in <path>:<line>:", NOT "at line N". An earlier hook matched the
    /// wrong shape, found nothing, formatted nothing and exited 0 -- reporting success from
    /// inside the check written to stop exactly that.
    #[test]
    fn the_dirty_list_matches_what_rustfmt_actually_prints() {
        let out = "Diff in /r/crates/fb/src/a.rs:12:\nDiff in /r/crates/fb/src/a.rs:40:\n\
                   Diff in /r/crates/fb/src/b.rs:1:\n";
        assert_eq!(
            dirty_files(out, "/r"),
            vec!["crates/fb/src/a.rs", "crates/fb/src/b.rs"]
        );
    }

    /// The shape the old hook matched must still yield nothing, so the mistake cannot come
    /// back as a silent pass.
    #[test]
    fn the_shape_the_old_hook_matched_yields_nothing() {
        assert!(dirty_files("Diff in /r/crates/fb/src/a.rs at line 12:\n", "/r").is_empty());
        assert!(dirty_files("", "/r").is_empty());
    }

    /// A file this commit already touches is its own work being normalised: stage it. One it
    /// does not touch is pre-existing drift and belongs in its own commit -- widening a merge
    /// commit to files it never meant to touch is how the drift got invisible.
    #[test]
    fn pre_existing_drift_is_separated_from_this_commits_own_files() {
        let dirty = vec!["a.rs".to_string(), "b.rs".to_string()];
        let staged = vec!["a.rs".to_string()];
        let (restage, drifted) = split_drift(&dirty, &staged);
        assert_eq!(restage, vec!["a.rs"]);
        assert_eq!(drifted, vec!["b.rs"]);
    }

    /// The gate applies to workspace Rust, not to every staged file.
    #[test]
    fn only_workspace_rust_files_are_gated() {
        let out = "crates/fb/src/a.rs\nREADME.md\n.fb/prompts/x.md\ncrates/core/b.rs\nbuild.rs\n";
        assert_eq!(
            staged_rs(out),
            vec!["crates/fb/src/a.rs", "crates/core/b.rs"]
        );
    }

    /// pre-commit is installed whether or not beads has one -- it is the hook that carries
    /// the formatting gate, and beads regenerating its directory must not remove it.
    #[test]
    fn pre_commit_is_installed_even_when_beads_has_none() {
        assert!(to_install(&[]).contains(&"pre-commit".to_string()));
        let with = to_install(&["post-merge".to_string(), "pre-commit".to_string()]);
        assert_eq!(with.iter().filter(|n| *n == "pre-commit").count(), 1);
        assert!(with.contains(&"post-merge".to_string()));
    }

    /// A file beads leaves in its directory that is not a git hook is not installed as one.
    #[test]
    fn a_non_hook_in_the_beads_directory_is_not_installed() {
        assert!(!to_install(&["README".to_string()]).contains(&"README".to_string()));
    }

    /// `fb` invoked under a hook's name IS that hook; invoked under any other name it is
    /// the CLI and parses its arguments normally.
    #[test]
    fn the_binary_recognises_the_name_git_invoked_it_under() {
        assert_eq!(invoked_as("/r/hooks/pre-commit"), Some("pre-commit"));
        assert_eq!(invoked_as("hooks/post-merge"), Some("post-merge"));
        assert_eq!(invoked_as("/r/target/debug/fb"), None);
        assert_eq!(invoked_as("fb"), None);
    }
}
