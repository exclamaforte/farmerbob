//! `fb isolated <worktree> <cmd...>` — run a command with sibling worktrees hidden.
//!
//! Ported from `fb-isolated`, which had to be a real executable rather than a shell
//! function because systemd-run execs its argument directly and reports "Failed to find
//! executable fb_isolated" for a function. Two runs died at 0s that way.
//!
//! The bwrap argv is the whole of this file's substance, and it is assembled by a pure
//! function so it can be tested without spawning a namespace. Two copies of that argv
//! existed before -- an executable on the dispatch path and a shell function on the
//! critique path -- and a guard added to the FUNCTION was verified by calling the function,
//! which is the copy that had been fixed. It held nowhere that mattered: every wave
//! dispatched afterwards went through the other copy and destroyed worktrees exactly as
//! before, taking the healthy count from 78 to 11.

use std::path::{Path, PathBuf};

/// The bwrap argv, or `None` when confinement is off or unavailable.
///
/// Order is load-bearing: bwrap applies binds in argv order and a later one wins, so the
/// writable re-binds must come after the read-only binds they punch through.
pub fn bwrap_argv(
    wt: &Path,
    root: &Path,
    admin: &Path,
    repo: &Path,
    cmd: &[String],
) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut a: Vec<String> = ["bwrap", "--dev-bind", "/", "/"]
        .iter()
        .map(|x| x.to_string())
        .collect();

    // THE ORCHESTRATOR'S OWN WORKING TREE IS READ-ONLY IN HERE.
    //
    // `--dev-bind / /` binds the whole filesystem READ-WRITE. The tmpfs below hides sibling
    // worktrees from each other, which is what this was written for, but nothing stopped an
    // arm writing to the repository itself -- its specs, its sources.toml, its source.
    //
    // Not hypothetical: decl-cmd/glm-53-flash resumed a session whose working directory was
    // the main repo rather than the worktree and wrote crates/fb/src/decl_cmd.rs and a
    // `mod decl_cmd;` line STRAIGHT INTO THE ORCHESTRATOR'S TREE. It scored NO-OP -- the
    // worktree was untouched -- while its log read "All three gates pass in
    // /home/gabe/Documents/farmerbob (the repo this task targets)".  (bead farmerbob-p3r)
    //
    // `.git` stays WRITABLE. A worktree shares the repository's object store, so an arm that
    // commits on its own branch writes objects into $repo/.git, and the scorer measures
    // committed work deliberately (farmerbob-qv6u).
    if repo.is_dir() {
        a.extend(["--ro-bind".into(), s(repo), s(repo)]);
        let git = repo.join(".git");
        if git.is_dir() {
            a.extend(["--bind".into(), s(&git), s(&git)]);
        }
    }

    a.extend(["--tmpfs".into(), s(root)]);
    a.extend(["--bind".into(), s(wt), s(wt)]);

    // The tmpfs hides every sibling worktree, which is the point -- but it also hides them
    // from GIT, whose admin directories live in the shared .git and stay writable under
    // --dev-bind. Inside this namespace `git worktree prune` sees ~200 worktrees whose
    // "gitdir file points to non-existent location" and deletes every one of their admin
    // directories. The files survive; the repository forgets they are worktrees, so
    // `git diff` answers "fatal: not a git repository" forever after and the scorer reads
    // that as the arm having written nothing.  (bead farmerbob-13p)
    if admin.is_dir() {
        a.extend(["--ro-bind".into(), s(admin), s(admin)]);
        if let Some(name) = wt.file_name() {
            let own = admin.join(name);
            if own.is_dir() {
                a.extend(["--bind".into(), s(&own), s(&own)]);
            }
        }
    }

    a.push("--".into());
    a.extend(cmd.iter().cloned());
    a
}

/// Where the repository sits, given the git admin directory.
pub fn repo_of(admin: &Path) -> PathBuf {
    admin
        .to_string_lossy()
        .strip_suffix("/.git/worktrees")
        .map(PathBuf::from)
        .unwrap_or_else(|| admin.to_path_buf())
}

fn have_bwrap() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("bwrap").is_file()))
        .unwrap_or(false)
}

/// Run the command, confined unless confinement is switched off or unavailable.
pub fn run(wt: &Path, cmd: &[String]) -> i32 {
    let Some((program, args)) = cmd.split_first() else {
        eprintln!("fb isolated: no command to run");
        return 2;
    };
    let status = if std::env::var("FB_NO_ISOLATE").as_deref() == Ok("1") || !have_bwrap() {
        std::process::Command::new(program).args(args).status()
    } else {
        let admin = std::env::var_os("FB_GIT_WORKTREES")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::paths::repo().join(".git/worktrees"));
        let repo = std::env::var_os("FB_REPO")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_of(&admin));
        let argv = bwrap_argv(wt, &crate::paths::worktrees(), &admin, &repo, cmd);
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .status()
    };
    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            // The command did not RUN. Reporting 0 would let a confinement failure read as
            // an arm that did its work and wrote nothing.
            eprintln!("fb isolated: could not run {program}: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its OWN base directory. One shared path was racy: these run in
    /// parallel and each `remove_dir_all` pulled the ground out from under the others,
    /// which failed one test at random rather than the test whose fixtures went missing.
    fn dirs(who: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("fb-isolated-{who}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let admin = repo.join(".git/worktrees");
        let root = base.join("worktrees");
        std::fs::create_dir_all(admin.join("task--arm")).unwrap();
        std::fs::create_dir_all(root.join("task--arm")).unwrap();
        (repo, admin, root)
    }

    fn pairs(a: &[String], flag: &str) -> Vec<String> {
        a.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].clone())
            .collect()
    }

    /// The repository is READ-ONLY inside the namespace. An arm that resumes a session whose
    /// working directory is the main repo once wrote a source file and a `mod` line straight
    /// into the orchestrator's tree, scored NO-OP, and reported success.
    #[test]
    fn the_orchestrators_tree_is_read_only() {
        let (repo, admin, root) = dirs("ro");
        let a = bwrap_argv(
            &root.join("task--arm"),
            &root,
            &admin,
            &repo,
            &["true".into()],
        );
        assert!(
            pairs(&a, "--ro-bind").contains(&repo.to_string_lossy().into_owned()),
            "{a:?}"
        );
    }

    /// `.git` stays WRITABLE inside it: a worktree shares the object store, and an arm that
    /// commits on its own branch writes there. The scorer measures committed work.
    #[test]
    fn the_object_store_stays_writable_inside_the_read_only_repo() {
        let (repo, admin, root) = dirs("git");
        let a = bwrap_argv(
            &root.join("task--arm"),
            &root,
            &admin,
            &repo,
            &["true".into()],
        );
        let git = repo.join(".git").to_string_lossy().into_owned();
        assert!(pairs(&a, "--bind").contains(&git), "{a:?}");
        // bwrap applies binds in order and the last wins, so the writable re-bind must come
        // AFTER the read-only bind it punches through.
        let ro = a
            .iter()
            .position(|x| x == &repo.to_string_lossy().into_owned())
            .unwrap();
        let rw = a.iter().position(|x| x == &git).unwrap();
        assert!(
            rw > ro,
            "the .git re-bind must follow the repo ro-bind: {a:?}"
        );
    }

    /// The tmpfs hides sibling worktrees from GIT too, and `git worktree prune` inside the
    /// namespace then deletes every admin directory it cannot see -- the repository forgets
    /// they are worktrees and `git diff` answers "fatal: not a git repository" forever
    /// after, which the scorer reads as the arm having written nothing.
    #[test]
    fn the_worktree_admin_directory_is_protected() {
        let (repo, admin, root) = dirs("admin");
        let wt = root.join("task--arm");
        let a = bwrap_argv(&wt, &root, &admin, &repo, &["true".into()]);
        assert!(
            pairs(&a, "--ro-bind").contains(&admin.to_string_lossy().into_owned()),
            "{a:?}"
        );
    }

    /// THIS run's own admin entry is bound back writable, or its own git stops working.
    #[test]
    fn this_runs_own_admin_entry_stays_writable() {
        let (repo, admin, root) = dirs("own");
        let wt = root.join("task--arm");
        let a = bwrap_argv(&wt, &root, &admin, &repo, &["true".into()]);
        let own = admin.join("task--arm").to_string_lossy().into_owned();
        assert!(pairs(&a, "--bind").contains(&own), "{a:?}");
        let ro = a
            .iter()
            .position(|x| x == &admin.to_string_lossy().into_owned())
            .unwrap();
        let rw = a.iter().position(|x| x == &own).unwrap();
        assert!(
            rw > ro,
            "the own-entry re-bind must follow the admin ro-bind: {a:?}"
        );
    }

    /// The worktree itself is writable, and the tmpfs over their shared root is what hides
    /// its siblings.
    #[test]
    fn the_worktree_is_writable_over_a_tmpfs_that_hides_its_siblings() {
        let (repo, admin, root) = dirs("wt");
        let wt = root.join("task--arm");
        let a = bwrap_argv(&wt, &root, &admin, &repo, &["true".into()]);
        assert!(
            pairs(&a, "--tmpfs").contains(&root.to_string_lossy().into_owned()),
            "{a:?}"
        );
        assert!(
            pairs(&a, "--bind").contains(&wt.to_string_lossy().into_owned()),
            "{a:?}"
        );
        let t = a
            .iter()
            .position(|x| x == &root.to_string_lossy().into_owned())
            .unwrap();
        let b = a
            .iter()
            .position(|x| x == &wt.to_string_lossy().into_owned())
            .unwrap();
        assert!(
            b > t,
            "the worktree bind must follow the tmpfs that would hide it: {a:?}"
        );
    }

    /// The command runs after `--`, unaltered, so an argument that looks like a bwrap flag
    /// is passed to the arm rather than to bwrap.
    #[test]
    fn the_command_is_passed_through_after_the_separator() {
        let (repo, admin, root) = dirs("cmd");
        let cmd: Vec<String> = ["opencode", "run", "--ro-bind", "-m", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = bwrap_argv(&root.join("task--arm"), &root, &admin, &repo, &cmd);
        let i = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(&a[i + 1..], &cmd[..]);
    }

    /// A directory that does not exist is not bound at all -- binding a missing path aborts
    /// bwrap, which would kill the run before the arm starts.
    #[test]
    fn absent_directories_are_not_bound() {
        let base = std::env::temp_dir().join(format!("fb-isolated-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = bwrap_argv(
            &base.join("wt"),
            &base.join("root"),
            &base.join("admin"),
            &base.join("repo"),
            &["true".into()],
        );
        assert!(!a.iter().any(|x| x == "--ro-bind"), "{a:?}");
    }

    #[test]
    fn the_repository_is_the_admin_directorys_grandparent() {
        assert_eq!(
            repo_of(Path::new("/x/y/.git/worktrees")),
            PathBuf::from("/x/y")
        );
    }
}
