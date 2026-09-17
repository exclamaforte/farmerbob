//! `fb score <bead>` — measure every candidate worktree for one task.
//!
//! Replaces `fb-score.sh`. The shell version was correct; what it could not do was share
//! its correctness. Two rules it got right were reimplemented wrongly elsewhere:
//!
//!   * the verdict, reimplemented four times, each time reintroducing the empty-suite pass
//!     (now `farmerbob_core::gate`, which this calls);
//!   * liveness, where a worktree is live only if the task marker is present AND a process
//!     is actually sitting in it. `fb-score.sh` fixed that; `fb-conform`, `fb-critique`,
//!     `fb-verify`, `fb-crossx` and `fb-defects` still test the marker alone and so still
//!     believe an orphaned run is live forever.  (bead farmerbob-jd2.2)
//!
//! Division of labour: this module does I/O — enumerate worktrees, run cargo, read /proc —
//! and every *decision* is delegated to `farmerbob-core`, which stays pure.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use farmerbob_core::gate::{judge, Observation as GateObs, Verdict};
use farmerbob_core::liveness::{Authority, Liveness, Observation as LiveObs, Tracker};
use farmerbob_core::measurement::Measurement;

/// Launcher process names that indicate an agent is working in a worktree.
const LAUNCHERS: [&str; 4] = ["opencode", "agy", "zcode", "codex"];

/// The marker `fb-dispatch.sh` writes on launch and removes on exit. Its ABSENCE is
/// positive evidence that the run finished; its presence is evidence only as fresh as
/// the process sitting in the worktree makes it.
const MARKER: &str = ".fb-task.md";

/// One candidate's measurements, in the JSON shape the rest of the harness already reads.
struct Record {
    source: String,
    verdict: Verdict,
    build: bool,
    tests_ok: bool,
    tests_run: u32,
    /// Lines added. Absent when git cannot read the worktree at all -- which must NOT be
    /// summed to zero, because zero is the arm's fault and absent is the harness's.
    lines: Measurement<u32>,
    new_files: u32,
    crates: BTreeSet<String>,
    /// Clippy warnings ABOVE the baseline. Absent when the build failed, because a
    /// crate that does not compile has no lint count — which is not the same as zero.
    clippy: Measurement<i32>,
    duration_s: Measurement<f64>,
    liveness: Liveness,
    err: Option<String>,
}

/// The legacy verdict spellings. The shell pipeline greps for these exact strings, so
/// they are a wire format, not a display choice.
fn wire(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::NoOp => "NO-OP",
        Verdict::NoCompile => "NO-COMPILE",
        Verdict::TestsFail => "TESTS-FAIL",
        Verdict::NoTests => "NO-TESTS",
        Verdict::WrongTarget => "WRONG-TARGET",
        Verdict::Indeterminate => "INDETERMINATE",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs a command in `dir`, returning (success, merged stdout+stderr).
fn run(dir: &Path, args: &[&str]) -> (bool, String) {
    match Command::new("cargo").args(args).current_dir(dir).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.success(), s)
        }
        Err(e) => (false, format!("error: cannot run cargo: {e}")),
    }
}

/// Runs git in `dir`. `None` means git REFUSED, which is not the same as git printing
/// nothing.
///
/// 103 of 181 worktrees have had their `.git/worktrees/<name>` admin directory pruned. The
/// files survive, the work is on disk, but `git diff` answers "fatal: not a git repository".
/// Both this script's shell ancestor and this module's first draft folded that into an empty
/// string, summed it to 0 lines, and handed `lines_added: Some(0)` to the gate -- which
/// correctly concluded NO-OP, i.e. THE ARM WROTE NOTHING. Twelve `probe` candidates that
/// each wrote a working `short()` were recorded as having done nothing at all.
///
/// That is the project's recurring failure reached for the tenth time, and the worst
/// instance so far: every earlier one lost a signal, this one manufactures a false
/// accusation against the arm and feeds it to the bandit.  (bead farmerbob-jd2.2)
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether any launcher process has `wt` as its working directory.
///
/// This is process IDENTITY, not a name match on a command line: `pgrep -f` matching the
/// harness's own git and worktree processes is bead farmerbob-7e2.
fn process_in(wt: &Path) -> Option<String> {
    let real = fs::canonicalize(wt).ok()?;
    for name in LAUNCHERS {
        let out = Command::new("pgrep").arg("-x").arg(name).output().ok()?;
        for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            let cwd = PathBuf::from(format!("/proc/{pid}/cwd"));
            if fs::canonicalize(&cwd).is_ok_and(|c| c == real) {
                return Some(format!("{name} pid {pid} cwd is the worktree"));
            }
        }
    }
    None
}

/// Folds the two liveness signals into one belief.
///
/// The marker alone is a stale marker: an orphaned run whose supervisor died leaves it
/// behind forever, and the worktree then looks live for good. So a `Working` claim from
/// the marker is RELEASED when no process corroborates it, which is precisely what
/// `fb-score.sh` expressed as `rm -f "$WT/.fb-task.md"`.
fn liveness_of(wt: &Path) -> (Liveness, String) {
    let mut t = Tracker::new();
    let marker = wt.join(MARKER);
    let at = now_ms();

    let claimed_live = marker.exists();
    t.observe(LiveObs {
        authority: Authority::Lifecycle,
        liveness: if claimed_live { Liveness::Working } else { Liveness::Gone },
        at_ms: at,
        evidence: if claimed_live {
            format!("{MARKER} is present")
        } else {
            format!("{MARKER} was removed by the dispatcher on exit")
        },
    });

    let proc = process_in(wt);
    t.observe(LiveObs {
        authority: Authority::Cgroup,
        liveness: if proc.is_some() { Liveness::Working } else { Liveness::Gone },
        at_ms: at,
        evidence: proc.clone().unwrap_or_else(|| "no launcher process in this worktree".into()),
    });

    if claimed_live && proc.is_none() {
        // The marker is orphaned. Drop the lifecycle claim rather than let it outrank the
        // process evidence forever, and clear the file so the five scripts that still read
        // the marker alone stop believing it too.
        t.release(Authority::Lifecycle);
        let _ = fs::remove_file(&marker);
    }

    let b = t.belief();
    (b.liveness, b.evidence)
}

/// Counts tests that actually EXECUTED across every `test result: ok.` line.
fn tests_passed_count(log: &str) -> u32 {
    log.lines()
        .filter_map(|l| l.strip_prefix("test result: ok. "))
        .filter_map(|r| r.split_whitespace().next())
        .filter_map(|n| n.parse::<u32>().ok())
        .sum()
}

fn clippy_warnings(log: &str) -> i32 {
    log.lines()
        .filter(|l| l.starts_with("warning") || l.starts_with("error"))
        .count() as i32
}

/// Measures one worktree. Returns `None` when the run is still live.
fn measure(wt: &Path, bead: &str, src: &str, krate: &str, base_clippy: i32, log_root: &Path) -> Option<Record> {
    let (live, why) = liveness_of(wt);
    if live == Liveness::Working {
        println!("{src:<22} {:<11} (SKIPPED: {why})", "LIVE");
        return None;
    }

    // Every line count below depends on git being able to read this worktree. If it cannot,
    // the count is MISSING, and the gate must return Indeterminate rather than blame the arm.
    let numstat = git(wt, &["diff", "--numstat", "HEAD", "--", "crates/"]);
    let untracked_raw = git(wt, &["ls-files", "--others", "--exclude-standard", "crates/"]);
    let tracked = git(wt, &["diff", "--name-only", "HEAD", "--", "crates/"]);

    let (lines, untracked, crates): (Measurement<u32>, Vec<String>, BTreeSet<String>) =
        match (numstat, untracked_raw, tracked) {
            (Some(ns), Some(ur), Some(tr)) => {
                let mut n: u32 = ns
                    .lines()
                    .filter_map(|l| l.split_whitespace().next())
                    .filter_map(|x| x.parse::<u32>().ok())
                    .sum();
                let untracked: Vec<String> =
                    ur.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
                for f in &untracked {
                    if let Ok(body) = fs::read_to_string(wt.join(f)) {
                        n += body.lines().count() as u32;
                    }
                }
                let crates = tr
                    .lines()
                    .map(str::to_string)
                    .chain(untracked.iter().cloned())
                    .filter_map(|p| p.split('/').nth(1).map(str::to_string))
                    .collect();
                (Measurement::observed(n), untracked, crates)
            }
            _ => (
                Measurement::instrument_failed(
                    "git cannot read this worktree -- its .git/worktrees admin directory was \
                     pruned, so the diff is unavailable even though the files are on disk",
                ),
                Vec::new(),
                BTreeSet::new(),
            ),
        };

    let (built, build_log) = run(wt, &["build", "-p", krate]);
    let mut tests_ok = false;
    let mut tests_run = 0;
    let clippy: Measurement<i32>;
    let mut err = None;

    if built {
        let (ok, log) = run(wt, &["test", "-p", krate]);
        tests_ok = ok;
        tests_run = tests_passed_count(&log);
        let (_, lint) = run(wt, &["clippy", "-p", krate, "--all-targets"]);
        // DELTA, not absolute: counting the crate's total charges every candidate for lint
        // debt it inherited, and since all candidates inherit the same debt the metric reads
        // as a tie rather than as broken.  (bead farmerbob-0fx)
        clippy = Measurement::observed(clippy_warnings(&lint) - base_clippy);
    } else {
        clippy = Measurement::instrument_failed("the crate does not compile, so it has no lint count");
        err = build_log
            .lines()
            .find(|l| l.starts_with("error"))
            .map(|l| l.chars().take(70).collect());
    }

    let verdict = judge(&GateObs {
        built: Some(built),
        tests_passed: Some(tests_ok),
        tests_run: Some(tests_run),
        lines_added: lines.value().copied(),
        declared_targets_present: None,
    });

    let duration_s = read_duration(log_root, bead, src);

    println!(
        "{src:<22} {:<11} tests={tests_run:<3} clippy={:<4} {:>5}L crates={:<2} {:>4}s",
        wire(verdict),
        clippy.value().map(i32::to_string).unwrap_or_else(|| "-".into()),
        lines.value().map(u32::to_string).unwrap_or_else(|| "?".into()),
        crates.len(),
        duration_s.value().map(|d| format!("{d:.0}")).unwrap_or_else(|| "-".into()),
    );

    Some(Record {
        source: src.to_string(),
        verdict,
        build: built,
        tests_ok,
        tests_run,
        lines,
        new_files: untracked.len() as u32,
        crates,
        clippy,
        duration_s,
        liveness: live,
        err,
    })
}

/// Reads the dispatcher's run record for a wallclock duration.
///
/// A missing record is `Missing`, not `-1`. The shell wrote `-1`, a sentinel that any
/// consumer treating duration as a number will happily average.
fn read_duration(log_root: &Path, bead: &str, src: &str) -> Measurement<f64> {
    let p = log_root.join(format!("{bead}--{src}.json"));
    let Ok(body) = fs::read_to_string(&p) else {
        return Measurement::not_attempted();
    };
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) => match v.get("duration_s").and_then(serde_json::Value::as_f64) {
            Some(d) => Measurement::observed(d),
            None => Measurement::nothing_to_measure("the run record has no duration_s"),
        },
        Err(e) => Measurement::untrusted(&format!("run record is not valid JSON: {e}")),
    }
}

fn to_json(r: &Record) -> serde_json::Value {
    serde_json::json!({
        "source": r.source,
        "verdict": wire(r.verdict),
        "build": if r.build { "pass" } else { "FAIL" },
        "test": if r.tests_ok { "pass" } else { "FAIL" },
        "tests_run": r.tests_run,
        "lines": r.lines.value().copied().unwrap_or(0),
        "lines_measured": r.lines.is_observed(),
        "new_files": r.new_files,
        "crates_touched": r.crates.len(),
        "crates": r.crates.iter().cloned().collect::<Vec<_>>().join(","),
        // Legacy spelling: a string, with "-" for unmeasured. Kept because fb-pareto and
        // fb-status parse it. The Measurement is the truth; this is the concession.
        "clippy": r.clippy.value().map(i32::to_string).unwrap_or_else(|| "-".into()),
        "clippy_measured": r.clippy.is_observed(),
        "duration_s": r.duration_s.value().copied().unwrap_or(-1.0),
        "duration_measured": r.duration_s.is_observed(),
        "liveness": format!("{:?}", r.liveness),
        "blames_arm": r.verdict.blames_arm(),
        "err": r.err.clone().unwrap_or_default(),
    })
}

/// Scores every candidate worktree for `bead`. Returns a process exit code.
pub fn run_cmd(bead: &str, krate: &str, json_only: bool) -> i32 {
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            eprintln!("error: HOME is not set");
            return 2;
        }
    };
    let wt_root = home.join(".local/share/farmerbob/worktrees");
    let log_root = home.join(".local/share/farmerbob/logs");
    let out = log_root.join(format!("{bead}.score.json"));

    // Baseline the crate on the repo HEAD, once, so each arm is measured on what it ADDED.
    let repo = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (_, base_log) = run(&repo, &["clippy", "-p", krate, "--all-targets"]);
    let base_clippy = clippy_warnings(&base_log);
    if !json_only {
        println!("clippy baseline: {krate} on HEAD = {base_clippy}");
    }

    let prefix = format!("{bead}--");
    let mut dirs: Vec<PathBuf> = match fs::read_dir(&wt_root) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
            })
            .collect(),
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", wt_root.display());
            return 2;
        }
    };
    dirs.sort();

    let mut records = Vec::new();
    for wt in &dirs {
        let name = wt.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let src = name.strip_prefix(&prefix).unwrap_or(name);
        if let Some(r) = measure(wt, bead, src, krate, base_clippy, &log_root) {
            records.push(r);
        }
    }

    let body: Vec<serde_json::Value> = records.iter().map(to_json).collect();
    let rendered = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "[]".into());
    if let Err(e) = fs::write(&out, &rendered) {
        eprintln!("error: cannot write {}: {e}", out.display());
        return 2;
    }
    if json_only {
        println!("{rendered}");
    } else {
        println!("-> {}", out.display());
    }
    if records.is_empty() {
        // No candidate measured is NOT "every candidate failed". Say so with an exit code
        // the caller can distinguish, rather than writing an empty array and moving on.
        eprintln!("no candidate worktrees for {bead} (looked in {})", wt_root.display());
        return 3;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verdict_has_a_distinct_wire_spelling() {
        let all = [
            Verdict::Pass, Verdict::NoOp, Verdict::NoCompile, Verdict::TestsFail,
            Verdict::NoTests, Verdict::WrongTarget, Verdict::Indeterminate,
        ];
        let spellings: BTreeSet<&str> = all.iter().map(|v| wire(*v)).collect();
        assert_eq!(spellings.len(), all.len(), "two verdicts share a wire spelling");
    }

    #[test]
    fn the_wire_spellings_are_the_ones_the_shell_greps_for() {
        // fb-status.sh, fb-pareto.sh and fb-verdict.sh all match these literals.
        assert_eq!(wire(Verdict::Pass), "PASS");
        assert_eq!(wire(Verdict::NoOp), "NO-OP");
        assert_eq!(wire(Verdict::NoTests), "NO-TESTS");
        assert_eq!(wire(Verdict::Indeterminate), "INDETERMINATE");
    }

    #[test]
    fn tests_run_sums_every_result_line_not_just_the_first() {
        let log = "\
test result: ok. 12 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored
";
        assert_eq!(tests_passed_count(log), 19);
    }

    #[test]
    fn a_filtered_run_that_matched_nothing_counts_zero_not_success() {
        // The first bug ever filed in this project: `ok. 0 passed` is a REAL zero.
        let log = "test result: ok. 0 passed; 0 failed; 0 ignored; 3 filtered out\n";
        assert_eq!(tests_passed_count(log), 0);
        assert_eq!(
            judge(&GateObs {
                built: Some(true),
                tests_passed: Some(true),
                tests_run: Some(0),
                lines_added: Some(40),
                declared_targets_present: None,
            }),
            Verdict::NoTests
        );
    }

    #[test]
    fn a_failed_build_leaves_clippy_missing_rather_than_zero() {
        let m: Measurement<i32> =
            Measurement::instrument_failed("the crate does not compile, so it has no lint count");
        assert!(!m.is_observed());
        assert_eq!(m.value(), None, "an unmeasured lint count must not read as clean");
    }

    #[test]
    fn a_clean_candidate_on_a_dirty_base_scores_negative_not_zero() {
        // Cleaning up inherited warnings deserves the credit; the delta keeps the sign.
        assert_eq!(clippy_warnings("warning: a\nwarning: b\n") - 6, -4);
    }

    #[test]
    fn clippy_counts_only_leading_warnings_not_mentions_in_prose() {
        let log = "   note: this warning originates in a macro\nwarning: unused import\n";
        assert_eq!(clippy_warnings(log), 1);
    }

    #[test]
    fn a_missing_run_record_is_missing_not_minus_one() {
        let d = read_duration(Path::new("/nonexistent-log-root"), "notask", "nobody");
        assert!(!d.is_observed());
        assert_eq!(d.value(), None);
    }
}

#[cfg(test)]
mod broken_worktree {
    use super::*;

    /// The tenth instance of this project's recurring failure, and the first that
    /// manufactures a false accusation rather than merely losing a signal.
    #[test]
    fn an_unreadable_worktree_is_indeterminate_not_a_no_op() {
        // git refused, so lines_added is absent.
        let unreadable = judge(&GateObs {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(39),
            lines_added: None,
            declared_targets_present: None,
        });
        assert_eq!(unreadable, Verdict::Indeterminate);
        assert!(
            !unreadable.blames_arm(),
            "a gate that cannot see must not blame the arm"
        );

        // git answered, and the answer was zero: that IS the arm's fault.
        let truly_empty = judge(&GateObs {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(39),
            lines_added: Some(0),
            declared_targets_present: None,
        });
        assert_eq!(truly_empty, Verdict::NoOp);
        assert!(truly_empty.blames_arm());

        assert_ne!(
            unreadable, truly_empty,
            "cannot-measure and wrote-nothing must never reach the same verdict"
        );
    }

    #[test]
    fn a_missing_line_count_is_not_silently_zero() {
        let m: Measurement<u32> = Measurement::instrument_failed("git cannot read this worktree");
        assert_eq!(m.value(), None);
        assert!(!m.is_observed());
    }
}
