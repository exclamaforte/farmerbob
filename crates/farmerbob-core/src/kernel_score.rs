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
    /// Lower end of the speedup's 95% interval, when one was computed
    /// (bead farmerbob-x81s.20). `None` means no interval, never a
    /// zero-width one.
    pub speedup_lo: Option<f64>,
    /// Upper end of the speedup's 95% interval, when one was computed.
    pub speedup_hi: Option<f64>,
}

/// Median of per-trial ratios with an empirical 95% interval (bead
/// farmerbob-x81s.20): returns `(median, lo, hi`).
///
/// The stated assumption: the trial ratios are treated as approximately
/// normal draws, so the standard error of their centre is the sample
/// standard deviation over the square root of the trial count, and the
/// interval is the median plus or minus 1.96 standard errors. This is
/// deliberately NOT a t-interval and NOT a bootstrap: with three to five
/// trials neither buys rigour, and a named approximation beats a quiet
/// one. Fewer than two ratios yield `None`: one trial carries no
/// dispersion, and an interval from it would be fiction.
pub fn ratio_ci(ratios: &[f64]) -> Option<(f64, f64, f64)> {
    if ratios.len() < 2 {
        return None;
    }
    if ratios.iter().any(|r| !r.is_finite()) {
        return None;
    }
    let mut sorted = ratios.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = median_of(&sorted)?;
    let n = ratios.len() as f64;
    let mean = ratios.iter().sum::<f64>() / n;
    let var = ratios.iter().map(|r| (r - mean) * (r - mean)).sum::<f64>() / (n - 1.0);
    let se = (var / n).sqrt();
    if !se.is_finite() {
        return None;
    }
    let half = 1.96 * se;
    Some((median, median - half, median + half))
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

/// Fraction of tasks whose speedup exceeds `p` with the interval excluding
/// `p` (bead farmerbob-x81s.20): the reviewable sibling of [`fast_p`].
/// A task counts when it is correct, its point speedup clears `p`, AND
/// its interval's lower end clears `p` too -- the effect is separable
/// from noise at roughly 95% confidence under the [`ratio_ci`]
/// assumption. Tasks without an interval never count here: an
/// unmeasured dispersion is not a tight one. The point-estimate
/// [`fast_p`] stays beside it for comparability with published
/// KernelBench numbers.
pub fn fast_p_interval(outcomes: &[TaskOutcome], p: f64) -> f64 {
    if outcomes.is_empty() {
        return 0.0;
    }
    let fast = outcomes
        .iter()
        .filter(|o| {
            o.correct
                && o.speedup.is_some_and(|s| s.is_finite() && s > p)
                && o.speedup_lo.is_some_and(|lo| lo.is_finite() && lo > p)
        })
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

/// Physics floor in milliseconds: no implementation moves `io_bytes`
/// through `bandwidth_gbps` faster than this, whatever the clock reported
/// (bead farmerbob-x81s.6). `None` for non-positive inputs: a floor built
/// on zero bytes or zero bandwidth is not a floor.
///
/// Deliberately a lower bound, never an estimate: real kernels move more
/// than inputs-plus-output (weights, temporaries, halo reads), so a result
/// above the floor is merely not-impossible, while a result below it did
/// not happen.
pub fn floor_ms(io_bytes: u64, bandwidth_gbps: f64) -> Option<f64> {
    if io_bytes == 0 || !bandwidth_gbps.is_finite() || bandwidth_gbps <= 0.0 {
        return None;
    }
    Some(io_bytes as f64 / (bandwidth_gbps * 1e9) * 1000.0)
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
                speedup_lo: None,
                speedup_hi: None,
            },
            TaskOutcome {
                correct: true,
                speedup: Some(1.0),
                speedup_lo: None,
                speedup_hi: None,
            },
            TaskOutcome {
                correct: false,
                speedup: Some(9.0),
                speedup_lo: None,
                speedup_hi: None,
            },
            TaskOutcome {
                correct: true,
                speedup: None,
                speedup_lo: None,
                speedup_hi: None,
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
            speedup_lo: None,
            speedup_hi: None,
        }];
        assert_eq!(fast_p(&outcomes, 1.0), 0.0);
    }

    #[test]
    fn fast_p_empty_is_zero_not_nan() {
        assert_eq!(fast_p(&[], 1.0), 0.0);
        assert_eq!(fast_p_interval(&[], 1.0), 0.0);
    }

    #[test]
    fn ratio_ci_centres_on_the_median_with_named_arithmetic() {
        // Ratios 1.9..2.1: median 2.0, mean 2.0, sample var 0.025/4,
        // SE = sqrt(0.00625/5) = sqrt(0.00125) ~= 0.0354, half ~= 0.0693.
        let (median, lo, hi) = ratio_ci(&[1.9, 1.95, 2.0, 2.05, 2.1]).expect("ci");
        assert_eq!(median, 2.0);
        assert!((lo - (2.0 - 1.96 * 0.00125_f64.sqrt())).abs() < 1e-12);
        assert!((hi - (2.0 + 1.96 * 0.00125_f64.sqrt())).abs() < 1e-12);
        assert!(lo < median && median < hi);
    }

    #[test]
    fn ratio_ci_needs_two_trials_and_finite_ratios() {
        assert_eq!(ratio_ci(&[]), None);
        assert_eq!(ratio_ci(&[2.0]), None);
        assert_eq!(ratio_ci(&[2.0, f64::INFINITY]), None);
        assert_eq!(ratio_ci(&[2.0, f64::NAN]), None);
    }

    #[test]
    fn fast_p_interval_counts_only_effects_clear_of_noise() {
        let outcomes = vec![
            // Clearly fast: interval excludes 1.0.
            TaskOutcome {
                correct: true,
                speedup: Some(2.0),
                speedup_lo: Some(1.9),
                speedup_hi: Some(2.1),
            },
            // Point estimate fast, interval straddles 1.0: noise, not a claim.
            TaskOutcome {
                correct: true,
                speedup: Some(1.05),
                speedup_lo: Some(0.95),
                speedup_hi: Some(1.15),
            },
            // No interval: unmeasured dispersion is not a tight one.
            TaskOutcome {
                correct: true,
                speedup: Some(3.0),
                speedup_lo: None,
                speedup_hi: None,
            },
        ];
        assert_eq!(fast_p(&outcomes, 1.0), 1.0);
        assert_eq!(fast_p_interval(&outcomes, 1.0), 1.0 / 3.0);
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

    /// 8 GiB through 1792 GB/s is ~4.79 ms; degenerate inputs yield no
    /// floor rather than a zero one.
    #[test]
    fn floor_is_bytes_over_bandwidth() {
        let floor = floor_ms(8_589_934_592, 1792.0).expect("floor");
        assert!((floor - 4.79).abs() < 0.01, "{floor}");
        assert_eq!(floor_ms(0, 1792.0), None);
        assert_eq!(floor_ms(100, 0.0), None);
        assert_eq!(floor_ms(100, f64::INFINITY), None);
    }
}
