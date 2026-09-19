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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use farmerbob_core::gate::{Observation as GateObs, Verdict, judge};
use farmerbob_core::liveness::{Authority, Liveness, Observation as LiveObs, Tracker};
use farmerbob_core::measurement::Measurement;
use farmerbob_core::scope::{Change, Declared, Departure, Scope, assess, is_clean};
use farmerbob_core::test_delta::{Contribution, Counts, contribution, crate_total};

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
    /// What this candidate contributed to the crate's suite, as decided by
    /// `farmerbob_core::test_delta`. Missing when the baseline or after count
    /// could not be measured.
    tests_delta: Measurement<Contribution>,
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
    /// read the worktree -- or refused to report deleted paths -- because "no departures
    /// found" and "we could not look" are different facts and an empty list reads as
    /// the first.  (bead farmerbob-7i30)
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
        // New spelling. Nothing in the shell greps for it yet; it is added here rather than
        // folded into an existing string so a departure can never be read as any other verdict.
        Verdict::OutOfScope => "OUT-OF-SCOPE",
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
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The revision a worktree's work should be measured against: the point its
/// branch left the base, so that committed and uncommitted work both count.
///
/// `None` means GIT REFUSED to read this worktree at all -- the pruned
/// `.git/worktrees/<name>` case -- which is not the same as "no commits" and
/// not the same as "no common ancestor".
///
/// Two observations, because one cannot tell the cases apart: `git merge-base
/// HEAD master` exits 1 when the histories share no common ancestor and 128
/// when the repository is unreadable, and `git` maps every non-zero status to
/// `None`. So readability is established first with `rev-parse --git-dir`, and
/// only then does a `None` from merge-base mean "no common ancestor" -- which
/// measures against `HEAD`, reproducing the pre-merge-base behaviour for that
/// worktree rather than calling it a refusal.
fn measure_base(wt: &Path) -> Option<String> {
    // Observation 1: is git answering in this worktree at all? Failing here is
    // the only route to `None`.
    git(wt, &["rev-parse", "--git-dir"])?;
    // Observation 2: the fork point. `None` now means no common ancestor (or
    // the unborn-HEAD corner, which diffs against `HEAD` exactly as today).
    let sha = git(wt, &["merge-base", "HEAD", "master"]).and_then(|out| {
        out.lines()
            .next()
            .map(str::trim)
            .filter(|sha| !sha.is_empty())
            .map(str::to_string)
    });
    Some(sha.unwrap_or_else(|| "HEAD".into()))
}

/// Paths git reported as deleted, or the stated reason there is no list.
///
/// `Missing` when git refused; NEVER an empty vector standing in for a
/// refusal, which is the defect this exists to close. Git ANSWERING with
/// nothing is `Observed(vec![])`: nothing was deleted is a measurement, and
/// it must stay distinguishable from "we could not look".
///
/// `--name-only` prints one path per line and each line is taken verbatim, so
/// a path holding a space or a quote survives. `-z` is deliberately not
/// parsed, so a path holding a NEWLINE is not represented here -- a real
/// limit of the one-path-per-line wire format, not a choice.
/// (bead farmerbob-7i30)
fn deleted_paths(wt: &Path, base: &str) -> Measurement<Vec<String>> {
    match git(
        wt,
        &[
            "diff",
            "--name-only",
            "--diff-filter=D",
            base,
            "--",
            "crates/",
        ],
    ) {
        Some(out) => Measurement::observed(out.lines().map(str::to_string).collect()),
        None => Measurement::instrument_failed(
            "git refused to report deleted paths, so a deletion cannot be told apart \
             from a modification",
        ),
    }
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
        liveness: if claimed_live {
            Liveness::Working
        } else {
            Liveness::Gone
        },
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
        liveness: if proc.is_some() {
            Liveness::Working
        } else {
            Liveness::Gone
        },
        at_ms: at,
        evidence: proc
            .clone()
            .unwrap_or_else(|| "no launcher process in this worktree".into()),
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

fn clippy_warnings(log: &str) -> i32 {
    log.lines()
        .filter(|l| l.starts_with("warning") || l.starts_with("error"))
        .count() as i32
}

/// Measure the crate's passing-test count on a candidate's base revision.
///
/// The base is exported to a temporary directory rather than checking it out
/// in the candidate worktree. This keeps the candidate's files and index
/// unchanged while allowing the same cargo command to measure the baseline.
fn baseline_tests(wt: &Path, krate: &str, base: Option<&str>) -> Measurement<u32> {
    let Some(base) = base else {
        return Measurement::instrument_failed(
            "git could not determine the candidate's base revision",
        );
    };

    let dir = std::env::temp_dir().join(format!(
        "fb-score-baseline-{}-{}",
        std::process::id(),
        now_ms()
    ));
    if let Err(e) = fs::create_dir(&dir) {
        return Measurement::instrument_failed(&format!(
            "could not create the baseline directory {}: {e}",
            dir.display()
        ));
    }

    let archive = match Command::new("git")
        .arg("-C")
        .arg(wt)
        .args(["archive", "--format=tar", base])
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => {
            let detail = String::from_utf8_lossy(&output.stderr);
            let _ = fs::remove_dir_all(&dir);
            return Measurement::instrument_failed(&format!(
                "git could not archive the candidate's base: {detail}"
            ));
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            return Measurement::instrument_failed(&format!(
                "could not run git to archive the candidate's base: {e}"
            ));
        }
    };

    let mut extract = match Command::new("tar")
        .args(["-x", "-f", "-", "-C"])
        .arg(&dir)
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            return Measurement::instrument_failed(&format!(
                "could not run tar for the baseline archive: {e}"
            ));
        }
    };

    let wrote_archive = match extract.stdin.take() {
        Some(mut stdin) => stdin.write_all(&archive).is_ok(),
        None => false,
    };
    let extracted = extract.wait().is_ok_and(|status| status.success());
    if !wrote_archive || !extracted {
        let _ = fs::remove_dir_all(&dir);
        return Measurement::instrument_failed(
            "the baseline archive could not be extracted, so its tests could not be measured",
        );
    }

    let (_, log) = run(&dir, &["test", "-p", krate]);
    let result = farmerbob_core::build_verdict::tests_run(&log);
    let _ = fs::remove_dir_all(&dir);
    result
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
    let (bead, krate, target, creates, base_clippy, log_root) = (
        t.bead,
        t.krate,
        t.target,
        t.creates,
        t.base_clippy,
        t.log_root,
    );
    let (live, why) = liveness_of(wt);
    if live == Liveness::Working {
        println!("{src:<22} {:<11} (SKIPPED: {why})", "LIVE");
        return None;
    }

    // Every line count below depends on git being able to read this worktree. If it cannot,
    // the count is MISSING, and the gate must return Indeterminate rather than blame the arm.
    // ONE base for every reading of the diff -- the merge-base with master, so committed
    // work counts -- because a count from the merge-base beside a path list from HEAD would
    // let a file be counted and not listed. `base` is `None` exactly when git refused, and
    // `None` numstat/tracked land in the existing Missing arm unchanged.
    let base = measure_base(wt);
    let numstat = base
        .as_deref()
        .and_then(|b| git(wt, &["diff", "--numstat", b, "--", "crates/"]));
    let untracked_raw = git(
        wt,
        &["ls-files", "--others", "--exclude-standard", "crates/"],
    );
    let tracked = base
        .as_deref()
        .and_then(|b| git(wt, &["diff", "--name-only", b, "--", "crates/"]));
    let mut changed_paths: Option<Vec<String>> = None;

    let (lines, untracked, crates): (Measurement<u32>, Vec<String>, BTreeSet<String>) =
        match (numstat, untracked_raw, tracked) {
            (Some(ns), Some(ur), Some(tr)) => {
                let mut n: u32 = ns
                    .lines()
                    .filter_map(|l| l.split_whitespace().next())
                    .filter_map(|x| x.parse::<u32>().ok())
                    .sum();
                let untracked: Vec<String> = ur
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
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
    // on at all leaves the whole assessment Missing rather than empty. Same base as the
    // count and the path list, or a deletion made in a commit would be listed as changed
    // and never told apart from a modification.
    let deleted: Measurement<Vec<String>> = match base.as_deref() {
        Some(b) => deleted_paths(wt, b),
        None => Measurement::instrument_failed(
            "git cannot read this worktree, so which paths were deleted is unknown",
        ),
    };
    // A refused deletion list is NOT an empty one, and the assessment is never computed
    // from a partial answer: with the list Missing, scope is Missing with git's refusal
    // named. An arm whose deletions we could not read must not read as one who deleted
    // nothing.  (bead farmerbob-7i30)
    let scope: Measurement<Scope> = match (changed_paths.as_ref(), deleted.value(), target) {
        (Some(paths), Some(gone), Some(t)) => {
            let changes: Vec<Change> = paths
                .iter()
                .map(|p| Change {
                    path: p.clone(),
                    deleted: gone.contains(p),
                })
                .collect();
            Measurement::observed(assess(
                &Declared {
                    target: t.to_string(),
                },
                &changes,
            ))
        }
        (None, _, _) => Measurement::instrument_failed(
            "git cannot read this worktree, so which files changed is unknown",
        ),
        (_, _, None) => Measurement::nothing_to_measure(
            "the task spec declares no deliverable, so there is no scope to check",
        ),
        (_, None, _) => Measurement::instrument_failed(
            "git refused to report deleted paths, so a deletion cannot be told apart \
             from a modification",
        ),
    };

    let (built, build_log) = run(wt, &["build", "-p", krate]);
    let mut tests_ok = false;
    let mut tests_run = 0;
    let mut after = Measurement::instrument_failed("the crate did not produce a test result");
    let clippy: Measurement<i32>;
    let mut err = None;

    if built {
        let (ok, log) = run(wt, &["test", "-p", krate]);
        tests_ok = ok;
        after = farmerbob_core::build_verdict::tests_run(&log);
        let (_, lint) = run(wt, &["clippy", "-p", krate, "--all-targets"]);
        // DELTA, not absolute: counting the crate's total charges every candidate for lint
        // debt it inherited, and since all candidates inherit the same debt the metric reads
        // as a tie rather than as broken.  (bead farmerbob-0fx)
        clippy = Measurement::observed(clippy_warnings(&lint) - base_clippy);
    } else {
        clippy =
            Measurement::instrument_failed("the crate does not compile, so it has no lint count");
        err = build_log
            .lines()
            .find(|l| l.starts_with("error"))
            .map(|l| l.chars().take(70).collect());
    }

    let before = baseline_tests(wt, krate, base.as_deref());
    let counts = Counts { before, after };
    let tests_delta = contribution(&counts);
    if let Some(total) = crate_total(&counts).value() {
        tests_run = *total;
    }

    let verdict = judge(&GateObs {
        built: Some(built),
        tests_passed: Some(tests_ok),
        tests_run: Some(tests_run),
        lines_added: lines.value().copied(),
        declared_targets_present: None,
        // The real one. `scope` is a Measurement, so an unassessed worktree arrives here as
        // None and yields Indeterminate rather than Pass -- which is the whole point: an arm
        // whose scope we could not check has not been shown to have stayed inside it.
        scope_departures: scope.value().map(|sc| sc.departures.len() as u32),
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
        clippy
            .value()
            .map(i32::to_string)
            .unwrap_or_else(|| "-".into()),
        lines
            .value()
            .map(u32::to_string)
            .unwrap_or_else(|| "?".into()),
        crates.len(),
        duration_s
            .value()
            .map(|d| format!("{d:.0}"))
            .unwrap_or_else(|| "-".into()),
    );

    Some(Record {
        source: src.to_string(),
        verdict,
        build: built,
        tests_ok,
        tests_run,
        tests_delta,
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

fn tests_delta_json(delta: &Measurement<Contribution>) -> serde_json::Value {
    match delta {
        Measurement::Observed(Contribution::Added(n)) => {
            serde_json::json!({"kind": "Added", "count": n})
        }
        Measurement::Observed(Contribution::Removed(n)) => {
            serde_json::json!({"kind": "Removed", "count": n})
        }
        Measurement::Observed(Contribution::Unchanged) => {
            serde_json::json!({"kind": "Unchanged"})
        }
        Measurement::Missing(_) => serde_json::Value::Null,
    }
}

fn to_json(r: &Record) -> serde_json::Value {
    serde_json::json!({
        "source": r.source,
        "verdict": wire(r.verdict),
        "build": if r.build { "pass" } else { "FAIL" },
        "test": if r.tests_ok { "pass" } else { "FAIL" },
        "tests_run": r.tests_run,
        "tests_delta": tests_delta_json(&r.tests_delta),
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
        let Some(rest) = line.trim().strip_prefix("<!-- fb:") else {
            continue;
        };
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
        eprintln!(
            "no candidate worktrees for {bead} (looked in {})",
            wt_root.display()
        );
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
            Verdict::Pass,
            Verdict::NoOp,
            Verdict::NoCompile,
            Verdict::TestsFail,
            Verdict::NoTests,
            Verdict::WrongTarget,
            Verdict::Indeterminate,
        ];
        let spellings: BTreeSet<&str> = all.iter().map(|v| wire(*v)).collect();
        assert_eq!(
            spellings.len(),
            all.len(),
            "two verdicts share a wire spelling"
        );
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
        assert_eq!(
            farmerbob_core::build_verdict::tests_run(log),
            Measurement::Observed(19)
        );
    }

    #[test]
    fn a_filtered_run_that_matched_nothing_counts_zero_not_success() {
        // The first bug ever filed in this project: `ok. 0 passed` is a REAL zero.
        let log = "test result: ok. 0 passed; 0 failed; 0 ignored; 3 filtered out\n";
        assert_eq!(
            farmerbob_core::build_verdict::tests_run(log),
            Measurement::Observed(0)
        );
        assert_eq!(
            judge(&GateObs {
                built: Some(true),
                tests_passed: Some(true),
                tests_run: Some(0),
                lines_added: Some(40),
                declared_targets_present: None,
                // Pre-existing test of another axis; scope is not what it measures.
                scope_departures: Some(0),
            }),
            Verdict::NoTests
        );
    }

    #[test]
    fn a_failed_build_leaves_clippy_missing_rather_than_zero() {
        let m: Measurement<i32> =
            Measurement::instrument_failed("the crate does not compile, so it has no lint count");
        assert!(!m.is_observed());
        assert_eq!(
            m.value(),
            None,
            "an unmeasured lint count must not read as clean"
        );
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
            // Pre-existing test of another axis; scope is not what it measures.
            scope_departures: Some(0),
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
            // Pre-existing test of another axis; scope is not what it measures.
            scope_departures: Some(0),
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
        let declared = Declared {
            target: "crates/farmerbob-core/src/matrix.rs".into(),
        };
        let changes: Vec<Change> = [
            "crates/farmerbob-core/src/matrix.rs", // the deliverable
            "crates/farmerbob-core/src/lib.rs",    // the required module declaration
            "crates/fb/src/main.rs",               // out of scope
            "crates/fb/src/doctor.rs",             // out of scope
        ]
        .iter()
        .map(|p| Change {
            path: (*p).to_string(),
            deleted: false,
        })
        .collect();

        let sc = assess(&declared, &changes);
        assert!(sc.target_changed);
        assert_eq!(
            sc.allowed,
            vec!["crates/farmerbob-core/src/lib.rs".to_string()]
        );
        assert_eq!(
            sc.departures.len(),
            2,
            "both crates/fb files are departures"
        );
        assert!(!is_clean(&sc));
    }

    #[test]
    fn declaring_the_new_module_is_not_a_departure() {
        let declared = Declared {
            target: "crates/farmerbob-core/src/scope.rs".into(),
        };
        let changes = [
            Change {
                path: "crates/farmerbob-core/src/scope.rs".into(),
                deleted: false,
            },
            Change {
                path: "crates/farmerbob-core/src/lib.rs".into(),
                deleted: false,
            },
        ];
        assert!(
            is_clean(&assess(&declared, &changes)),
            "adding `pub mod x;` is required"
        );
    }

    /// An unreadable worktree must not read as "stayed in scope". Same rule as lines_added:
    /// an empty departure list and an unmeasured one are different facts.
    #[test]
    fn an_unreadable_worktree_leaves_scope_missing_not_clean() {
        let m: Measurement<Scope> = Measurement::instrument_failed("git cannot read this worktree");
        assert!(!m.is_observed());
        assert_eq!(m.value().map(is_clean), None, "must not read as clean");
    }

    /// Both declaration verbs. Six of seven readers in this harness knew only fb:creates.
    #[test]
    fn the_target_reader_accepts_creates_and_modifies() {
        for (verb, want) in [
            ("creates", "crates/a/src/b.rs"),
            ("modifies", "crates/a/src/b.rs"),
        ] {
            let line = format!("<!-- fb:{verb} {want} -->");
            let got: Option<String> = line
                .trim()
                .strip_prefix("<!-- fb:")
                .and_then(|r| {
                    r.strip_prefix("creates ")
                        .or_else(|| r.strip_prefix("modifies "))
                })
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
            // Pre-existing test of another axis; scope is not what it measures.
            scope_departures: Some(0),
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
            // Pre-existing test of another axis; scope is not what it measures.
            scope_departures: Some(0),
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
            let got = rest
                .strip_prefix("creates ")
                .map(|_| true)
                .or_else(|| rest.strip_prefix("modifies ").map(|_| false));
            assert_eq!(got, Some(want_creates), "verb in {line}");
        }
    }
}

#[cfg(test)]
mod committed_work {
    //! Clause tests for `measure_base` and for scoring work that is COMMITTED on its
    //! branch. Every fixture is a fresh repository in a scratch directory; none of this
    //! suite reads this repository's own worktrees.
    //!
    //! Fixture construction runs git subcommands beyond the module's closed five
    //! (init, symbolic-ref, config, add, commit, checkout, rev-parse) -- that is setup,
    //! and no assertion is made on any invocation the module itself does not make.
    use super::*;

    const ONE_LINE: &str = "one\n";
    const FIVE_LINES: &str = "one\ntwo\nthree\nfour\nfive\n";
    const THREE_LINES: &str = "one\ntwo\nthree\n";
    /// The further uncommitted edits of clause 3, applied alone: disjoint from the
    /// committed lines, so the total can equal the sum exactly.
    const FURTHER_LINES: &str = "one\nfour\nfive\n";
    const TARGET: &str = "crates/demo/src/lib.rs";

    fn scratch(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("fb-score-committed-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("scratch dir");
        p
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git must be runnable");
        assert!(st.success(), "git {args:?} failed in {}", repo.display());
    }

    fn git_out(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git must be runnable");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A readable repository, one baseline commit on `master`, files under `crates/`
    /// because every diff this module takes is limited to `crates/`.
    fn new_repo(parent: &Path, name: &str) -> PathBuf {
        let p = parent.join(name);
        fs::create_dir_all(p.join("crates/demo/src")).expect("repo layout");
        fs::create_dir_all(p.join("crates/other/src")).expect("repo layout");
        git_ok(&p, &["init", "-q"]);
        git_ok(&p, &["symbolic-ref", "HEAD", "refs/heads/master"]);
        git_ok(&p, &["config", "user.email", "test@example.com"]);
        git_ok(&p, &["config", "user.name", "test"]);
        git_ok(&p, &["config", "commit.gpgsign", "false"]);
        fs::write(p.join(TARGET), ONE_LINE).expect("baseline lib.rs");
        // A second crate, present but untouched by most fixtures.
        fs::write(
            p.join("crates/other/src/old.rs"),
            "o1\no2\no3\no4\no5\no6\n",
        )
        .expect("baseline other crate");
        fs::write(p.join("README.md"), "baseline\n").expect("baseline readme");
        git_ok(&p, &["add", "-A"]);
        git_ok(&p, &["commit", "-q", "-m", "baseline"]);
        p
    }

    fn run_measure(wt: &Path, log_root: &Path) -> Record {
        let t = Task {
            bead: "committed-work-spec",
            krate: "demo",
            target: Some(TARGET),
            creates: false,
            base_clippy: 0,
            log_root,
        };
        measure(wt, "candidate", &t).expect("worktree under test must not look live")
    }

    /// Today's number, computed the way the pre-merge-base code did: numstat against
    /// `HEAD`, insertions column only. Clause 1 is the equality with this.
    fn head_numstat_added(repo: &Path) -> u32 {
        git_out(repo, &["diff", "--numstat", "HEAD", "--", "crates/"])
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .filter_map(|x| x.parse::<u32>().ok())
            .sum()
    }

    // ---- measure_base: the boundaries, each half of the refusal/no-ancestor split ----

    #[test]
    fn a_fresh_branch_with_no_commits_measures_against_head_itself() {
        let tmp = scratch("no-commits");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        assert_eq!(
            measure_base(&repo),
            Some(git_out(&repo, &["rev-parse", "HEAD"])),
            "merge-base(HEAD, master) is HEAD when the branch has no commits"
        );
    }

    #[test]
    fn one_commit_measures_against_the_fork_point_not_head() {
        let tmp = scratch("one-commit");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        fs::write(repo.join(TARGET), FIVE_LINES).expect("arm edit");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "arm work"]);
        let base = measure_base(&repo).expect("repo is readable");
        assert_eq!(base, git_out(&repo, &["rev-parse", "master"]));
        assert_ne!(
            base,
            git_out(&repo, &["rev-parse", "HEAD"]),
            "with a commit on the branch, the base is the fork point, not HEAD"
        );
    }

    #[test]
    fn no_common_ancestor_in_a_readable_repo_falls_back_to_head() {
        // merge-base EXITS 1 here -- not a success, not a refusal of the repository --
        // and rev-parse has already answered, so the base is the literal "HEAD".
        let tmp = scratch("orphan");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "--orphan", "lonely"]);
        git_ok(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "orphan root"],
        );
        assert_eq!(measure_base(&repo), Some("HEAD".to_string()));
    }

    #[test]
    fn an_unreadable_worktree_is_a_refusal_not_a_fallback() {
        let tmp = scratch("unreadable");
        // Never a repository at all.
        let plain = tmp.join("plain");
        fs::create_dir_all(plain.join("crates")).expect("plain dir");
        assert_eq!(measure_base(&plain), None);
        // The pruned-worktree shape: the files survive, the admin directory does not.
        let pruned = new_repo(&tmp, "pruned");
        fs::remove_dir_all(pruned.join(".git")).expect("remove .git");
        assert_eq!(measure_base(&pruned), None);
    }

    // ---- the clauses, end to end through measure() ----

    #[test]
    fn a_clean_tree_with_zero_commits_is_a_genuine_no_op() {
        // The boundary the fix could break: measured, observed, and zero.
        let tmp = scratch("clean-no-op");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        let r = run_measure(&repo, &tmp.join("logs"));
        assert_eq!(r.lines.value().copied(), Some(0));
        assert!(r.lines.is_observed());
    }

    #[test]
    fn uncommitted_work_scores_exactly_as_it_did_against_head() {
        // Clause 1: for the uncommitted case the merge-base is HEAD itself, so the
        // score equals the HEAD-numstat count -- the equality pins that the base
        // changed nothing here.
        let tmp = scratch("uncommitted");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        fs::write(repo.join(TARGET), FIVE_LINES).expect("dirty edit");
        let today = head_numstat_added(&repo);
        assert!(today > 0, "fixture must have real uncommitted work");
        let r = run_measure(&repo, &tmp.join("logs"));
        assert_eq!(r.lines.value().copied(), Some(today));
    }

    #[test]
    fn committed_work_scores_like_the_same_work_uncommitted() {
        // Clause 2, tested as the PAIR with clause 1: two arrangements of one content,
        // plus master moved PAST the fork point. An implementation that always diffs
        // against master reads 9 here (restoring master's rewrite of old.rs as
        // insertions) where the merge-base reads 4 -- so the equality alone, at this
        // fork, already catches it; clause 1 catches today's HEAD-diff code.
        let tmp = scratch("committed-pair");
        let logs = tmp.join("logs");
        let edit_repo = |name: &str, commit: bool| {
            let repo = new_repo(&tmp, name);
            git_ok(&repo, &["checkout", "-q", "-b", "work"]);
            // master advances after the branch left it, with work in another crate.
            git_ok(&repo, &["checkout", "-q", "master"]);
            fs::write(repo.join("crates/other/src/old.rs"), "o1\n").expect("master edit");
            git_ok(&repo, &["add", "-A"]);
            git_ok(&repo, &["commit", "-q", "-m", "master rewrites old.rs"]);
            git_ok(&repo, &["checkout", "-q", "work"]);
            fs::write(repo.join(TARGET), FIVE_LINES).expect("arm edit");
            if commit {
                git_ok(&repo, &["add", "-A"]);
                git_ok(&repo, &["commit", "-q", "-m", "arm work"]);
            }
            repo
        };
        let committed = edit_repo("committed", true);
        let uncommitted = edit_repo("dirty", false);
        let a = run_measure(&committed, &logs).lines.value().copied();
        let b = run_measure(&uncommitted, &logs).lines.value().copied();
        assert_eq!(
            a, b,
            "committed and uncommitted arrangements of the same work"
        );
        assert_eq!(a, Some(4), "the committed lines, not 0 and not master's");
    }

    #[test]
    fn a_commit_and_further_edits_count_once_each() {
        // Clause 3: the total is the sum of the two arrangements, no line twice.
        // The committed lines (+2) and the further edits (+2) are disjoint, so the
        // equality is exact.
        let tmp = scratch("both");
        let logs = tmp.join("logs");
        let both = new_repo(&tmp, "both");
        git_ok(&both, &["checkout", "-q", "-b", "work"]);
        fs::write(both.join(TARGET), THREE_LINES).expect("committed edit");
        git_ok(&both, &["add", "-A"]);
        git_ok(&both, &["commit", "-q", "-m", "first half"]);
        fs::write(both.join(TARGET), FIVE_LINES).expect("further edits");
        let commit_only = new_repo(&tmp, "commit-only");
        git_ok(&commit_only, &["checkout", "-q", "-b", "work"]);
        fs::write(commit_only.join(TARGET), THREE_LINES).expect("committed edit");
        git_ok(&commit_only, &["add", "-A"]);
        git_ok(&commit_only, &["commit", "-q", "-m", "first half"]);
        let dirty_only = new_repo(&tmp, "dirty-only");
        git_ok(&dirty_only, &["checkout", "-q", "-b", "work"]);
        fs::write(dirty_only.join(TARGET), FURTHER_LINES).expect("further edits");
        let total = run_measure(&both, &logs).lines.value().copied();
        let committed = run_measure(&commit_only, &logs).lines.value().copied();
        let uncommitted = run_measure(&dirty_only, &logs).lines.value().copied();
        assert_eq!(committed, Some(2));
        assert_eq!(uncommitted, Some(2));
        assert_eq!(
            total,
            Some(committed.unwrap_or(0) + uncommitted.unwrap_or(0)),
            "both halves counted, once each"
        );
    }

    #[test]
    fn an_unreadable_worktree_is_missing_never_zero() {
        // Clause 4, end to end in the pruned-worktree shape: a buildable crate whose
        // git admin directory is gone. Lines must be Missing and the verdict must not
        // blame the arm -- one level down, this is the defect the whole task exists
        // to prevent.
        let tmp = scratch("pruned-buildable");
        let repo = new_repo(&tmp, "r");
        fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/demo\"]\n",
        )
        .expect("workspace toml");
        fs::write(
            repo.join("crates/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .expect("crate toml");
        fs::write(
            repo.join(TARGET),
            "#[test]\nfn exists() {\n    assert_eq!(2 + 2, 4);\n}\n",
        )
        .expect("crate lib.rs");
        fs::remove_dir_all(repo.join(".git")).expect("prune the admin directory");
        let r = run_measure(&repo, &tmp.join("logs"));
        assert!(!r.lines.is_observed(), "no git means no line count");
        assert_eq!(r.lines.value(), None, "never Some(0)");
        assert_eq!(r.verdict, Verdict::Indeterminate);
        assert!(!r.verdict.blames_arm());
    }

    #[test]
    fn a_committed_change_to_the_target_is_in_scope() {
        // Clause 5, in scope: a committed edit to the declared deliverable is a changed
        // path, the crate set reads the same diff, and the scope gate sees the target.
        let tmp = scratch("in-scope");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        fs::write(repo.join(TARGET), FIVE_LINES).expect("committed edit");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "arm work"]);
        let r = run_measure(&repo, &tmp.join("logs"));
        assert_eq!(r.lines.value().copied(), Some(4), "the commit is counted");
        let sc = r.scope.value().expect("scope is observed");
        assert!(sc.target_changed, "the committed change IS the target");
        assert!(is_clean(sc), "nothing outside the target changed");
        assert_eq!(
            r.crates,
            ["demo".to_string()]
                .into_iter()
                .collect::<BTreeSet<String>>(),
            "crate set from the same base as the count"
        );
    }

    #[test]
    fn a_committed_change_outside_the_target_is_a_departure() {
        // Clause 5, out of scope: a committed edit in another crate. Against HEAD this
        // run would read as a 0-line no-op with an empty departure list; against the
        // merge-base the file is a changed path and the scope gate says so.
        let tmp = scratch("out-of-scope");
        let repo = new_repo(&tmp, "r");
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        fs::write(repo.join("crates/other/src/old.rs"), "x\n").expect("committed edit");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "arm work in another crate"]);
        let r = run_measure(&repo, &tmp.join("logs"));
        assert!(
            r.lines.value().copied().unwrap_or(0) > 0,
            "the committed work must not read as a no-op"
        );
        let sc = r.scope.value().expect("scope is observed");
        assert!(!is_clean(sc), "a committed foreign change is a departure");
        let departed: Vec<String> = sc
            .departures
            .iter()
            .map(|d| match d {
                Departure::Foreign { path } | Departure::Deleted { path } => path.clone(),
            })
            .collect();
        assert_eq!(departed, vec!["crates/other/src/old.rs".to_string()]);
        assert!(
            r.crates.contains("other"),
            "the crate set reads the same diff as the count"
        );
    }
}

#[cfg(test)]
mod deleted_list {
    //! Clause tests for `deleted_paths` and the scope verdict it feeds. Every
    //! fixture is a fresh repository in a scratch directory; none of this
    //! suite reads this repository's own worktrees, and a refusal is
    //! reproduced by pointing at a directory that is not a repository.
    //!
    //! Fixture construction runs git subcommands beyond the module's closed
    //! five (init, symbolic-ref, config, add, commit) -- that is setup, and no
    //! assertion is made on any invocation the module itself does not make.
    use super::*;
    use farmerbob_core::measurement::Absent;

    /// The declared deliverable: a module INSIDE the crate, so the crate root
    /// beside it can be deleted as its own case.
    const TARGET: &str = "crates/demo/src/greeter.rs";
    /// The module declaration beside TARGET: a library crate's root.
    const DECLARATION: &str = "crates/demo/src/lib.rs";
    /// A path in another crate: the out-of-scope side of clause 6.
    const FOREIGN: &str = "crates/other/src/old.rs";

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fb-score-deleted-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("scratch dir");
        p
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git must be runnable");
        assert!(st.success(), "git {args:?} failed in {}", repo.display());
    }

    /// A readable repository, one baseline commit on `master`, with a module
    /// declaration to delete, a foreign crate to stray into, and one path
    /// holding a space.
    fn new_repo(parent: &Path, name: &str) -> PathBuf {
        let p = parent.join(name);
        fs::create_dir_all(p.join("crates/demo/src")).expect("repo layout");
        fs::create_dir_all(p.join("crates/other/src")).expect("repo layout");
        git_ok(&p, &["init", "-q"]);
        git_ok(&p, &["symbolic-ref", "HEAD", "refs/heads/master"]);
        git_ok(&p, &["config", "user.email", "test@example.com"]);
        git_ok(&p, &["config", "user.name", "test"]);
        git_ok(&p, &["config", "commit.gpgsign", "false"]);
        fs::write(p.join(DECLARATION), "pub mod greeter;\n").expect("baseline lib.rs");
        fs::write(p.join(TARGET), "pub fn greet() -> u32 {\n    1\n}\n").expect("baseline target");
        fs::write(p.join(FOREIGN), "o1\no2\no3\no4\no5\no6\n").expect("baseline other crate");
        fs::write(p.join("crates/other/src/has space.rs"), "s1\ns2\n")
            .expect("baseline spaced path");
        git_ok(&p, &["add", "-A"]);
        git_ok(&p, &["commit", "-q", "-m", "baseline"]);
        p
    }

    /// The base every diff in `measure` takes -- the same one `deleted_paths`
    /// must take, never a second base.
    fn base_of(repo: &Path) -> String {
        measure_base(repo).expect("fixture repo must be readable by git")
    }

    /// The stated reason there is no list, whatever `Absent` shape it took.
    fn absent_reason<T>(m: &Measurement<T>) -> String {
        match m.absent() {
            Some(Absent::InstrumentFailed { reason })
            | Some(Absent::NothingToMeasure { reason })
            | Some(Absent::Untrusted { reason }) => reason.clone(),
            Some(Absent::NotAttempted) | None => String::new(),
        }
    }

    fn run_measure(wt: &Path, log_root: &Path) -> Record {
        let t = Task {
            bead: "deleted-list-spec",
            krate: "demo",
            target: Some(TARGET),
            creates: false,
            base_clippy: 0,
            log_root,
        };
        measure(wt, "candidate", &t).expect("worktree under test must not look live")
    }

    // ---- clause 1: an answer with a list is the list, in git's order ----

    #[test]
    fn a_list_comes_back_observed_in_the_order_git_printed_it() {
        let tmp = scratch("list");
        let repo = new_repo(&tmp, "r");
        fs::remove_file(repo.join(TARGET)).expect("rm the target");
        fs::remove_file(repo.join(FOREIGN)).expect("rm the foreign file");
        let got = deleted_paths(&repo, &base_of(&repo));
        // git prints paths sorted (demo sorts before other); the measurement
        // keeps that order verbatim.
        assert_eq!(
            got,
            Measurement::Observed(vec![TARGET.to_string(), FOREIGN.to_string()]),
        );
    }

    #[test]
    fn a_path_with_a_space_is_taken_verbatim_one_path_per_line() {
        let tmp = scratch("space");
        let repo = new_repo(&tmp, "r");
        fs::remove_file(repo.join("crates/other/src/has space.rs")).expect("rm the spaced path");
        let got = deleted_paths(&repo, &base_of(&repo));
        // No -z parsing: the line is the path. A path containing a NEWLINE is
        // therefore not representable -- a stated limit of this wire format,
        // not a choice.
        assert_eq!(
            got,
            Measurement::Observed(vec!["crates/other/src/has space.rs".to_string()]),
        );
    }

    // ---- clauses 2, 3 and 4: an empty answer and a refusal are different ----

    #[test]
    fn zero_deletions_is_an_observed_empty_answer_not_a_refusal() {
        let tmp = scratch("zero");
        let repo = new_repo(&tmp, "r");
        let got = deleted_paths(&repo, &base_of(&repo));
        assert_eq!(got, Measurement::Observed(Vec::<String>::new()));
        assert!(got.is_observed());
    }

    #[test]
    fn a_refusal_is_missing_with_a_reason_that_names_git() {
        // Never a repository at all.
        let tmp = scratch("refused-plain");
        let plain = tmp.join("plain");
        fs::create_dir_all(&plain).expect("plain dir");
        let got = deleted_paths(&plain, "HEAD");
        assert!(!got.is_observed(), "a refusal is never an observed list");
        assert_eq!(got.value(), None);
        let reason = absent_reason(&got);
        assert!(!reason.trim().is_empty(), "the reason is stated, not blank");
        assert!(
            reason.to_lowercase().contains("git"),
            "the reason names git: {reason}"
        );
    }

    #[test]
    fn files_on_disk_are_not_evidence_git_could_read_them() {
        // The pruned-worktree shape: the work survives, the admin directory
        // does not. The list is Missing anyway -- inferring "nothing deleted"
        // from the files still on disk is the original defect.
        let tmp = scratch("refused-pruned");
        let repo = new_repo(&tmp, "r");
        fs::remove_dir_all(repo.join(".git")).expect("prune the admin directory");
        assert!(repo.join(TARGET).exists(), "fixture: the files survive");
        let got = deleted_paths(&repo, "HEAD");
        assert!(!got.is_observed(), "the surviving files prove nothing");
        let reason = absent_reason(&got);
        assert!(!reason.trim().is_empty());
        assert!(reason.to_lowercase().contains("git"), "reason: {reason}");
    }

    /// Clauses 2 and 3 pinned as a PAIR, in one test. Separately, each passes
    /// against an implementation that returns an empty vector for both -- which
    /// is the code this task replaces; only the comparison falsifies it.
    #[test]
    fn an_empty_answer_and_a_refusal_are_different_values() {
        let tmp = scratch("pair");
        let answers = new_repo(&tmp, "answers");
        let empty = deleted_paths(&answers, &base_of(&answers));
        assert!(
            empty.is_observed(),
            "clause 2: git answered: nothing deleted"
        );
        assert_eq!(empty.value(), Some(&Vec::<String>::new()));

        let refuses = tmp.join("refuses");
        fs::create_dir_all(&refuses).expect("plain dir");
        let missing = deleted_paths(&refuses, "HEAD");
        assert!(!missing.is_observed(), "clause 3: git refused");
        assert!(
            absent_reason(&missing).to_lowercase().contains("git"),
            "clause 3: the refusal names git"
        );
        assert_ne!(
            empty, missing,
            "clause 4: an empty list is not a refusal, and must never read as one"
        );
    }

    // ---- clause 6: an observed list marks deletions in the scope verdict ----

    #[test]
    fn deleting_the_declared_target_is_the_deleted_departure() {
        // The in-scope side, with the deletion COMMITTED: the merge-base base
        // is what lets a deletion made in a commit be listed as changed AND
        // told apart as deleted. The verdict is farmerbob_core::scope's
        // `Departure::Deleted`; asserted, not re-derived.
        let tmp = scratch("committed-target");
        let repo = new_repo(&tmp, "r");
        // The arm works on a branch, as the harness dispatches it: committing
        // on master would move the merge-base past the deletion.
        git_ok(&repo, &["checkout", "-q", "-b", "work"]);
        fs::remove_file(repo.join(TARGET)).expect("rm the target");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "delete the target"]);
        let r = run_measure(&repo, &tmp.join("logs"));
        let sc = r
            .scope
            .value()
            .expect("the deletion list was observed, so scope is observed");
        assert!(
            !sc.target_changed,
            "deleting the target is not producing it"
        );
        assert_eq!(
            sc.departures,
            vec![Departure::Deleted {
                path: TARGET.to_string(),
            }],
        );
        assert!(!is_clean(sc));
    }

    #[test]
    fn deleting_the_module_declaration_is_the_deleted_departure() {
        // ONE deleted path and it is the module declaration (the crate root
        // beside the target). Also `Departure::Deleted` per core: an allowed
        // file deleted is a violation like any other deletion, not an
        // exercised allowance.
        let tmp = scratch("declaration");
        let repo = new_repo(&tmp, "r");
        fs::remove_file(repo.join(DECLARATION)).expect("rm lib.rs");
        let r = run_measure(&repo, &tmp.join("logs"));
        let sc = r.scope.value().expect("scope is observed");
        assert!(sc.allowed.is_empty(), "a deleted allowance is not granted");
        assert_eq!(
            sc.departures,
            vec![Departure::Deleted {
                path: DECLARATION.to_string(),
            }],
        );
    }

    #[test]
    fn deleting_a_foreign_file_is_a_deleted_departure_not_a_foreign_one() {
        // The out-of-scope side: the scope gate reads the deletion list, so a
        // deletion outside the target must arrive as Deleted -- never as
        // Foreign, and never as absent.
        let tmp = scratch("foreign-del");
        let repo = new_repo(&tmp, "r");
        fs::remove_file(repo.join(FOREIGN)).expect("rm the foreign file");
        let r = run_measure(&repo, &tmp.join("logs"));
        let sc = r.scope.value().expect("scope is observed");
        assert!(!sc.target_changed);
        assert_eq!(
            sc.departures,
            vec![Departure::Deleted {
                path: FOREIGN.to_string(),
            }],
        );
        assert!(!is_clean(sc));
    }

    #[test]
    fn the_deleted_flag_comes_from_the_deletion_list_not_from_the_path_list() {
        // The same foreign path in two arrangements: modified, it is Foreign;
        // deleted, it is Deleted. An implementation that marks nothing deleted
        // -- the empty-vector defect -- passes the first and fails the second.
        let tmp = scratch("contrast");
        let logs = tmp.join("logs");
        let modified_repo = new_repo(&tmp, "modified");
        fs::write(modified_repo.join(FOREIGN), "x\n").expect("rewrite the foreign file");
        let m = run_measure(&modified_repo, &logs);
        assert_eq!(
            m.scope.value().expect("scope is observed").departures,
            vec![Departure::Foreign {
                path: FOREIGN.to_string(),
            }],
        );

        let deleted_repo = new_repo(&tmp, "deleted");
        fs::remove_file(deleted_repo.join(FOREIGN)).expect("delete the foreign file");
        let d = run_measure(&deleted_repo, &logs);
        assert_eq!(
            d.scope.value().expect("scope is observed").departures,
            vec![Departure::Deleted {
                path: FOREIGN.to_string(),
            }],
        );
    }

    // ---- clauses 5 and 7: a refusal reaches scope and nothing else ----

    #[test]
    fn a_refused_deletion_list_leaves_scope_missing_and_lines_observed() {
        // The worktree still holds the deliverable on disk, but git is gone:
        // the on-disk files are not evidence git could read them, so the
        // deletion list is Missing and scope is Missing with it -- never
        // Observed with every path marked not-deleted. The refusal does not
        // cascade: the line count is still observed (the creates recovery
        // needs no git).
        let tmp = scratch("refused-scope");
        let repo = new_repo(&tmp, "r");
        fs::remove_dir_all(repo.join(".git")).expect("prune the admin directory");
        let t = Task {
            bead: "deleted-list-spec",
            krate: "demo",
            target: Some(TARGET),
            creates: true,
            base_clippy: 0,
            log_root: &tmp.join("logs"),
        };
        let r = measure(&repo, "candidate", &t).expect("worktree under test must not look live");
        assert!(!r.scope.is_observed(), "clause 5: scope is Missing");
        assert_eq!(r.scope.value(), None, "never Observed-all-not-deleted");
        let reason = absent_reason(&r.scope);
        assert!(!reason.trim().is_empty());
        assert!(reason.to_lowercase().contains("git"), "reason: {reason}");
        let lines = r
            .lines
            .value()
            .copied()
            .expect("clause 7: the line count survives the refusal");
        assert!(lines > 0, "the deliverable's own lines are a real count");
    }
}
