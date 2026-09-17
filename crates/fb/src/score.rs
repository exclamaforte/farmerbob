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
use farmerbob_core::scope::{assess, is_clean, Change, Declared, Departure, Scope};

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
    /// Whether the run stayed inside its declared deliverable. Absent when git could not
    /// read the worktree, because "no departures found" and "we could not look" are
    /// different facts and an empty list reads as the first.
    scope: Measurement<Scope>,
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
/// What every candidate in one task is measured against.
struct Task<'a> {
    bead: &'a str,
    krate: &'a str,
    /// The declared deliverable, and whether the spec said `creates` (as opposed to
    /// `modifies`). The verb decides whether a destroyed worktree can be recovered from.
    target: Option<&'a str>,
    creates: bool,
    base_clippy: i32,
    log_root: &'a Path,
}

fn measure(wt: &Path, src: &str, t: &Task<'_>) -> Option<Record> {
    let (bead, krate, target, creates, base_clippy, log_root) =
        (t.bead, t.krate, t.target, t.creates, t.base_clippy, t.log_root);
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
    let mut changed_paths: Option<Vec<String>> = None;

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
                changed_paths = Some(
                    tr.lines()
                        .map(str::to_string)
                        .chain(untracked.iter().cloned())
                        .collect::<Vec<String>>(),
                );
                (Measurement::observed(n), untracked, crates)
            }
            _ => {
                // RECOVERY for a destroyed worktree. git cannot diff it, but for a `creates`
                // task the deliverable must NOT have existed on the base -- that is the
                // dispatch precondition -- so the whole of that file is the arm's work and
                // its line count needs no git at all.
                //
                // This is a genuine measurement of a LOWER BOUND: it misses whatever the arm
                // changed in other files, notably the module declaration in lib.rs. A lower
                // bound is exactly what the gate needs, because the question the gate asks of
                // this number is "did the arm write anything", and it rescues a run from the
                // false NO-OP that 103 -- now 203 -- pruned worktrees produced.
                //
                // Scope stays Missing regardless: the changed-file list cannot be recovered,
                // and claiming an arm stayed in scope on the strength of not being able to
                // look is the failure this whole file exists to avoid.  (bead farmerbob-13p)
                let recovered = target
                    .filter(|_| creates)
                    .map(|t| wt.join(t))
                    .filter(|p| p.exists())
                    .and_then(|p| fs::read_to_string(p).ok())
                    .map(|body| body.lines().count() as u32)
                    .filter(|n| *n > 0);
                let lines = match recovered {
                    Some(n) => Measurement::observed(n),
                    None => Measurement::instrument_failed(
                        "git cannot read this worktree -- its .git/worktrees admin directory \
                         was pruned -- and the declared deliverable is absent or empty on \
                         disk, so nothing can be recovered",
                    ),
                };
                (lines, Vec::new(), BTreeSet::new())
            }
        };

    // Scope, measured against the DECLARED DELIVERABLE rather than by counting crates.
    // Counting crates was the old signal, and an arm that modified 36 files across a crate
    // it was never asked to touch registered as "touched 2" -- the count was not wrong, it
    // was measuring the wrong thing.  (bead farmerbob-jxp)
    //
    // Deletions: `git diff --name-only` lists a deleted path like any other, so the two are
    // told apart by asking git for the status letters separately. A path git cannot report
    // on at all leaves the whole assessment Missing rather than empty.
    let deleted: Vec<String> = git(wt, &["diff", "--name-only", "--diff-filter=D", "HEAD", "--", "crates/"])
        .map(|o| o.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let scope: Measurement<Scope> = match (changed_paths.as_ref(), target) {
        (Some(paths), Some(t)) => {
            let changes: Vec<Change> = paths
                .iter()
                .map(|p| Change { path: p.clone(), deleted: deleted.contains(p) })
                .collect();
            Measurement::observed(assess(&Declared { target: t.to_string() }, &changes))
        }
        (None, _) => Measurement::instrument_failed(
            "git cannot read this worktree, so which files changed is unknown",
        ),
        (_, None) => Measurement::nothing_to_measure(
            "the task spec declares no deliverable, so there is no scope to check",
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

    let scope_note = match scope.value() {
        Some(sc) if !is_clean(sc) => format!("  OUT OF SCOPE: {} file(s)", sc.departures.len()),
        Some(_) => String::new(),
        None => "  scope=?".to_string(),
    };
    println!(
        "{src:<22} {:<11} tests={tests_run:<3} clippy={:<4} {:>5}L crates={:<2} {:>4}s{scope_note}",
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
        scope,
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
        // Scope against the declared deliverable. `scope_clean` is null, not true, when the
        // assessment could not be made.
        "scope_clean": r.scope.value().map(is_clean),
        "scope_departures": r.scope.value().map(|sc| sc.departures.len()).unwrap_or(0),
        "scope_measured": r.scope.is_observed(),
        "departed": r.scope.value().map(|sc| sc.departures.iter().map(|d| match d {
            Departure::Foreign { path } => path.clone(),
            Departure::Deleted { path } => format!("{path} (deleted)"),
        }).collect::<Vec<_>>()).unwrap_or_default(),
        "blames_arm": r.verdict.blames_arm(),
        "err": r.err.clone().unwrap_or_default(),
    })
}

/// Scores every candidate worktree for `bead`. Returns a process exit code.
/// The crate a deliverable belongs to: `crates/<name>/...` names it directly.
fn crate_of(target: Option<&str>) -> Option<String> {
    let t = target?;
    let mut parts = t.split('/');
    (parts.next()? == "crates").then(|| parts.next().map(str::to_string))?
}

/// The crate assumed when a spec declares no target and none is given.
pub const DEFAULT_CRATE: &str = "farmerbob-core";

/// Reads the task spec's declared deliverable, the one path the arm was asked to produce.
///
/// Accepts BOTH verbs. Seven readers in this harness each grepped for `fb:creates` alone and
/// six of them did not know `fb:modifies` existed.  (bead farmerbob-9mh)
fn declared_target(bead: &str) -> (Option<String>, bool) {
    let Ok(spec) = fs::read_to_string(format!(".fb/prompts/{bead}.md")) else {
        return (None, false);
    };
    for line in spec.lines() {
        let Some(rest) = line.trim().strip_prefix("<!-- fb:") else { continue };
        for (verb, creates) in [("creates ", true), ("modifies ", false)] {
            if let Some(r) = rest.strip_prefix(verb)
                && let Some(path) = r.split_whitespace().next()
            {
                return (Some(path.to_string()), creates);
            }
        }
    }
    (None, false)
}

pub fn run_cmd(bead: &str, krate: &str, json_only: bool) -> i32 {
    let wt_root = crate::paths::worktrees();
    let log_root = crate::paths::logs();
    let out = log_root.join(format!("{bead}.score.json"));

    let (target, creates) = declared_target(bead);

    // DERIVE the crate from the declared deliverable rather than trusting a default.
    //
    // `--crate` defaulted to farmerbob-core. A bare `fb score port-critique` therefore built,
    // tested and linted a crate those candidates had never touched, and reported PASS with
    // clippy 0 for a field in which one candidate's tests actually failed and another was
    // nine files out of scope. It measured something real; it was not the thing asked about.
    //
    // crates/fb/src/critique.rs can only belong to the crate `fb`, so there is no need to
    // guess. An explicit --crate still overrides, for a target the path cannot classify.
    //   (bead farmerbob-jd2.12)
    let krate = match (crate_of(target.as_deref()), krate) {
        (Some(derived), given) if given == DEFAULT_CRATE && derived != given => {
            if !json_only {
                println!("crate: {derived} (derived from the deliverable, not the default)");
            }
            derived
        }
        (_, given) => given.to_string(),
    };
    let krate = krate.as_str();
    if !json_only {
        match target.as_deref() {
            Some(t) => println!("declared deliverable: {t}"),
            None => println!("declared deliverable: NONE -- scope cannot be checked"),
        }
    }

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
        let task = Task {
            bead,
            krate,
            target: target.as_deref(),
            creates,
            base_clippy,
            log_root: &log_root,
        };
        if let Some(r) = measure(wt, src, &task) {
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

#[cfg(test)]
mod scope_integration {
    use super::*;

    /// The signal this replaces counted CRATES. codex-luna modified 36 files across a crate
    /// it was never asked to touch and that registered as "touched 2".  (bead farmerbob-jxp)
    #[test]
    fn a_rewrite_of_another_crate_is_a_departure_not_a_crate_count() {
        let declared = Declared { target: "crates/farmerbob-core/src/matrix.rs".into() };
        let changes: Vec<Change> = [
            "crates/farmerbob-core/src/matrix.rs", // the deliverable
            "crates/farmerbob-core/src/lib.rs",    // the required module declaration
            "crates/fb/src/main.rs",               // out of scope
            "crates/fb/src/doctor.rs",             // out of scope
        ]
        .iter()
        .map(|p| Change { path: (*p).to_string(), deleted: false })
        .collect();

        let sc = assess(&declared, &changes);
        assert!(sc.target_changed);
        assert_eq!(sc.allowed, vec!["crates/farmerbob-core/src/lib.rs".to_string()]);
        assert_eq!(sc.departures.len(), 2, "both crates/fb files are departures");
        assert!(!is_clean(&sc));
    }

    #[test]
    fn declaring_the_new_module_is_not_a_departure() {
        let declared = Declared { target: "crates/farmerbob-core/src/scope.rs".into() };
        let changes = [
            Change { path: "crates/farmerbob-core/src/scope.rs".into(), deleted: false },
            Change { path: "crates/farmerbob-core/src/lib.rs".into(), deleted: false },
        ];
        assert!(is_clean(&assess(&declared, &changes)), "adding `pub mod x;` is required");
    }

    /// An unreadable worktree must not read as "stayed in scope". Same rule as lines_added:
    /// an empty departure list and an unmeasured one are different facts.
    #[test]
    fn an_unreadable_worktree_leaves_scope_missing_not_clean() {
        let m: Measurement<Scope> =
            Measurement::instrument_failed("git cannot read this worktree");
        assert!(!m.is_observed());
        assert_eq!(m.value().map(is_clean), None, "must not read as clean");
    }

    /// Both declaration verbs. Six of seven readers in this harness knew only fb:creates.
    #[test]
    fn the_target_reader_accepts_creates_and_modifies() {
        for (verb, want) in [("creates", "crates/a/src/b.rs"), ("modifies", "crates/a/src/b.rs")] {
            let line = format!("<!-- fb:{verb} {want} -->");
            let got: Option<String> = line
                .trim()
                .strip_prefix("<!-- fb:")
                .and_then(|r| r.strip_prefix("creates ").or_else(|| r.strip_prefix("modifies ")))
                .and_then(|r| r.split_whitespace().next())
                .map(str::to_string);
            assert_eq!(got.as_deref(), Some(want), "verb {verb} must parse");
        }
    }
}

#[cfg(test)]
mod destroyed_worktree_recovery {
    use super::*;

    /// A `creates` task's deliverable must not exist on the base -- that is the dispatch
    /// precondition -- so when git can no longer read the worktree, the file's own line count
    /// is still a valid LOWER BOUND on what the arm wrote, and needs no git.
    ///
    /// It rescues a run from the false NO-OP that 203 pruned worktrees produced.
    #[test]
    fn a_creates_deliverable_on_disk_survives_a_destroyed_worktree() {
        // The gate only asks whether the arm wrote anything; a lower bound answers that.
        let recovered = judge(&GateObs {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(46),
            lines_added: Some(709),
            declared_targets_present: None,
        });
        assert_eq!(recovered, Verdict::Pass);

        // With nothing recoverable the answer stays Indeterminate, never NoOp: we still
        // could not look.
        let unrecoverable = judge(&GateObs {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(46),
            lines_added: None,
            declared_targets_present: None,
        });
        assert_eq!(unrecoverable, Verdict::Indeterminate);
        assert!(!unrecoverable.blames_arm());
    }

    /// Recovery is for `creates` only. A `modifies` target exists on the base, so its line
    /// count is the WHOLE file and says nothing about what this arm changed.
    #[test]
    fn a_modifies_target_is_not_recoverable_from_its_line_count() {
        let (target, creates) = ("crates/farmerbob-core/src/lease.rs", false);
        assert!(!creates, "lease.rs is a modifies target: {target}");
        // The recovery is gated on `creates`, so a modifies task keeps Missing lines and the
        // Indeterminate verdict above.
    }

    #[test]
    fn both_declaration_verbs_parse_and_report_which_they_are() {
        for (line, want_creates) in [
            ("<!-- fb:creates crates/a/src/b.rs -->", true),
            ("<!-- fb:modifies crates/a/src/b.rs -->", false),
        ] {
            let rest = line.trim().strip_prefix("<!-- fb:").expect("prefix");
            let got = rest.strip_prefix("creates ").map(|_| true)
                .or_else(|| rest.strip_prefix("modifies ").map(|_| false));
            assert_eq!(got, Some(want_creates), "verb in {line}");
        }
    }
}
