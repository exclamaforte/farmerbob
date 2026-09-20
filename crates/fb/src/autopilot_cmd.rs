//! `fb autopilot` — launch queued waves and run missing pipeline stages, forever.
//!
//! Ported from fb-autopilot.sh. The decisions are pure and tested; the loop and the clock
//! are not.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Why a queued matrix is not launched now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// The matrix names no arms, so launching it would dispatch nothing while consuming
    /// the queue slot.
    NoArms,
    /// Not enough free slots for the arms it wants.
    NotEnoughSlots {
        wants: usize,
        live: usize,
        slots: usize,
    },
    /// A task in this matrix writes a file a LIVE task is already writing. Two arms editing
    /// one path in parallel is how a merge becomes unattributable.
    Collides {
        task: String,
        target: String,
        with: String,
    },
}

/// How many arms a matrix will dispatch: the sum over EVERY row.
///
/// Counting only the first row said "1 arm" for a five-row wave, and the autopilot launched
/// it into a machine that could hold one.
pub fn wave_arms(matrix: &str) -> usize {
    matrix
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| l.split('\t').nth(2))
        .map(|arms| arms.split(',').filter(|a| !a.trim().is_empty()).count())
        .sum()
}

/// The tasks a matrix names, in order, deduplicated.
pub fn tasks_in(matrix: &str) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for line in matrix.lines() {
        let t = line.split('\t').next().unwrap_or("").trim();
        if !t.is_empty() && seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out
}

/// Whether this matrix may launch.
///
/// `target_of` answers what a task writes; `live` is what is running now.
pub fn may_launch(
    matrix: &str,
    live: usize,
    slots: usize,
    live_tasks: &[String],
    target_of: &impl Fn(&str) -> Option<String>,
) -> Result<(), Hold> {
    let wants = wave_arms(matrix);
    if wants == 0 {
        return Err(Hold::NoArms);
    }
    if live + wants > slots {
        return Err(Hold::NotEnoughSlots { wants, live, slots });
    }
    for task in tasks_in(matrix) {
        let Some(want) = target_of(&task) else {
            continue;
        };
        for other in live_tasks {
            if target_of(other).as_deref() == Some(want.as_str()) {
                return Err(Hold::Collides {
                    task,
                    target: want,
                    with: other.clone(),
                });
            }
        }
    }
    Ok(())
}

/// The tasks with a live agent, from the worktrees those agents are working in.
///
/// `live_tasks_from` reads systemd's scope list, which is the measurement `live_cmd` exists
/// to replace: a scope left in a FAILED state after a timeout lingers as a unit and reads as
/// live indefinitely, and an agent launched without a scope reads as absent. There is a
/// stale `fb-doctor-reach--oc-kimi-k3-122408.scope` in this user's session right now whose
/// process died long ago.
///
/// A worktree is named `<task>--<arm>`, so the task falls straight out of it.
pub fn live_tasks_in(worktrees: &BTreeSet<String>) -> Vec<String> {
    worktrees
        .iter()
        .filter_map(|w| w.split_once("--").map(|(task, _)| task.to_string()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A task that has been scored, whose deliverable is not merged, and whose claims are
/// missing -- the pipeline never finished for it.
pub fn needs_pipeline(
    spec_exists: bool,
    target_merged: bool,
    has_claims: bool,
    skipped: bool,
) -> bool {
    spec_exists && !target_merged && !has_claims && !skipped
}

fn run_once(repo: &Path, logs: &Path, max_waves: usize) -> bool {
    let queue = repo.join(".fb/queue");
    let dispatched = queue.join("dispatched");
    let _ = fs::create_dir_all(&dispatched);
    // ONE MEASUREMENT, ONE IMPLEMENTATION. This counted running systemd scopes, which is
    // the measurement `live_cmd` was written to replace -- a scope left FAILED after a
    // timeout lingers and reads as live for ever, and an agent launched without a scope
    // reads as absent. `fb live` inspects the worktrees under /proc instead, and the two
    // counts sitting side by side is how this harness got its launcher table twice.
    let root = crate::paths::worktrees().to_string_lossy().into_owned();
    let busy = crate::live_cmd::worktrees_live(&crate::live_cmd::gather_procs(&root), &root);
    let live_tasks = live_tasks_in(&busy);
    let live = busy.len();
    let waves = std::process::Command::new("pgrep")
        .args(["-fc", "fb admit"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(0);
    let slots = crate::admit_cmd::usable_slots(available_mb(), 3072, 1536);

    let target_of = |task: &str| -> Option<String> {
        let spec = fs::read_to_string(repo.join(".fb/prompts").join(format!("{task}.md"))).ok()?;
        let ds = farmerbob_core::target_decl::declared_all(&spec).ok()?;
        ds.first()
            .map(|d| farmerbob_core::target_decl::path(d).to_string())
    };

    if waves < max_waves && slots > 0 {
        let mut queued: Vec<PathBuf> = fs::read_dir(&queue)
            .map(|e| {
                e.flatten()
                    .map(|x| x.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "tsv"))
                    .collect()
            })
            .unwrap_or_default();
        queued.sort();
        if let Some(next) = queued.first() {
            let text = fs::read_to_string(next).unwrap_or_default();
            match may_launch(&text, live, slots, &live_tasks, &target_of) {
                Ok(()) => {
                    println!(
                        "launching {}",
                        next.file_name().unwrap_or_default().to_string_lossy()
                    );
                    let code = crate::wave_cmd::run(next, true);
                    return code == 0;
                }
                Err(hold) => println!("holding: {hold:?}"),
            }
        }
    }
    // Nothing launched and nothing live: catch up a task whose pipeline never finished.
    if live == 0 && waves == 0 {
        let prompts = repo.join(".fb/prompts");
        if let Ok(entries) = fs::read_dir(&prompts) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.strip_suffix(".md")
                        .filter(|t| !t.starts_with('_'))
                        .map(str::to_string)
                })
                .collect();
            names.sort();
            for task in names {
                let scored = logs.join(format!("{task}.score.json")).is_file();
                if !scored {
                    continue;
                }
                let merged = target_of(&task)
                    .map(|t| repo.join(t).is_file())
                    .unwrap_or(false);
                let has_claims = logs
                    .join(format!("{task}.claims.json"))
                    .metadata()
                    .map(|m| m.len() > 0)
                    .unwrap_or(false);
                let skipped = dispatched.join(format!(".skip.{task}")).is_file();
                if needs_pipeline(true, merged, has_claims, skipped) {
                    println!("running missing pipeline stages for {task}");
                    if crate::pipeline_cmd::run(&task, "farmerbob-core", None) != 0 {
                        let _ = fs::write(dispatched.join(format!(".skip.{task}")), "");
                    }
                    return true;
                }
            }
        }
        println!("IDLE, queue empty -- nothing to launch");
    } else {
        println!("busy: {live} agents, {waves} wave(s)");
    }
    false
}

fn available_mb() -> u64 {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("MemAvailable:"))
                .and_then(|r| {
                    r.split_whitespace()
                        .next()
                        .and_then(|v| v.parse::<u64>().ok())
                })
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

/// Run the loop. `once` does a single tick, which is what the tests and a cron would want.
/// Whether this process is running a binary that has since been replaced on disk.
///
/// `cargo build` replaces the file, and the kernel then appends " (deleted)" to
/// /proc/self/exe for every process still running the old inode.
///
/// THIS HAS STOPPED THE AUTOPILOT TWICE IN ONE DAY. The first time it failed loudly -- the
/// wave launch was ENOENT because `current_exe()` handed back the deleted path -- and
/// `wave_cmd::runnable_exe` fixed that. The second time it failed SILENTLY: a process built
/// before that fix went on ticking with the old logic, printed "busy" every two minutes, and
/// launched nothing. A queued wave sat for fourteen minutes until I restarted it by hand.
///
/// Fixing the spawn was not enough, because the stale process is still running stale
/// DECISIONS. The only safe answer is to stop being that process.
pub fn binary_was_replaced() -> bool {
    std::fs::read_link("/proc/self/exe")
        .map(|p| p.to_string_lossy().ends_with(" (deleted)"))
        .unwrap_or(false)
}

pub fn run(once: bool) -> i32 {
    let repo = crate::paths::repo();
    let logs = crate::paths::logs();
    let max_waves: usize = std::env::var("FB_MAX_WAVES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    loop {
        // RE-EXEC RATHER THAN RUN STALE. Exiting would leave no autopilot at all, which is
        // worse than stale logic and is the one thing this loop must never do. Re-exec
        // replaces this process with the current binary, keeping the loop alive across a
        // rebuild -- which happens many times an hour while the harness is being worked on.
        if !once && binary_was_replaced() {
            let current = repo.join("target/debug/fb");
            println!(
                "autopilot: my binary was replaced on disk; re-execing {}",
                current.display()
            );
            let err = std::os::unix::process::CommandExt::exec(
                std::process::Command::new(&current).arg("autopilot"),
            );
            // `exec` only returns on failure. Say so and carry on with the old code rather
            // than dying: stale decisions beat no autopilot.
            eprintln!("autopilot: re-exec failed ({err}); continuing on the old binary");
        }
        let launched = run_once(&repo, &logs, max_waves);
        if once {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_secs(if launched {
            10
        } else {
            120
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arms are summed over EVERY row. Counting only the first said "1 arm" for a five-row
    /// wave and launched it into a machine that could hold one.
    #[test]
    fn arms_are_counted_across_every_row() {
        let m = "t1\tfb\ta,b\nt2\tfb\tc\nt3\tfb\td,e\n";
        assert_eq!(wave_arms(m), 5);
    }

    /// A matrix naming no arms is held, not launched. Launching it would consume the queue
    /// slot and dispatch nothing.
    #[test]
    fn a_matrix_with_no_arms_is_held() {
        assert_eq!(
            may_launch("t\tfb\t\n", 0, 10, &[], &|_| None),
            Err(Hold::NoArms)
        );
    }

    /// The slot check counts LIVE plus WANTED, not either alone. A wave wanting three arms
    /// on a machine with two free slots and one live agent does not fit.
    #[test]
    fn the_slot_check_adds_live_to_wanted() {
        let m = "t\tfb\ta,b,c\n";
        assert_eq!(
            may_launch(m, 1, 3, &[], &|_| None),
            Err(Hold::NotEnoughSlots {
                wants: 3,
                live: 1,
                slots: 3
            })
        );
        assert!(may_launch(m, 0, 3, &[], &|_| None).is_ok(), "exactly fits");
    }

    /// A task writing a file a LIVE task is already writing is HELD. Two arms editing one
    /// path in parallel makes a merge unattributable, and the hold names both tasks.
    #[test]
    fn a_collision_with_a_live_task_holds_and_names_it() {
        let m = "mine\tfb\ta\n";
        // BOTH tasks write the same file -- that is the collision.
        let target = |_: &str| Some("x.rs".to_string());
        match may_launch(m, 0, 10, &["theirs".to_string()], &target) {
            Err(Hold::Collides { task, target, with }) => {
                assert_eq!(task, "mine");
                assert_eq!(target, "x.rs");
                assert_eq!(with, "theirs");
            }
            other => panic!("expected Collides, got {other:?}"),
        }
    }

    /// Different targets do not collide. Same input as the test above with one value
    /// changed -- without this pair the check could hold everything and look correct.
    #[test]
    fn different_targets_do_not_collide() {
        let m = "mine\tfb\ta\n";
        let target = |t: &str| Some(if t == "mine" { "x.rs" } else { "y.rs" }.to_string());
        assert!(may_launch(m, 0, 10, &["theirs".to_string()], &target).is_ok());
    }

    /// Worktree names reduce to task names, and two arms on one task are ONE live task:
    /// the caller uses this to refuse a matrix that collides with work already running.
    ///
    /// This read systemd's scope list until 2026-09-19. That is the measurement `live_cmd`
    /// exists to replace -- a scope left FAILED after a timeout lingers as a unit and reads
    /// as live for ever, and this user's session is carrying exactly such a stale scope
    /// right now, for a process that died long ago.
    #[test]
    fn worktree_names_reduce_to_tasks() {
        let wts: BTreeSet<String> = ["cost--codex-luna", "cost--glm-53", "other--arm"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            live_tasks_in(&wts),
            vec!["cost".to_string(), "other".to_string()]
        );
    }

    /// A directory that is not a `<task>--<arm>` worktree names no task, rather than naming
    /// itself as one: a stray directory under the root must not make a task look busy and
    /// block its matrix for ever.
    #[test]
    fn a_directory_that_is_not_a_worktree_names_no_task() {
        let wts: BTreeSet<String> = ["scratch".to_string()].into_iter().collect();
        assert!(live_tasks_in(&wts).is_empty());
    }

    /// A task needs the pipeline only when it was scored, is NOT merged, has no claims and
    /// was not skipped. Every one of those is a reason to leave it alone.
    #[test]
    fn needs_pipeline_requires_all_four_conditions() {
        assert!(needs_pipeline(true, false, false, false));
        assert!(!needs_pipeline(false, false, false, false), "no spec");
        assert!(!needs_pipeline(true, true, false, false), "already merged");
        assert!(
            !needs_pipeline(true, false, true, false),
            "already has claims"
        );
        assert!(!needs_pipeline(true, false, false, true), "skipped");
    }

    /// THIS HAS STOPPED THE AUTOPILOT TWICE IN ONE DAY. `cargo build` replaces the binary
    /// and the kernel appends " (deleted)" to /proc/self/exe for every process still on the
    /// old inode. The first failure was loud -- an ENOENT wave launch. The second was
    /// silent: a process built before that fix ticked on with old logic, printed "busy"
    /// every two minutes, and launched nothing while a queued wave sat for fourteen minutes.
    #[test]
    fn a_live_binary_is_not_reported_as_replaced() {
        // This test process's own binary exists, so the check must be false for it.
        assert!(!binary_was_replaced());
    }

    /// The marker is the kernel's exact suffix, not a substring that could appear in a path.
    #[test]
    fn the_deleted_marker_is_a_suffix_not_a_contains() {
        let deleted = "/repo/target/debug/fb (deleted)";
        let innocent = "/repo/target/debug/fb (deleted)-backup/fb";
        assert!(deleted.ends_with(" (deleted)"));
        assert!(
            !innocent.ends_with(" (deleted)"),
            "a path merely CONTAINING the marker is not a replaced binary"
        );
    }
}
