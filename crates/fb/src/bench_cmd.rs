//! The impure end of the benchmark path: spawn the scripts, drive the loop.
//!
//! Four pure modules describe a benchmark run and none of them can run one.
//! [`trial_plan::next`] decides whether another trial is worth attempting,
//! [`bench_read`] turns one run's captured stdout into a verification or a
//! measurement, and `task_contract::score` consumes what a finished run
//! produced. This module is the connector: it spawns `verify.sh` once, then
//! `bench.sh` per trial, captures stdout and exit status, and lets the pure
//! modules make every decision. Nothing here parses JSON or counts readable
//! runs on its own — a decision this file could delegate to `trial_plan` or
//! `bench_read` and makes itself is the defect this task exists to avoid.

#![allow(dead_code)] // run_bench has no subcommand yet; that wiring is a separate task.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use farmerbob_core::bench_read::{self, Run};
use farmerbob_core::task_contract::{TaskManifest, VerifyOutput};
use farmerbob_core::trial_plan::{self, Next, Progress};

/// Where a task's scripts live and how hard to try.
#[derive(Debug)]
pub struct BenchPlan {
    /// Directory containing verify.sh and bench.sh.
    pub dir: PathBuf,
    /// Seconds before one trial is killed. Zero means no limit.
    pub timeout_s: u64,
    /// Total trials that may be attempted.
    pub max_attempts: u32,
    /// Unreadable runs tolerated before the instrument is judged unreliable.
    pub max_bad: u32,
}

/// What a whole benchmark run produced.
#[derive(Debug)]
pub enum Outcome {
    /// Verification and enough trials. Carries what the scorer needs.
    Measured {
        /// What verify.sh said.
        verify: VerifyOutput,
        /// Good measurements, in the order the trials produced them.
        samples: Vec<f64>,
    },
    /// Verification ran and said the implementation is INCORRECT. This is a
    /// result about the candidate, and no trials were attempted.
    Incorrect(VerifyOutput),
    /// The instrument failed: verification could not be read, or too many
    /// trials were unreadable, or attempts ran out. Carries a reason.
    Instrument(String),
}

/// One spawned script's captured stdout and exit status.
struct Captured {
    stdout: String,
    exited_zero: bool,
}

/// Run one script under `sh` inside `dir`, capturing stdout.
///
/// The script is spawned via `sh` rather than executed directly, so a task's
/// scripts need not carry the executable bit, and its working directory is
/// `dir`, so a script's relative side-effect paths land beside the scripts.
///
/// With `timeout_s` above zero, a run that exceeds the budget is killed and
/// reported as failed; its partial output is discarded, because a killed run
/// is unreadable no matter what it printed. Zero means no limit.
fn run_captured(dir: &Path, script: &str, timeout_s: u64) -> Result<Captured, String> {
    let script_path = dir.join(script);
    if !script_path.is_file() {
        return Err(format!("missing script {}", script_path.display()));
    }
    let mut child = Command::new("sh")
        .arg(&script_path)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not spawn {}: {}", script_path.display(), error))?;

    // Drain stdout on a thread so a chatty script cannot fill the pipe and
    // block the polling loop below.
    let pipe = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut collected = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut collected);
        }
        String::from_utf8_lossy(&collected).into_owned()
    });

    let deadline = if timeout_s == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(timeout_s))
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = reader.join().unwrap_or_default();
                return Ok(Captured {
                    stdout,
                    exited_zero: status.success(),
                });
            }
            Ok(None) => {
                if let Some(budget) = deadline
                    && Instant::now() >= budget
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    // The reader thread is not joined here: a killed
                    // script's grandchildren can hold the pipe open for a
                    // while, and the run is unreadable regardless.
                    return Ok(Captured {
                        stdout: String::new(),
                        exited_zero: false,
                    });
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                return Err(format!(
                    "could not track {}: {}",
                    script_path.display(),
                    error
                ));
            }
        }
    }
}

/// Run verify.sh once and read what it printed.
fn read_verification(p: &BenchPlan) -> Result<VerifyOutput, String> {
    let captured = match run_captured(&p.dir, "verify.sh", p.timeout_s) {
        Ok(captured) => captured,
        Err(reason) => return Err(format!("verification: {}", reason)),
    };
    let run = Run {
        stdout: captured.stdout,
        exited_zero: captured.exited_zero,
    };
    match bench_read::verify(&run) {
        Ok(verify) => Ok(verify),
        Err(error) => Err(format!(
            "verification: could not read verify.sh output: {:?}",
            error
        )),
    }
}

/// Run verification, then trials, driving `trial_plan`.
///
/// Verification runs before any trial and at most once. If `trial_plan`
/// abandons the not-yet-started run — a zero attempt budget — no script runs
/// at all. A verification that ran and reported incorrect is
/// [`Outcome::Incorrect`], a result about the candidate; every other early
/// stop is [`Outcome::Instrument`], a result about the harness.
pub fn run_bench(m: &TaskManifest, p: &BenchPlan) -> Outcome {
    // The plan is consulted before anything spawns: a zero attempt budget
    // abandons the empty run, so neither script executes.
    let start = Progress {
        attempted: 0,
        good: 0,
        bad: 0,
    };
    if matches!(
        trial_plan::next(m, &start, p.max_attempts, p.max_bad),
        Next::Abandon(_)
    ) {
        return Outcome::Instrument(String::from(
            "attempts: trial_plan abandoned the run before any script ran; max_attempts is 0",
        ));
    }

    let verify = match read_verification(p) {
        Ok(verify) => verify,
        Err(reason) => return Outcome::Instrument(reason),
    };
    if !verify.correct {
        return Outcome::Incorrect(verify);
    }

    let mut progress = Progress {
        attempted: 0,
        good: 0,
        bad: 0,
    };
    let mut samples = Vec::new();
    loop {
        match trial_plan::next(m, &progress, p.max_attempts, p.max_bad) {
            Next::Run => {
                let captured = match run_captured(&p.dir, "bench.sh", p.timeout_s) {
                    Ok(captured) => captured,
                    Err(reason) => return Outcome::Instrument(format!("trials: {}", reason)),
                };
                progress.attempted += 1;
                let run = Run {
                    stdout: captured.stdout,
                    exited_zero: captured.exited_zero,
                };
                match bench_read::bench(&run) {
                    Ok(output) => {
                        progress.good += 1;
                        samples.push(output.ms);
                    }
                    Err(_) => progress.bad += 1,
                }
            }
            Next::Enough(_) => return Outcome::Measured { verify, samples },
            Next::Abandon(_) => {
                return Outcome::Instrument(format!(
                    "attempts: trial_plan abandoned the run after {} attempted, {} readable, {} unreadable",
                    progress.attempted, progress.good, progress.bad
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::task_contract::{TaskName, Verification};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const OK_VERIFY: &str = "echo '{\"correct\":true,\"detail\":\"ok\"}'\n";

    /// Every test spawns real scripts, so each one gets a fresh directory.
    fn scratch(tag: &str) -> PathBuf {
        static USED: AtomicUsize = AtomicUsize::new(0);
        let slot = USED.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "fb-bench-cmd-{}-{}-{}",
            std::process::id(),
            tag,
            slot
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    fn write_script(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).expect("test script");
    }

    fn manifest(min_trials: u32) -> TaskManifest {
        TaskManifest {
            name: TaskName(String::from("bench")),
            description: String::new(),
            verification: Verification::Benchmark,
            timeout_s: 0,
            exclusive: Vec::new(),
            min_trials,
        }
    }

    fn plan(dir: &Path, timeout_s: u64, max_attempts: u32, max_bad: u32) -> BenchPlan {
        BenchPlan {
            dir: dir.to_path_buf(),
            timeout_s,
            max_attempts,
            max_bad,
        }
    }

    /// A bench.sh that appends one line to a log per execution and prints
    /// that line count as its measurement.
    fn counting_bench(dir: &Path) {
        let log = dir.join("runs.log");
        write_script(
            dir,
            "bench.sh",
            &format!(
                "echo x >> {}\nprintf '{{\"ms\":%s}}\\n' \"$(wc -l < {})\"\n",
                log.display(),
                log.display()
            ),
        );
    }

    /// Clauses 1 and 5: enough good trials yield `Measured` with every
    /// sample, the loop stops at exactly `min_trials` executions, and the
    /// samples keep production order.
    #[test]
    fn stops_at_enough_and_keeps_sample_order() {
        let dir = scratch("enough");
        write_script(&dir, "verify.sh", OK_VERIFY);
        counting_bench(&dir);
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 10, 2));
        match outcome {
            Outcome::Measured { verify, samples } => {
                assert!(verify.correct);
                assert_eq!(verify.detail, "ok");
                assert_eq!(samples, vec![1.0, 2.0, 3.0]);
            }
            other => panic!("expected Measured, got {:?}", other),
        }
        let log = fs::read_to_string(dir.join("runs.log")).expect("runs.log");
        assert_eq!(log.lines().count(), 3);
    }

    /// Clause 2: an incorrect verification is a result about the candidate,
    /// and bench.sh is never executed — pinned with a marker the script
    /// would create.
    #[test]
    fn incorrect_verification_never_runs_bench() {
        let dir = scratch("incorrect");
        write_script(
            &dir,
            "verify.sh",
            "echo '{\"correct\":false,\"detail\":\"mismatch\"}'\n",
        );
        write_script(&dir, "bench.sh", "touch marker\n");
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 10, 2));
        match outcome {
            Outcome::Incorrect(verify) => {
                assert!(!verify.correct);
                assert_eq!(verify.detail, "mismatch");
            }
            other => panic!("expected Incorrect, got {:?}", other),
        }
        assert!(!dir.join("marker").exists());
    }

    /// Clause 3, pinned against clause 2: a verification that crashed is a
    /// result about the harness, never `Incorrect`, and no trial follows.
    #[test]
    fn crashed_verification_is_instrument_not_incorrect() {
        let dir = scratch("crashed-verify");
        write_script(&dir, "verify.sh", "echo boom >&2\nexit 3\n");
        write_script(&dir, "bench.sh", "touch marker\n");
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 10, 2));
        assert!(matches!(outcome, Outcome::Instrument(_)));
        assert!(!dir.join("marker").exists());
    }

    /// A verification that exits zero but prints nothing readable is also an
    /// instrument failure, and only a correct verification leads to trials.
    #[test]
    fn unreadable_verification_is_instrument_and_runs_no_trials() {
        let dir = scratch("unreadable-verify");
        write_script(&dir, "verify.sh", "echo not-json\n");
        write_script(&dir, "bench.sh", "touch marker\n");
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 10, 2));
        assert!(matches!(outcome, Outcome::Instrument(_)));
        assert!(!dir.join("marker").exists());
    }

    /// Clause 4: unreadable runs beyond max_bad abandon the run —
    /// trial_plan's precedence, not re-derived here.
    #[test]
    fn bad_runs_beyond_tolerance_abandon() {
        let dir = scratch("too-bad");
        write_script(&dir, "verify.sh", OK_VERIFY);
        write_script(&dir, "bench.sh", "exit 1\n");
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 10, 2));
        assert!(matches!(outcome, Outcome::Instrument(_)));
    }

    /// Clause 4 against clause 5 as trial_plan orders them: once Enough is
    /// reached the loop stops, even though later executions would have
    /// failed. The harness must not run past Enough to count extra failures.
    #[test]
    fn enough_stops_before_later_failures_could_occur() {
        let dir = scratch("good-then-bad");
        write_script(&dir, "verify.sh", OK_VERIFY);
        write_script(
            &dir,
            "bench.sh",
            "n=$(cat n 2>/dev/null || echo 0)\nn=$((n+1))\necho \"$n\" > n\nif [ \"$n\" -gt 2 ]; then exit 1; fi\necho '{\"ms\":5}'\n",
        );
        let outcome = run_bench(&manifest(2), &plan(&dir, 0, 10, 5));
        match outcome {
            Outcome::Measured { samples, .. } => assert_eq!(samples, vec![5.0, 5.0]),
            other => panic!("expected Measured, got {:?}", other),
        }
    }

    /// Clause 6: a trial that exceeds timeout_s is killed and counts as bad,
    /// never as a measurement. With zero tolerance, one killed trial
    /// abandons the run; a harness without a working timeout would instead
    /// measure the sleeping script and return `Measured`.
    #[test]
    fn slow_trial_is_killed_and_counts_bad() {
        let dir = scratch("timeout");
        write_script(&dir, "verify.sh", OK_VERIFY);
        write_script(&dir, "bench.sh", "sleep 5\necho '{\"ms\":5}'\n");
        let outcome = run_bench(&manifest(1), &plan(&dir, 1, 2, 0));
        assert!(matches!(outcome, Outcome::Instrument(_)));
    }

    /// Clause 7: timeout_s zero imposes no limit, so a short script
    /// completes and measures.
    #[test]
    fn zero_timeout_uses_no_limit() {
        let dir = scratch("no-limit");
        write_script(&dir, "verify.sh", OK_VERIFY);
        write_script(&dir, "bench.sh", "echo '{\"ms\":5}'\n");
        let outcome = run_bench(&manifest(1), &plan(&dir, 0, 5, 2));
        match outcome {
            Outcome::Measured { samples, .. } => assert_eq!(samples, vec![5.0]),
            other => panic!("expected Measured, got {:?}", other),
        }
    }

    /// Boundary: a zero attempt budget abandons before anything runs —
    /// neither script executes, pinned with markers both would create.
    #[test]
    fn zero_attempt_budget_runs_nothing() {
        let dir = scratch("zero-attempts");
        write_script(&dir, "verify.sh", "touch verify-ran\n");
        write_script(&dir, "bench.sh", "touch bench-ran\n");
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 0, 2));
        assert!(matches!(outcome, Outcome::Instrument(_)));
        assert!(!dir.join("verify-ran").exists());
        assert!(!dir.join("bench-ran").exists());
    }

    /// Boundary: min_trials zero asks for zero trials. The run measures with
    /// an empty sample list and bench.sh is never executed.
    #[test]
    fn zero_minimum_measures_with_zero_samples() {
        let dir = scratch("zero-minimum");
        write_script(&dir, "verify.sh", OK_VERIFY);
        write_script(&dir, "bench.sh", "touch marker\n");
        let outcome = run_bench(&manifest(0), &plan(&dir, 0, 10, 2));
        match outcome {
            Outcome::Measured { samples, .. } => assert!(samples.is_empty()),
            other => panic!("expected Measured, got {:?}", other),
        }
        assert!(!dir.join("marker").exists());
    }

    /// Boundary: a directory without verify.sh is an instrument failure
    /// naming what was missing.
    #[test]
    fn missing_verify_script_is_an_instrument_failure() {
        let dir = scratch("no-verify");
        write_script(&dir, "bench.sh", "echo '{\"ms\":5}'\n");
        let outcome = run_bench(&manifest(3), &plan(&dir, 0, 10, 2));
        match outcome {
            Outcome::Instrument(reason) => assert!(reason.contains("verify.sh")),
            other => panic!("expected Instrument, got {:?}", other),
        }
    }

    /// Boundary: a missing bench.sh after a correct verification is an
    /// instrument failure — distinct from the min_trials zero case, where
    /// bench.sh is absent-but-also-unneeded. Here it was needed.
    #[test]
    fn missing_bench_script_is_an_instrument_failure() {
        let dir = scratch("no-bench");
        write_script(&dir, "verify.sh", OK_VERIFY);
        let outcome = run_bench(&manifest(1), &plan(&dir, 0, 10, 2));
        match outcome {
            Outcome::Instrument(reason) => assert!(reason.contains("bench.sh")),
            other => panic!("expected Instrument, got {:?}", other),
        }
    }
}
