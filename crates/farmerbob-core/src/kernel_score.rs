//! Kernel benchmark aggregation: median, IQR, and fast_p.
//!
//! [`crate::task_contract::score`] gates ONE experiment: correctness first,
//! then enough trials, then spread. This module aggregates many experiments
//! into the numbers a leaderboard reports, per KernelBench semantics:
//!
//! - per task: median and IQR over repeated trials, never a single number;
//! - across tasks: `fast_p`, the fraction of tasks both correct AND faster
//!   than `p` times the local baseline;
//! - reliability: a result whose IQR is wide relative to its median is
//!   marked unreliable rather than silently averaged.
//!
//! The module is pure: no I/O.

/// Median and spread of one task's repeated measurements, in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// Median of the samples.
    pub median: f64,
    /// First quartile (median of the lower half).
    pub q1: f64,
    /// Third quartile (median of the upper half).
    pub q3: f64,
    /// Inter-quartile range (`q3 - q1`).
    pub iqr: f64,
    /// How many samples were summarised.
    pub trials: u32,
}

/// Summarise repeated trials. `None` for an empty slice: no measurements is
/// not a zero-median measurement.
pub fn summarize(samples: &[f64]) -> Option<Summary> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = median_of(&sorted)?;
    let n = sorted.len();
    let (lower, upper) = if n.is_multiple_of(2) {
        (sorted[..n / 2].to_vec(), sorted[n / 2..].to_vec())
    } else if n == 1 {
        (sorted.clone(), sorted.clone())
    } else {
        (sorted[..n / 2].to_vec(), sorted[n / 2 + 1..].to_vec())
    };
    let q1 = median_of(&lower).unwrap_or(median);
    let q3 = median_of(&upper).unwrap_or(median);
    Some(Summary {
        median,
        q1,
        q3,
        iqr: (q3 - q1).max(0.0),
        trials: n as u32,
    })
}

fn median_of(sorted: &[f64]) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    } else {
        Some(sorted[mid])
    }
}

/// One task's outcome for the cross-task aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskOutcome {
    /// Whether the candidate kernel was correct.
    pub correct: bool,
    /// Candidate median ms divided into the baseline median ms. `None` when
    /// the task was never measured (incorrect, unreliable, or missing).
    pub speedup: Option<f64>,
}

/// Fraction of tasks both correct and faster than `p` times the baseline.
///
/// A wrong kernel scores zero regardless of speed: `correct == false` never
/// counts, even with a speedup attached. An unmeasured task (`speedup`
/// `None`) never counts either. An empty set scores `0.0`: no evidence of
/// fast work is not evidence of fast work.
pub fn fast_p(outcomes: &[TaskOutcome], p: f64) -> f64 {
    if outcomes.is_empty() {
        return 0.0;
    }
    let fast = outcomes
        .iter()
        .filter(|o| o.correct && o.speedup.is_some_and(|s| s.is_finite() && s > p))
        .count();
    fast as f64 / outcomes.len() as f64
}

/// Whether a task's repeated measurements are tight enough to trust.
///
/// `max_iqr_ratio` bounds `iqr / median`: a result varying widely relative
/// to its own centre is unreliable rather than averagable. A non-positive or
/// non-finite median is never reliable: there is no centre to be tight
/// around.
pub fn reliable(summary: &Summary, max_iqr_ratio: f64) -> bool {
    if !summary.median.is_finite() || summary.median <= 0.0 {
        return false;
    }
    if !summary.iqr.is_finite() || summary.iqr < 0.0 {
        return false;
    }
    summary.iqr / summary.median <= max_iqr_ratio
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_samples_have_no_summary() {
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn single_sample_summarises_with_zero_spread() {
        assert_eq!(
            summarize(&[4.0]),
            Some(Summary {
                median: 4.0,
                q1: 4.0,
                q3: 4.0,
                iqr: 0.0,
                trials: 1,
            })
        );
    }

    #[test]
    fn odd_samples_report_tukey_hinges() {
        // sorted: 1 2 3 4 5 6 7 8; median 4.5 is checked elsewhere —
        // here an odd count: median 3, lower half median 1.5, upper 4.5.
        let s = summarize(&[5.0, 1.0, 4.0, 2.0, 3.0]).expect("summary");
        assert_eq!(s.median, 3.0);
        assert_eq!(s.q1, 1.5);
        assert_eq!(s.q3, 4.5);
        assert_eq!(s.iqr, 3.0);
        assert_eq!(s.trials, 5);
    }

    #[test]
    fn even_samples_split_halves() {
        let s = summarize(&[1.0, 2.0, 3.0, 4.0]).expect("summary");
        assert_eq!(s.median, 2.5);
        assert_eq!(s.q1, 1.5);
        assert_eq!(s.q3, 3.5);
        assert_eq!(s.iqr, 2.0);
    }

    #[test]
    fn fast_p_counts_only_correct_and_faster() {
        let outcomes = vec![
            TaskOutcome {
                correct: true,
                speedup: Some(2.0),
            },
            TaskOutcome {
                correct: true,
                speedup: Some(1.0),
            },
            TaskOutcome {
                correct: false,
                speedup: Some(9.0),
            },
            TaskOutcome {
                correct: true,
                speedup: None,
            },
        ];
        assert_eq!(fast_p(&outcomes, 1.0), 0.25);
        assert_eq!(fast_p(&outcomes, 0.5), 0.5);
    }

    #[test]
    fn fast_p_wrong_is_zero_despite_speedup() {
        let outcomes = vec![TaskOutcome {
            correct: false,
            speedup: Some(100.0),
        }];
        assert_eq!(fast_p(&outcomes, 1.0), 0.0);
    }

    #[test]
    fn fast_p_empty_is_zero_not_nan() {
        assert_eq!(fast_p(&[], 1.0), 0.0);
    }

    #[test]
    fn tight_measurements_are_reliable_wide_are_not() {
        let tight = summarize(&[10.0, 10.1, 9.9, 10.0]).expect("summary");
        assert!(reliable(&tight, 0.5));
        let wide = summarize(&[1.0, 10.0]).expect("summary");
        assert!(!reliable(&wide, 0.5));
    }

    #[test]
    fn non_positive_median_is_never_reliable() {
        let zero = Summary {
            median: 0.0,
            q1: 0.0,
            q3: 0.0,
            iqr: 0.0,
            trials: 2,
        };
        assert!(!reliable(&zero, 99.0));
    }
}
