//! Score one kernel task from its baseline and fresh samples.
//!
//! The GPU-side scripts (`kb_shim.py`, `kb_baseline.py`, `kb_report.py`)
//! collect measurements; the math lives here, in one implementation over
//! [`farmerbob_core::kernel_score`] and
//! [`farmerbob_core::env_fingerprint`]:
//!
//! - correctness first: a wrong kernel scores zero regardless of speed;
//! - staleness second: a baseline from another fingerprint is refused;
//! - then median and IQR over the fresh samples, unreliable when the IQR
//!   is wide relative to the median;
//! - speedup is baseline median over sample median; `fast` means correct,
//!   scored, and faster than `p`.
//!
//! [`farmerbob_core::kernel_score::fast_p`] aggregates these rows across
//! tasks; `kb_report.py` applies that definition verbatim.

use std::io::Write;
use std::path::Path;

use farmerbob_core::env_fingerprint::EnvFingerprint;
use farmerbob_core::kernel_score::{self, TaskOutcome};

/// One task's score, rendered as a single JSON object.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskScore {
    /// `scored` | `incorrect` | `unreliable` | `stale`.
    pub status: &'static str,
    /// Speedup over the baseline median. Present only when scored.
    pub speedup: Option<f64>,
    /// Median of the fresh samples. Present when samples existed.
    pub median_ms: Option<f64>,
    /// IQR of the fresh samples. Present when samples existed.
    pub iqr_ms: Option<f64>,
    /// Baseline median. Present when the baseline was readable.
    pub baseline_ms: Option<f64>,
    /// True when scored and faster than `p`.
    pub fast: bool,
    /// Human-readable detail, never empty.
    pub detail: String,
}

impl TaskScore {
    fn render(&self) -> String {
        serde_json::json!({
            "status": self.status,
            "speedup": self.speedup,
            "median_ms": self.median_ms,
            "iqr_ms": self.iqr_ms,
            "baseline_ms": self.baseline_ms,
            "fast": self.fast,
            "detail": self.detail,
        })
        .to_string()
    }
}

/// Score one task.
///
/// `baseline_json` is the parsed content of `ref/baseline.json` (must carry
/// `fingerprint` and `median_ms`). `current_fingerprint` is the environment
/// the samples were measured in. `samples` are fresh bench measurements in
/// ms. `correct` is the verification verdict. `p` is the fast threshold.
/// `min_samples` is the minimum fresh samples before a task may be scored.
/// `clock` is the clock verdict (`""` when clean): a non-empty value means
/// the candidate reached the instrument's clock (bead farmerbob-x81s.2),
/// and the measurement set is refused like a tampered run.
///
/// `io_bytes` enables the plausibility floor (bead farmerbob-x81s.6);
/// zero disables it.
///
/// `specialised` carries the held-out verdict (bead farmerbob-x81s.4).
///
/// `reference_call` carries the reference-call answer (bead
/// farmerbob-x81s.5).
///
/// A tampered or clock-tainted run is never scored: see
/// [`score_with_trust`].
#[allow(clippy::too_many_arguments)] // Eleven CLI flags in, one struct out would only rename the call sites.
pub fn score(
    baseline_json: &serde_json::Value,
    current_fingerprint: &str,
    samples: &[f64],
    correct: bool,
    p: f64,
    min_samples: usize,
    clock: &str,
    io_bytes: u64,
    specialised: bool,
    reference_call: bool,
) -> TaskScore {
    score_with_trust(
        baseline_json,
        current_fingerprint,
        samples,
        correct,
        p,
        min_samples,
        &[],
        clock,
        io_bytes,
        specialised,
        reference_call,
    )
}

/// Score one task with the trust boundary applied.
///
/// `tampered` carries the post-run re-hash findings (bead
/// farmerbob-x81s.1): a non-empty list means the agent rewrote a pinned
/// file -- the scorer, the reference, the manifest -- and the run is VOID.
/// It names the file and both hashes, and it gets NO SCORE: speedup None,
/// fast false. It must not be scored low and ranked -- a tampered run is
/// not a weak result, and reporting it as one would be this project's
/// signature defect (a failure state indistinguishable from a success
/// state) in the place it matters most.
///
/// `clock` carries the clock verdict (bead farmerbob-x81s.2): non-empty
/// means a trial caught the candidate rebinding a timing primitive or
/// reporting a measurement impossible against the harness wall clock. The
/// run is `clock` -- a named verdict distinct from slow and from
/// incorrect, with no speedup and fast false.
///
/// `io_bytes` carries the task's minimum memory traffic in bytes (inputs
/// plus output, as the bench shim reports it). Non-zero enables the
/// plausibility floor (bead farmerbob-x81s.6): a median below
/// bytes-over-peak-bandwidth is `unreliable`, naming the floor and the
/// figure. Zero disables the check.
///
/// `specialised` carries the held-out verdict (bead farmerbob-x81s.4):
/// true means the candidate passed the visible inputs and failed inputs
/// from a seed the agent never had. The run is `specialised` -- its own
/// verdict, never incorrect, with no speedup and fast false.
///
/// `reference_call` carries the reference-call answer (bead
/// farmerbob-x81s.5): true means the candidate executed the task's own
/// reference code during the run. The run is `reference` -- it measured
/// the answer key, not the arm -- with no speedup and fast false.
///
/// Tamper dominates clock dominates reference dominates specialised
/// dominates `incorrect`: provenance verdicts are never hidden behind a
/// content verdict.
#[allow(clippy::too_many_arguments)] // Eleven CLI flags in, one struct out would only rename the call sites.
pub fn score_with_trust(
    baseline_json: &serde_json::Value,
    current_fingerprint: &str,
    samples: &[f64],
    correct: bool,
    p: f64,
    min_samples: usize,
    tampered: &[farmerbob_core::kernel_trust::Tamper],
    clock: &str,
    io_bytes: u64,
    specialised: bool,
    reference_call: bool,
) -> TaskScore {
    if !tampered.is_empty() {
        let lines: Vec<String> = tampered
            .iter()
            .map(farmerbob_core::kernel_trust::void_line)
            .collect();
        return TaskScore {
            status: "void",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: baseline_json
                .get("median_ms")
                .and_then(serde_json::Value::as_f64),
            fast: false,
            detail: lines.join("; "),
        };
    }
    if !clock.is_empty() {
        return TaskScore {
            status: "clock",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: baseline_json
                .get("median_ms")
                .and_then(serde_json::Value::as_f64),
            fast: false,
            detail: clock.to_string(),
        };
    }
    if reference_call {
        return TaskScore {
            status: "reference",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: baseline_json
                .get("median_ms")
                .and_then(serde_json::Value::as_f64),
            fast: false,
            detail: "executed the task's own reference code during the run: measured the answer key, not the arm".to_string(),
        };
    }
    if specialised {
        return TaskScore {
            status: "specialised",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: baseline_json
                .get("median_ms")
                .and_then(serde_json::Value::as_f64),
            fast: false,
            detail: "passed the visible inputs and failed the held-out set: input specialisation, not ordinary incorrectness".to_string(),
        };
    }
    let baseline_fp = baseline_json
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let baseline_ms = baseline_json
        .get("median_ms")
        .and_then(serde_json::Value::as_f64);
    let (Ok(base_fp), Ok(cur_fp)) = (
        EnvFingerprint::parse(baseline_fp),
        EnvFingerprint::parse(current_fingerprint),
    ) else {
        return TaskScore {
            status: "stale",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms,
            fast: false,
            detail: "unparseable fingerprint; refusing to compare".to_string(),
        };
    };
    if !correct {
        return TaskScore {
            status: "incorrect",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms,
            fast: false,
            detail: "wrong kernel scores zero regardless of speed".to_string(),
        };
    }
    if let Err(reason) = cur_fp.check_against(&base_fp) {
        return TaskScore {
            status: "stale",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms,
            fast: false,
            detail: reason,
        };
    }
    let baseline_ms = match baseline_ms {
        Some(m) if m.is_finite() && m > 0.0 => m,
        _ => {
            return TaskScore {
                status: "unreliable",
                speedup: None,
                median_ms: None,
                iqr_ms: None,
                baseline_ms,
                fast: false,
                detail: "baseline median is not a positive measurement".to_string(),
            };
        }
    };
    if samples.iter().any(|s| !s.is_finite() || *s <= 0.0) {
        return TaskScore {
            status: "unreliable",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: Some(baseline_ms),
            fast: false,
            detail: "samples contain a non-positive or non-finite measurement".to_string(),
        };
    }
    if samples.len() < min_samples {
        return TaskScore {
            status: "unreliable",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: Some(baseline_ms),
            fast: false,
            detail: format!(
                "too few samples: {} measured, {min_samples} required",
                samples.len()
            ),
        };
    }
    let Some(summary) = kernel_score::summarize(samples) else {
        return TaskScore {
            status: "unreliable",
            speedup: None,
            median_ms: None,
            iqr_ms: None,
            baseline_ms: Some(baseline_ms),
            fast: false,
            detail: "no samples were measured".to_string(),
        };
    };
    if !kernel_score::reliable(&summary, 0.5) {
        return TaskScore {
            status: "unreliable",
            speedup: None,
            median_ms: Some(summary.median),
            iqr_ms: Some(summary.iqr),
            baseline_ms: Some(baseline_ms),
            fast: false,
            detail: format!(
                "IQR {} ms is wide relative to median {} ms",
                summary.iqr, summary.median
            ),
        };
    }
    // The plausibility floor (bead farmerbob-x81s.6): the same Unreliable
    // verdict as a wide IQR, on a different axis. Physics, not statistics:
    // no implementation moves io_bytes through the fingerprinted GPU's
    // peak bandwidth faster than this, so a median below it did not
    // happen, whatever the clock reported. The floor is a lower bound by
    // construction (real kernels move more than inputs-plus-output), so a
    // result above it is merely not-impossible.
    if io_bytes > 0
        && let Some(bandwidth) = cur_fp.peak_memory_bandwidth_gbps()
        && let Some(floor) = kernel_score::floor_ms(io_bytes, bandwidth)
        && summary.median < floor
    {
        return TaskScore {
            status: "unreliable",
            speedup: None,
            median_ms: Some(summary.median),
            iqr_ms: Some(summary.iqr),
            baseline_ms: Some(baseline_ms),
            fast: false,
            detail: format!(
                "measured {} ms below physics floor {:.3} ms ({} bytes at {} GB/s)",
                summary.median, floor, io_bytes, bandwidth
            ),
        };
    }
    let speedup = baseline_ms / summary.median;
    let fast = TaskOutcome {
        correct: true,
        speedup: Some(speedup),
    };
    let fast = kernel_score::fast_p(std::slice::from_ref(&fast), p) > 0.0;
    TaskScore {
        status: "scored",
        speedup: Some(speedup),
        median_ms: Some(summary.median),
        iqr_ms: Some(summary.iqr),
        baseline_ms: Some(baseline_ms),
        fast,
        detail: format!("speedup {speedup:.3} over baseline {baseline_ms} ms"),
    }
}

/// Read the baseline, score, render one JSON object. Returns the exit code:
/// 0 scored, 1 not scored (void/clock/specialised/incorrect/unreliable/stale), 4 unreadable.
///
/// `tampered` carries the trust re-hash findings: non-empty voids the run
/// (bead farmerbob-x81s.1), which is never a score. `clock` carries the
/// clock verdict: non-empty is the `clock` status (bead farmerbob-x81s.2),
/// named and distinct from slow and from incorrect. `io_bytes` enables the
/// plausibility floor (bead farmerbob-x81s.6); zero disables it.
/// `specialised` is the held-out verdict (bead farmerbob-x81s.4): its own
/// status, never incorrect. `reference_call` is the reference-call answer
/// (bead farmerbob-x81s.5): its own status, never incorrect, never slow.
#[allow(clippy::too_many_arguments)] // Eleven CLI flags in, one struct out would only rename the call sites.
pub fn run(
    baseline_path: &Path,
    current_fingerprint: &str,
    samples: &[f64],
    correct: bool,
    p: f64,
    min_samples: usize,
    tampered: &[farmerbob_core::kernel_trust::Tamper],
    clock: &str,
    io_bytes: u64,
    specialised: bool,
    reference_call: bool,
    out: &mut dyn Write,
) -> i32 {
    let content = match std::fs::read_to_string(baseline_path) {
        Ok(c) => c,
        Err(e) => {
            // A task recorded unmeasurable on this fingerprint is answered,
            // not missing (bead farmerbob-x81s.13): the verdict says so
            // instead of reading as a lost file. A stale record (other
            // fingerprint) is plain missing -- a new GPU may measure it.
            // Layout: <task>/ref/baseline.json, record beside it.
            let unmeasurable = baseline_path
                .parent()
                .map(|r| r.join("unmeasurable.json"))
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .filter(|v| {
                    v.get("fingerprint").and_then(serde_json::Value::as_str)
                        == Some(current_fingerprint)
                });
            let detail = match unmeasurable {
                Some(v) => {
                    let reason = v
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unmeasurable");
                    let why = v
                        .get("detail")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    format!("no baseline: recorded unmeasurable ({reason}): {why}")
                }
                None => format!("no baseline at {}: {e}", baseline_path.display()),
            };
            let _ = writeln!(
                out,
                "{}",
                TaskScore {
                    status: "stale",
                    speedup: None,
                    median_ms: None,
                    iqr_ms: None,
                    baseline_ms: None,
                    fast: false,
                    detail,
                }
                .render()
            );
            return 4;
        }
    };
    let baseline_json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(
                out,
                "{}",
                TaskScore {
                    status: "stale",
                    speedup: None,
                    median_ms: None,
                    iqr_ms: None,
                    baseline_ms: None,
                    fast: false,
                    detail: format!("baseline at {} is not JSON: {e}", baseline_path.display()),
                }
                .render()
            );
            return 4;
        }
    };
    // A requested floor the fingerprint cannot derive is not silently
    // skipped: the operator asked for physics and the GPU is unknown, so
    // say so where the operator looks. The score itself proceeds -- an
    // unknown GPU is a missing table row, not evidence against the run.
    if io_bytes > 0
        && EnvFingerprint::parse(current_fingerprint)
            .ok()
            .and_then(|fp| fp.peak_memory_bandwidth_gbps())
            .is_none()
    {
        eprintln!(
            "warning: no peak bandwidth for this GPU; scoring without the plausibility floor"
        );
    }
    let scored = if tampered.is_empty()
        && clock.is_empty()
        && io_bytes == 0
        && !specialised
        && !reference_call
    {
        score(
            &baseline_json,
            current_fingerprint,
            samples,
            correct,
            p,
            min_samples,
            clock,
            io_bytes,
            specialised,
            reference_call,
        )
    } else {
        score_with_trust(
            &baseline_json,
            current_fingerprint,
            samples,
            correct,
            p,
            min_samples,
            tampered,
            clock,
            io_bytes,
            specialised,
            reference_call,
        )
    };
    let code = match scored.status {
        "scored" => 0,
        _ => 1,
    };
    let _ = writeln!(out, "{}", scored.render());
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str =
        "torch=2.14.0+cu130;cuda=13.0;driver=610.57.04;gpu=RTX5090;arch=sm_120;python=3.14.7";

    fn baseline() -> serde_json::Value {
        serde_json::json!({"fingerprint": FP, "median_ms": 2.0})
    }

    #[test]
    fn correct_tight_samples_score_with_speedup() {
        let s = score(
            &baseline(),
            FP,
            &[1.0, 1.0, 1.0],
            true,
            1.0,
            1,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "scored");
        assert_eq!(s.speedup, Some(2.0));
        assert!(s.fast);
        assert!(!s.detail.is_empty());
    }

    #[test]
    fn wrong_kernel_scores_zero_despite_samples() {
        let s = score(
            &baseline(),
            FP,
            &[0.5, 0.5, 0.5],
            false,
            1.0,
            1,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "incorrect");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
    }

    #[test]
    fn stale_fingerprint_is_refused_not_scored() {
        let other = FP.replace("sm_120", "sm_90");
        let s = score(
            &baseline(),
            &other,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "stale");
        assert_eq!(s.speedup, None);
        assert!(s.detail.contains("stale") || s.detail.contains("arch"));
    }

    #[test]
    fn incorrect_beats_stale_so_arm_failures_are_never_hidden() {
        let other = FP.replace("sm_120", "sm_90");
        let s = score(
            &baseline(),
            &other,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "incorrect");
    }

    #[test]
    fn wide_spread_is_unreliable_not_scored() {
        let s = score(
            &baseline(),
            FP,
            &[1.0, 10.0],
            true,
            1.0,
            1,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "unreliable");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
    }

    #[test]
    fn slow_but_correct_scores_without_fast() {
        let s = score(
            &baseline(),
            FP,
            &[4.0, 4.0, 4.0],
            true,
            1.0,
            1,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "scored");
        assert_eq!(s.speedup, Some(0.5));
        assert!(!s.fast);
    }

    #[test]
    fn empty_samples_are_unreliable() {
        let s = score(&baseline(), FP, &[], true, 1.0, 1, "", 0, false, false);
        assert_eq!(s.status, "unreliable");
    }

    #[test]
    fn fewer_than_min_samples_is_unreliable() {
        let s = score(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            3,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "unreliable");
        assert!(s.detail.contains("too few samples"));
        let s = score(
            &baseline(),
            FP,
            &[1.0, 1.0, 1.0],
            true,
            1.0,
            3,
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "scored");
    }

    #[test]
    fn missing_baseline_file_exits_4_with_status() {
        let gone =
            std::env::temp_dir().join(format!("fb-kernel-score-gone-{}", std::process::id()));
        let mut out = Vec::new();
        let code = run(
            &gone,
            FP,
            &[1.0],
            true,
            1.0,
            1,
            &[],
            "",
            0,
            false,
            false,
            &mut out,
        );
        assert_eq!(code, 4);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("stale") || text.contains("no baseline"));
    }

    #[test]
    fn run_codes_distinguish_scored_from_incorrect() {
        let dir = std::env::temp_dir().join(format!("fb-kernel-score-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline()).unwrap()).expect("write");
        let mut out = Vec::new();
        assert_eq!(
            run(
                &path,
                FP,
                &[1.0, 1.0],
                true,
                1.0,
                1,
                &[],
                "",
                0,
                false,
                false,
                &mut out
            ),
            0
        );
        let mut out = Vec::new();
        assert_eq!(
            run(
                &path,
                FP,
                &[1.0, 1.0],
                false,
                1.0,
                1,
                &[],
                "",
                0,
                false,
                false,
                &mut out
            ),
            1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tamper() -> Vec<farmerbob_core::kernel_trust::Tamper> {
        vec![farmerbob_core::kernel_trust::Tamper {
            file: "verify.sh".to_string(),
            before: "aaa".to_string(),
            after: "bbb".to_string(),
        }]
    }

    /// A tampered run is VOID with the file and both hashes named, and gets
    /// NO score -- not a low score, not a rank (bead farmerbob-x81s.1).
    #[test]
    fn a_tampered_run_is_void_never_scored() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &tamper(),
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "void");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
        assert!(
            s.detail.contains("VOID") && s.detail.contains("verify.sh"),
            "{}",
            s.detail
        );
        assert!(
            s.detail.contains("aaa") && s.detail.contains("bbb"),
            "{}",
            s.detail
        );
    }

    /// Tamper dominates incorrect: a run that cheated and is wrong is void,
    /// never merely wrong.
    #[test]
    fn void_beats_incorrect() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            &tamper(),
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "void");
    }

    /// A void run exits 1, like every other not-scored verdict.
    #[test]
    fn a_void_run_exits_1() {
        let dir = std::env::temp_dir().join(format!("fb-kernel-score-void-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline()).unwrap()).expect("write");
        let mut out = Vec::new();
        assert_eq!(
            run(
                &path,
                FP,
                &[1.0, 1.0],
                true,
                1.0,
                1,
                &tamper(),
                "",
                0,
                false,
                false,
                &mut out
            ),
            1
        );
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("void") && text.contains("verify.sh"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clock-tainted run is `clock`: named, distinct from slow and from
    /// incorrect, with no speedup and fast false (bead farmerbob-x81s.2).
    #[test]
    fn a_clock_tainted_run_is_clock_never_scored() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &[],
            "tampered:time.perf_counter: timing primitive rebound during eval",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "clock");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
        assert!(s.detail.contains("tampered"), "{}", s.detail);
    }

    /// Clock dominates incorrect but yields to tamper: integrity verdicts
    /// outrank content verdicts, and the file rewrite is the stronger claim.
    #[test]
    fn clock_beats_incorrect_but_yields_to_void() {
        let clock = "divergent:reported=0.001ms x 10 trials > wall=12.40s";
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            &[],
            clock,
            0,
            false,
            false,
        );
        assert_eq!(s.status, "clock");
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &tamper(),
            clock,
            0,
            false,
            false,
        );
        assert_eq!(s.status, "void");
    }

    /// A clock run exits 1 with the verdict on stdout.
    #[test]
    fn a_clock_run_exits_1() {
        let dir =
            std::env::temp_dir().join(format!("fb-kernel-score-clock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline()).unwrap()).expect("write");
        let mut out = Vec::new();
        assert_eq!(
            run(
                &path,
                FP,
                &[1.0, 1.0],
                true,
                1.0,
                1,
                &[],
                "tampered:torch.cuda.Event",
                0,
                false,
                false,
                &mut out
            ),
            1
        );
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("clock") && text.contains("tampered"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A median below the physics floor is Unreliable naming the floor and
    /// the figure -- the same verdict as a wide IQR, on a different axis
    /// (bead farmerbob-x81s.6). 8 GiB through 1792 GB/s cannot beat
    /// ~4.79 ms, so a 1.0 ms median did not happen.
    #[test]
    fn below_floor_is_unreliable_naming_floor_and_figure() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &[],
            "",
            8_589_934_592,
            false,
            false,
        );
        assert_eq!(s.status, "unreliable");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
        assert!(s.detail.contains("physics floor"), "{}", s.detail);
        assert!(s.detail.contains("4.793"), "{}", s.detail);
        assert_eq!(s.median_ms, Some(1.0));
    }

    /// Above the floor scores: a 1.0 ms median against a ~0.0006 ms floor
    /// (1 MB through 1792 GB/s) is merely not-impossible, and the speedup
    /// stands.
    #[test]
    fn above_floor_scores() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &[],
            "",
            1_000_000,
            false,
            false,
        );
        assert_eq!(s.status, "scored");
        assert_eq!(s.speedup, Some(2.0));
        assert!(s.fast);
    }

    /// A zero io_bytes disables the check, and an unknown GPU skips it:
    /// a missing table row is not evidence against the run.
    #[test]
    fn floor_off_or_unknown_gpu_scores() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[0.001, 0.001],
            true,
            1.0,
            1,
            &[],
            "",
            0,
            false,
            false,
        );
        assert_eq!(s.status, "scored");
        let other_fp = FP.replace("RTX5090", "H100");
        let other_baseline = serde_json::json!({"fingerprint": other_fp, "median_ms": 2.0});
        let s = score_with_trust(
            &other_baseline,
            &other_fp,
            &[0.001, 0.001],
            true,
            1.0,
            1,
            &[],
            "",
            8_589_934_592,
            false,
            false,
        );
        assert_eq!(s.status, "scored");
    }

    /// A specialised run is its own verdict: named, no speedup, fast
    /// false -- never incorrect (bead farmerbob-x81s.4).
    #[test]
    fn specialised_is_its_own_verdict_never_incorrect() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            &[],
            "",
            0,
            true,
            false,
        );
        assert_eq!(s.status, "specialised");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
        assert!(s.detail.contains("specialis"), "{}", s.detail);
    }

    /// Specialised dominates incorrect (it says more) but yields to void
    /// and clock (integrity first).
    #[test]
    fn specialised_beats_incorrect_yields_to_void_and_clock() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            &[],
            "",
            0,
            true,
            false,
        );
        assert_eq!(s.status, "specialised");
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            &tamper(),
            "",
            0,
            true,
            false,
        );
        assert_eq!(s.status, "void");
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &[],
            "tampered:x",
            0,
            true,
            false,
        );
        assert_eq!(s.status, "clock");
    }

    /// A specialised run exits 1 with the verdict on stdout.
    #[test]
    fn a_specialised_run_exits_1() {
        let dir = std::env::temp_dir().join(format!(
            "fb-kernel-score-specialised-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline()).unwrap()).expect("write");
        let mut out = Vec::new();
        assert_eq!(
            run(
                &path,
                FP,
                &[1.0, 1.0],
                false,
                1.0,
                1,
                &[],
                "",
                0,
                true,
                false,
                &mut out
            ),
            1
        );
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("specialised"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reference call is its own verdict: named, no speedup, fast
    /// false -- never incorrect, never slow (bead farmerbob-x81s.5).
    #[test]
    fn reference_call_is_its_own_verdict() {
        let s = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &[],
            "",
            0,
            false,
            true,
        );
        assert_eq!(s.status, "reference");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
        assert!(s.detail.contains("answer key"), "{}", s.detail);
    }

    /// Reference dominates specialised and incorrect, yields to void and
    /// clock: provenance first.
    #[test]
    fn reference_beats_specialised_yields_to_void_and_clock() {
        let both = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            false,
            1.0,
            1,
            &[],
            "",
            0,
            true,
            true,
        );
        assert_eq!(both.status, "reference");
        let clocked = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &[],
            "tampered:x",
            0,
            false,
            true,
        );
        assert_eq!(clocked.status, "clock");
        let voided = score_with_trust(
            &baseline(),
            FP,
            &[1.0, 1.0],
            true,
            1.0,
            1,
            &tamper(),
            "",
            0,
            false,
            true,
        );
        assert_eq!(voided.status, "void");
    }

    /// A reference run exits 1 with the verdict on stdout.
    #[test]
    fn a_reference_run_exits_1() {
        let dir =
            std::env::temp_dir().join(format!("fb-kernel-score-reference-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline()).unwrap()).expect("write");
        let mut out = Vec::new();
        assert_eq!(
            run(
                &path,
                FP,
                &[1.0, 1.0],
                true,
                1.0,
                1,
                &[],
                "",
                0,
                false,
                true,
                &mut out
            ),
            1
        );
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("reference"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A task recorded unmeasurable on this fingerprint is answered, not
    /// missing: the verdict names the record (bead farmerbob-x81s.13). A
    /// record from another fingerprint is plain missing -- a new GPU may
    /// measure it.
    #[test]
    fn unmeasurable_record_answers_where_a_missing_file_does_not() {
        let dir =
            std::env::temp_dir().join(format!("fb-kernel-score-nobase-{}", std::process::id()));
        let klein = dir.join("ref");
        std::fs::create_dir_all(&klein).expect("scratch");
        std::fs::write(
            klein.join("unmeasurable.json"),
            serde_json::json!({
                "fingerprint": FP,
                "reason": "structural-oom",
                "detail": "inputs ~8.6 GB x4 peak",
            })
            .to_string(),
        )
        .expect("write");
        let mut out = Vec::new();
        assert_eq!(
            run(
                &klein.join("baseline.json"),
                FP,
                &[1.0],
                true,
                1.0,
                1,
                &[],
                "",
                0,
                false,
                false,
                &mut out
            ),
            4
        );
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("unmeasurable"), "{text}");
        assert!(text.contains("structural-oom"), "{text}");
        // Another fingerprint: the record is stale, the file is missing.
        let mut out = Vec::new();
        assert_eq!(
            run(
                &klein.join("baseline.json"),
                &FP.replace("sm_120", "sm_90"),
                &[1.0],
                true,
                1.0,
                1,
                &[],
                "",
                0,
                false,
                false,
                &mut out
            ),
            4
        );
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("recorded unmeasurable"), "{text}");
        assert!(text.contains("no baseline"), "STALE-CASE-MISSING: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Scoring inputs carried by one `fb bench --json` object (bead
/// farmerbob-x81s.15): the machine-readable twin that lets a wave pipe
/// bench into scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchJsonInputs {
    /// Fresh bench samples in ms.
    pub samples: Vec<f64>,
    /// Whether verification passed.
    pub correct: bool,
    /// Clock verdict, `""` when clean.
    pub clock: String,
    /// Memory traffic in bytes, 0 when unreported.
    pub io_bytes: u64,
    /// Held-out verdict.
    pub specialised: bool,
    /// Reference-call answer.
    pub reference_call: bool,
}

/// Parse bench JSON. Missing keys default to the fail-closed values
/// (incorrect, clean clock, no bytes, no flags): a bench object that
/// cannot say what it measured must never score.
pub fn parse_bench_json(text: &str) -> Result<BenchJsonInputs, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("bench JSON is not JSON: {e}"))?;
    let samples = match value.get("samples") {
        None => Vec::new(),
        Some(list) => {
            let Some(items) = list.as_array() else {
                return Err("bench JSON samples is not an array".to_string());
            };
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(ms) = item.as_f64() else {
                    return Err(format!("bench JSON sample is not a number: {item}"));
                };
                out.push(ms);
            }
            out
        }
    };
    let verify = value.get("verify");
    let flag = |key: &str| {
        verify
            .and_then(|v| v.get(key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    Ok(BenchJsonInputs {
        samples,
        correct: flag("correct"),
        clock: value
            .get("clock")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        io_bytes: value
            .get("io_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        specialised: flag("specialised"),
        reference_call: flag("reference_call"),
    })
}

#[cfg(test)]
mod bench_json_tests {
    use super::*;

    /// The full bench object parses field for field.
    #[test]
    fn full_bench_object_parses() {
        let inputs = parse_bench_json(
            r#"{"task":"t","status":"measured","samples":[19.5,19.2],"verify":{"correct":true,"specialised":false,"reference_call":false},"clock":null,"io_bytes":8589934592,"detail":"x"}"#,
        )
        .expect("parse");
        assert_eq!(inputs.samples, vec![19.5, 19.2]);
        assert!(inputs.correct);
        assert_eq!(inputs.clock, "");
        assert_eq!(inputs.io_bytes, 8_589_934_592);
        assert!(!inputs.specialised);
        assert!(!inputs.reference_call);
    }

    /// Missing keys fail closed: nothing scorable by confusion.
    #[test]
    fn missing_keys_fail_closed() {
        let inputs = parse_bench_json(r#"{"status":"instrument"}"#).expect("parse");
        assert!(inputs.samples.is_empty());
        assert!(!inputs.correct);
        assert_eq!(inputs.clock, "");
        assert_eq!(inputs.io_bytes, 0);
    }

    /// Clock and flags ride through for tainted runs.
    #[test]
    fn tainted_runs_ride_through() {
        let inputs = parse_bench_json(
            r#"{"status":"clock","samples":[],"verify":null,"clock":"tampered:x","io_bytes":null,"detail":"y"}"#,
        )
        .expect("parse");
        assert_eq!(inputs.clock, "tampered:x");
        assert!(!inputs.correct);
    }

    /// Malformed objects and non-number samples are refused, naming why.
    #[test]
    fn malformed_objects_are_refused() {
        assert!(parse_bench_json("not json").is_err());
        assert!(parse_bench_json(r#"{"samples":{}}"#).is_err());
        assert!(parse_bench_json(r#"{"samples":["fast"]}"#).is_err());
    }
}
