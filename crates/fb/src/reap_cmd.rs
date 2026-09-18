//! `fb reap` — the first caller of the wtreap chain.
//!
//! `wtreap`, `reap_plan` and `reap_exec` were specified, implemented, cross-examined and merged
//! over four waves, and until this file nothing called any of them. Meanwhile 327 worktree
//! directories sat on disk against 124 git registrations, which is the number that motivated
//! the whole chain.
//!
//! This reads the real world and hands it to that logic. It makes no decisions: every rule
//! lives in core and is tested there. What lives here is I/O and the refusal to act without
//! `--execute`.
//!
//! DRY RUN BY DEFAULT, deliberately. The thing being automated is deletion of directories that
//! agents write to, and this project has already destroyed eight of ten runs once by removing a
//! worktree under a live arm. A command that prints what it would do, and needs a second flag to
//! do it, is the shape that failure argues for.

use farmerbob_core::reap_exec::{Admitted, Stale, admit_plan};
use farmerbob_core::reap_plan::{Skip, Step, plan};
use farmerbob_core::wtreap::{Worktree, census, conflicts};
use std::collections::BTreeSet;
use std::process::Command;

/// Directory names under the worktrees root.
fn dirs_on_disk(root: &std::path::Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if let (true, Some(n)) = (entry.file_type()?.is_dir(), entry.file_name().to_str()) {
            out.push(n.to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// Basenames git currently registers as worktrees.
///
/// Parsed from `git worktree list --porcelain`, whose `worktree <path>` lines
/// are stable across versions; the human format is not.
fn registered(repo: &std::path::Path) -> Vec<String> {
    let out = match Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .filter_map(|p| p.rsplit('/').next())
        .map(str::to_string)
        .collect()
}

/// Directory keys with a live agent, from the systemd scope unit names.
fn live_keys() -> Vec<String> {
    let out = match Command::new("systemctl")
        .args(["--user", "list-units", "--type=scope", "--no-legend"])
        .output()
    {
        Ok(o) => o.stdout,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out);
    let mut keys = BTreeSet::new();
    for tok in text.split_whitespace() {
        let Some(rest) = tok.strip_prefix("fb-") else {
            continue;
        };
        let Some(unit) = rest.strip_suffix(".scope") else {
            continue;
        };
        // fb-<task>--<arm>-<pid>.scope
        let Some(cut) = unit.rfind('-') else { continue };
        let key = &unit[..cut];
        if key.contains("--") {
            keys.insert(key.to_string());
        }
    }
    keys.into_iter().collect()
}

/// Directory keys whose task has an adjudication record.
///
/// A settled TASK settles every one of its arms' directories: the ruling is
/// written once per task, and the evidence that outlived it is the record,
/// not the tree.
fn settled_keys(repo: &std::path::Path, dirs: &[String]) -> Vec<String> {
    let adj = repo.join(".fb/adjudicated");
    let tasks: BTreeSet<String> = match std::fs::read_dir(&adj) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect(),
        Err(_) => BTreeSet::new(),
    };
    dirs.iter()
        .filter(|d| match d.split_once("--") {
            Some((task, _)) => tasks.contains(task),
            None => false,
        })
        .cloned()
        .collect()
}

fn skip_word(s: Skip) -> &'static str {
    match s {
        Skip::InUse => "in use",
        Skip::Indeterminate => "indeterminate",
        Skip::Conflicted => "conflicted",
        Skip::RegisteredAndUnregisterNotPermitted => "registered (unregister not permitted)",
    }
}

fn stale_word(s: Stale) -> &'static str {
    match s {
        Stale::NowInUse => "a live run now holds it",
        Stale::NoLongerRegistered => "git no longer registers it",
        Stale::NowRegistered => "it is registered now",
        Stale::Absent => "it is already gone",
    }
}

fn run_step(repo: &std::path::Path, root: &std::path::Path, step: &Step) -> Result<(), String> {
    match step {
        Step::Unregister { dir } => {
            let path = root.join(dir);
            let out = Command::new("git")
                .args(["-C"])
                .arg(repo)
                .args(["worktree", "remove", "--force"])
                .arg(&path)
                .output()
                .map_err(|e| format!("git worktree remove {dir}: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "git worktree remove {dir}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        }
        Step::Delete { dir } => {
            let path = root.join(dir);
            // Refuse anything that is not under the worktrees root. The plan only ever names
            // basenames, so this can only fail if one carried a separator -- but a bug that
            // deletes outside the root is the one bug worth a second check.
            if dir.contains('/') || dir.contains("..") || !path.starts_with(root) {
                return Err(format!(
                    "refusing to delete {dir}: not a plain name under the root"
                ));
            }
            std::fs::remove_dir_all(&path).map_err(|e| format!("rm {dir}: {e}"))
        }
    }
}

/// Print the reap plan; with `execute`, perform the admitted steps.
pub fn run_cmd(execute: bool, may_unregister: bool) -> i32 {
    let repo = crate::paths::repo();
    let root = crate::paths::worktrees();

    let dirs = match dirs_on_disk(&root) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("fb reap: reading {}: {e}", root.display());
            return 1;
        }
    };
    let reg = registered(&repo);
    let live = live_keys();
    let settled = settled_keys(&repo, &dirs);

    let reg_set: BTreeSet<&str> = reg.iter().map(String::as_str).collect();
    let wts: Vec<Worktree<'_>> = dirs
        .iter()
        .map(|d| Worktree {
            dir: d.as_str(),
            registered: reg_set.contains(d.as_str()),
        })
        .collect();
    let live_r: Vec<&str> = live.iter().map(String::as_str).collect();
    let settled_r: Vec<&str> = settled.iter().map(String::as_str).collect();

    let c = census(&wts, &live_r, &settled_r);
    println!(
        "{} directories, {} registered with git, {} live, {} settled",
        dirs.len(),
        reg.len(),
        live.len(),
        settled.len()
    );
    println!(
        "census: registered {}  reapable {}  in-use {}  indeterminate {}",
        c.registered, c.reapable, c.in_use, c.indeterminate
    );
    let conf = conflicts(&wts);
    if !conf.is_empty() {
        println!(
            "{} contradictory listing(s), excluded from every plan:",
            conf.len()
        );
        for k in conf.iter().take(10) {
            println!(
                "  {} ({} say registered, {} say not)",
                k.dir, k.registered, k.unregistered
            );
        }
    }

    let p = plan(&wts, &live_r, &settled_r, may_unregister);
    let present: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let reg_r: Vec<&str> = reg.iter().map(String::as_str).collect();
    let Admitted { go, stopped } = admit_plan(&p, &live_r, &reg_r, &present);

    let mut by_reason: std::collections::BTreeMap<&str, u32> = Default::default();
    for (_, s) in &p.skipped {
        *by_reason.entry(skip_word(*s)).or_default() += 1;
    }
    if !by_reason.is_empty() {
        println!("skipped, by reason:");
        for (r, n) in &by_reason {
            println!("  {n:>4}  {r}");
        }
    }

    if !stopped.is_empty() {
        println!("{} step(s) refused on re-check:", stopped.len());
        for (step, why) in stopped.iter().take(10) {
            let d = match step {
                Step::Unregister { dir } | Step::Delete { dir } => dir,
            };
            println!("  {d}: {}", stale_word(*why));
        }
    }

    if go.is_empty() {
        println!("nothing to do. That is an answer, not a failure.");
        return 0;
    }

    println!("{} admitted step(s):", go.len());
    for step in &go {
        match step {
            Step::Unregister { dir } => println!("  unregister  {dir}"),
            Step::Delete { dir } => println!("  delete      {dir}"),
        }
    }

    if !execute {
        println!();
        println!("DRY RUN. Nothing was changed. Re-run with --execute to perform these steps.");
        return 0;
    }

    let mut failed = 0;
    for step in &go {
        if let Err(e) = run_step(&repo, &root, step) {
            eprintln!("  {e}");
            failed += 1;
        }
    }
    println!("performed {} step(s), {failed} failed", go.len() - failed);
    i32::from(failed > 0)
}
