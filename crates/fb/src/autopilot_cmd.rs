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

/// The scope names systemd reports, reduced to task names.
pub fn live_tasks_from(scopes: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for line in scopes.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        let Some(rest) = name.strip_prefix("fb-") else {
            continue;
        };
        let Some(stem) = rest.strip_suffix(".scope") else {
            continue;
        };
        // fb-<task>--<arm>-<pid>.scope
        if let Some((task, _)) = stem.split_once("--") {
            out.insert(task.to_string());
        }
    }
    out.into_iter().collect()
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
    let live_scopes = std::process::Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "--type=scope",
            "--state=running",
            "--no-legend",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let live_tasks = live_tasks_from(&live_scopes);
    let live = live_scopes.lines().filter(|l| l.contains("fb-")).count();
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
pub fn run(once: bool) -> i32 {
    let repo = crate::paths::repo();
    let logs = crate::paths::logs();
    let max_waves: usize = std::env::var("FB_MAX_WAVES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    loop {
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

    /// Scope names reduce to task names. systemd reports fb-<task>--<arm>-<pid>.scope.
    #[test]
    fn scope_names_reduce_to_tasks() {
        let s = "fb-cost--codex_luna-123.scope loaded active running\n\
                 fb-cost--glm-456.scope loaded active running\n\
                 other.scope loaded active running\n";
        assert_eq!(live_tasks_from(s), vec!["cost".to_string()]);
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
}
