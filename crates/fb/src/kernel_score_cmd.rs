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
pub fn score(
    baseline_json: &serde_json::Value,
    current_fingerprint: &str,
    samples: &[f64],
    correct: bool,
    p: f64,
    min_samples: usize,
) -> TaskScore {
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
/// 0 scored, 1 not scored (incorrect/unreliable/stale), 4 unreadable.
pub fn run(
    baseline_path: &Path,
    current_fingerprint: &str,
    samples: &[f64],
    correct: bool,
    p: f64,
    min_samples: usize,
    out: &mut dyn Write,
) -> i32 {
    let content = match std::fs::read_to_string(baseline_path) {
        Ok(c) => c,
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
                    detail: format!("no baseline at {}: {e}", baseline_path.display()),
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
    let scored = score(
        &baseline_json,
        current_fingerprint,
        samples,
        correct,
        p,
        min_samples,
    );
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
        let s = score(&baseline(), FP, &[1.0, 1.0, 1.0], true, 1.0, 1);
        assert_eq!(s.status, "scored");
        assert_eq!(s.speedup, Some(2.0));
        assert!(s.fast);
        assert!(!s.detail.is_empty());
    }

    #[test]
    fn wrong_kernel_scores_zero_despite_samples() {
        let s = score(&baseline(), FP, &[0.5, 0.5, 0.5], false, 1.0, 1);
        assert_eq!(s.status, "incorrect");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
    }

    #[test]
    fn stale_fingerprint_is_refused_not_scored() {
        let other = FP.replace("sm_120", "sm_90");
        let s = score(&baseline(), &other, &[1.0, 1.0], true, 1.0, 1);
        assert_eq!(s.status, "stale");
        assert_eq!(s.speedup, None);
        assert!(s.detail.contains("stale") || s.detail.contains("arch"));
    }

    #[test]
    fn incorrect_beats_stale_so_arm_failures_are_never_hidden() {
        let other = FP.replace("sm_120", "sm_90");
        let s = score(&baseline(), &other, &[1.0, 1.0], false, 1.0, 1);
        assert_eq!(s.status, "incorrect");
    }

    #[test]
    fn wide_spread_is_unreliable_not_scored() {
        let s = score(&baseline(), FP, &[1.0, 10.0], true, 1.0, 1);
        assert_eq!(s.status, "unreliable");
        assert_eq!(s.speedup, None);
        assert!(!s.fast);
    }

    #[test]
    fn slow_but_correct_scores_without_fast() {
        let s = score(&baseline(), FP, &[4.0, 4.0, 4.0], true, 1.0, 1);
        assert_eq!(s.status, "scored");
        assert_eq!(s.speedup, Some(0.5));
        assert!(!s.fast);
    }

    #[test]
    fn empty_samples_are_unreliable() {
        let s = score(&baseline(), FP, &[], true, 1.0, 1);
        assert_eq!(s.status, "unreliable");
    }

    #[test]
    fn fewer_than_min_samples_is_unreliable() {
        let s = score(&baseline(), FP, &[1.0, 1.0], true, 1.0, 3);
        assert_eq!(s.status, "unreliable");
        assert!(s.detail.contains("too few samples"));
        let s = score(&baseline(), FP, &[1.0, 1.0, 1.0], true, 1.0, 3);
        assert_eq!(s.status, "scored");
    }

    #[test]
    fn missing_baseline_file_exits_4_with_status() {
        let gone =
            std::env::temp_dir().join(format!("fb-kernel-score-gone-{}", std::process::id()));
        let mut out = Vec::new();
        let code = run(&gone, FP, &[1.0], true, 1.0, 1, &mut out);
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
        assert_eq!(run(&path, FP, &[1.0, 1.0], true, 1.0, 1, &mut out), 0);
        let mut out = Vec::new();
        assert_eq!(run(&path, FP, &[1.0, 1.0], false, 1.0, 1, &mut out), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
