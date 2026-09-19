//! `fb pipeline <task>` — run every stage for one task, in order, skipping what is fresh.
//!
//! Ported from fb-pipeline.sh. The wave path implements Dispatch only; score and the
//! subjective tier were driven by `fb trial` and silently stopped running when waves
//! replaced it -- fifteen tasks were adjudicated on objective metrics alone and nothing
//! reported the absence (bead farmerbob-k9f). This is the missing tail, and it is
//! idempotent so the autopilot can call it repeatedly.

use farmerbob_core::target_decl;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether a stage's artefact exists in a form that counts.
///
/// A non-empty FILE, or a DIRECTORY holding at least one entry. critique writes a
/// directory, and the pipeline declared its artefact as a file it has never written -- so
/// the stage reported "RAN BUT PRODUCED NOTHING" on every run where it had in fact written
/// every review asked of it.
pub fn have_artefact(path: &Path) -> bool {
    if let Ok(m) = fs::metadata(path) {
        if m.is_file() {
            return m.len() > 0;
        }
        if m.is_dir() {
            return fs::read_dir(path)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        }
    }
    false
}

/// One candidate's content, as a short hash: tracked edits plus anything created untracked.
///
/// A created deliverable is invisible to `git diff HEAD`, which is most of this project's
/// tasks. No `git add` -- a freshness check must not mutate what it measures.
pub fn wt_hash(wt_root: &Path, task: &str, arm: &str) -> String {
    let dir = wt_root.join(format!("{task}--{arm}"));
    if !dir.is_dir() {
        return "GONE".to_string();
    }
    let mut h = Fnv::new();
    if let Ok(out) = Command::new("git")
        .current_dir(&dir)
        .args(["diff", "HEAD"])
        .output()
    {
        h.write(&out.stdout);
    }
    if let Ok(out) = Command::new("git")
        .current_dir(&dir)
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
    {
        for f in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(bytes) = fs::read(dir.join(f.trim())) {
                h.write(&bytes);
            }
        }
    }
    format!("{:012x}", h.0)
}

/// FNV-1a, 64-bit. A freshness key only has to change when the bytes change, and a
/// dependency for that is not worth carrying -- nor is `DefaultHasher`, whose value is not
/// promised to be stable across toolchains, which would silently re-run every stage after
/// an upgrade.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x1000_0000_01b3);
        }
    }
}

/// The set of CANDIDATES on disk, with their content.
///
/// Names alone are not enough: a follow-up turn edits a candidate without changing which
/// candidates exist, so a name-only key reports "already done" while the reviewed code has
/// moved underneath it.
pub fn wtfield(wt_root: &Path, task: &str) -> String {
    let prefix = format!("{task}--");
    let Ok(entries) = fs::read_dir(wt_root) else {
        return "NOCANDIDATES".to_string();
    };
    let mut arms: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix(&prefix).map(str::to_string)
        })
        // A critic's scratch checkout matches the same prefix and is not a candidate.
        .filter(|a| !a.starts_with("critic--"))
        .collect();
    arms.sort();
    if arms.is_empty() {
        return "NOCANDIDATES".to_string();
    }
    arms.iter()
        .map(|a| format!("{a}:{}", wt_hash(wt_root, task, a)))
        .collect::<Vec<_>>()
        .join(",")
}

/// The set of gate-PASSING candidates, with their content.
///
/// TOTAL: it returns exactly one of three shapes and never an empty string. It used to
/// print nothing both when no candidate passed and when the task had never been scored, so
/// an empty field could never trigger a re-run.
pub fn field(logs: &Path, wt_root: &Path, task: &str) -> String {
    let score = logs.join(format!("{task}.score.json"));
    let Ok(text) = fs::read_to_string(&score) else {
        return "UNSCORED".to_string();
    };
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return "UNSCORED".to_string();
    };
    let mut passing: Vec<String> = rows
        .iter()
        .filter(|r| r.get("verdict").and_then(|v| v.as_str()) == Some("PASS"))
        .filter_map(|r| r.get("source").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    passing.sort();
    if passing.is_empty() {
        return "NONE".to_string();
    }
    passing
        .iter()
        .map(|a| format!("{a}:{}", wt_hash(wt_root, task, a)))
        .collect::<Vec<_>>()
        .join(",")
}

/// What running one stage decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// Fresh: the artefact exists and the key matches.
    AlreadyDone,
    /// Ran and produced its artefact.
    Ok,
    /// Not applicable -- the stage said so with exit 4. Nothing went wrong.
    NotApplicable,
    /// The command failed.
    Failed,
    /// The command exited 0 and wrote nothing. Either the stage is broken or the pipeline
    /// names the wrong file, and BOTH are bugs worth stopping for.
    ProducedNothing,
}

/// Decide a stage's outcome from what the command did and what it left behind.
///
/// `None` for `code` means the stage was not run because it was already fresh.
pub fn outcome_of_opt(code: Option<i32>, artefact_present: bool) -> StageOutcome {
    let Some(code) = code else {
        return StageOutcome::AlreadyDone;
    };
    outcome_of(code, artefact_present)
}

/// Decide a stage's outcome from what the command did and what it left behind.
pub fn outcome_of(code: i32, artefact_present: bool) -> StageOutcome {
    match code {
        4 => StageOutcome::NotApplicable,
        0 if artefact_present => StageOutcome::Ok,
        0 => StageOutcome::ProducedNothing,
        _ => StageOutcome::Failed,
    }
}

/// Whether a stage should run at all, given its artefact and the recorded key.
pub fn needs_run(artefact_present: bool, recorded: Option<&str>, now: &str) -> bool {
    if !artefact_present {
        return true;
    }
    recorded != Some(now)
}

fn logs_dir() -> PathBuf {
    crate::paths::logs()
}

/// Run every stage for one task.
pub fn run(task: &str, krate: &str, target: Option<&str>) -> i32 {
    let repo = crate::paths::repo();
    let logs = logs_dir();
    let wt_root = crate::paths::worktrees();
    let target = match target {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => {
            let spec = fs::read_to_string(repo.join(".fb/prompts").join(format!("{task}.md")))
                .unwrap_or_default();
            match target_decl::declared_all(&spec) {
                Ok(ds) if !ds.is_empty() => target_decl::path(&ds[0]).to_string(),
                _ => {
                    eprintln!("{task}: the spec declares no deliverable");
                    return 2;
                }
            }
        }
    };
    println!("== pipeline {task} ({krate}, {target})");

    let mut failed: Vec<&str> = Vec::new();
    let stage = |name: &'static str,
                 artefact: PathBuf,
                 key: String,
                 code: i32,
                 failed: &mut Vec<&'static str>| {
        let sig = PathBuf::from(format!("{}.field", artefact.display()));
        let recorded = fs::read_to_string(&sig).ok();
        match outcome_of_opt(Some(code), have_artefact(&artefact)) {
            StageOutcome::AlreadyDone => println!("  {name}: already done"),
            StageOutcome::NotApplicable => println!("  {name}: n/a"),
            StageOutcome::Ok => {
                println!("  {name}: ok");
                let _ = fs::write(&sig, &key);
            }
            StageOutcome::ProducedNothing => {
                println!(
                    "  {name}: RAN BUT PRODUCED NOTHING at {}",
                    artefact.display()
                );
                println!("     the command exited 0 and the artefact is missing or empty.");
                failed.push(name);
            }
            StageOutcome::Failed => {
                println!("  {name}: FAILED");
                failed.push(name);
            }
        }
        let _ = recorded;
    };

    // score is keyed on the CANDIDATES ON DISK; everything downstream on who PASSED.
    let score_art = logs.join(format!("{task}.score.json"));
    let wtkey = wtfield(&wt_root, task);
    if needs_run(
        have_artefact(&score_art),
        fs::read_to_string(format!("{}.field", score_art.display()))
            .ok()
            .as_deref(),
        &wtkey,
    ) {
        println!("  score: running");
        let code = crate::score::run_cmd(task, krate, false);
        stage("score", score_art.clone(), wtkey, code, &mut failed);
    } else {
        println!("  score: already done");
    }

    let key = field(&logs, &wt_root, task);
    // The commands are called INSIDE the loop, after the freshness check. Building an array
    // of (name, artefact, code) triples evaluates every run_cmd before the loop begins, so
    // every stage ran on every invocation however fresh it was -- which defeats the whole
    // point of the key and spends an arm each time.
    let downstream: [(&'static str, PathBuf); 4] = [
        ("crossx", logs.join(format!("{task}.crossx.json"))),
        ("critique", logs.join("critiques").join(task)),
        ("promote", logs.join(format!("{task}.promoted.json"))),
        ("prove", logs.join(format!("{task}.proved.json"))),
    ];
    for (name, artefact) in downstream {
        let sig = format!("{}.field", artefact.display());
        if !needs_run(
            have_artefact(&artefact),
            fs::read_to_string(&sig).ok().as_deref(),
            &key,
        ) {
            println!("  {name}: already done");
            continue;
        }
        println!("  {name}: running");
        let code = match name {
            "crossx" => crate::crossx::run_cmd(task, krate, &target),
            "critique" => crate::critique::run_cmd(task, krate, &target),
            "promote" => crate::promote::run_cmd(task),
            _ => crate::prove::run_cmd(task, krate, &target, &crate::prove::default_prover(None)),
        };
        stage(name, artefact, key.clone(), code, &mut failed);
    }

    if failed.is_empty() {
        println!("== {task} ready for adjudication");
        0
    } else {
        println!("== {task} INCOMPLETE: {}", failed.join(", "));
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exit 4 is NOT APPLICABLE and must never be a failure. crossx returning 1 for both
    /// "fewer than two candidates" and a usage error halted every single-arm task's
    /// pipeline (bead farmerbob-9ef2).
    #[test]
    fn exit_four_is_not_applicable_not_failure() {
        assert_eq!(outcome_of(4, false), StageOutcome::NotApplicable);
        assert_eq!(outcome_of(1, false), StageOutcome::Failed);
    }

    /// A STAGE IS DONE WHEN ITS ARTEFACT EXISTS, not when its command exits 0. A command
    /// that succeeds and writes nothing is a bug in what the pipeline believes the stage
    /// produces -- promote declared an artefact nothing wrote and was recorded as success.
    #[test]
    fn exiting_zero_without_an_artefact_is_not_success() {
        assert_eq!(outcome_of(0, true), StageOutcome::Ok);
        assert_eq!(outcome_of(0, false), StageOutcome::ProducedNothing);
    }

    /// A stage that was not run because it was fresh is its own outcome, distinct from one
    /// that ran and produced its artefact. Both are "fine"; only one spent an arm.
    #[test]
    fn not_running_is_distinct_from_running_successfully() {
        assert_eq!(outcome_of_opt(None, true), StageOutcome::AlreadyDone);
        assert_eq!(outcome_of_opt(Some(0), true), StageOutcome::Ok);
    }

    /// A missing artefact always runs. An unrecorded key is not a match.
    #[test]
    fn a_missing_artefact_always_runs() {
        assert!(needs_run(false, Some("k"), "k"));
        assert!(needs_run(true, None, "k"));
    }

    /// The key is what makes re-adjudication automatic: same artefact, changed key, runs
    /// again. Pinned as a pair against the matching key, which does not.
    #[test]
    fn a_changed_key_re_runs_and_a_matching_one_does_not() {
        assert!(needs_run(true, Some("old"), "new"));
        assert!(!needs_run(true, Some("same"), "same"));
    }

    /// An empty directory is not an artefact. critique writes a DIRECTORY, and the pipeline
    /// once declared its artefact as a file critique has never written, so the stage
    /// reported producing nothing on every run where it had written every review asked of
    /// it.
    #[test]
    fn an_empty_directory_is_not_an_artefact() {
        let d = std::env::temp_dir().join(format!("fb-pipe-art-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
        assert!(!have_artefact(&d), "empty directory");
        let _ = fs::write(d.join("x.md"), "one\n");
        assert!(have_artefact(&d), "directory with an entry");
        let empty = d.join("empty.json");
        let _ = fs::write(&empty, "");
        assert!(!have_artefact(&empty), "empty file");
        let _ = fs::remove_dir_all(&d);
    }
}
