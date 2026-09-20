//! The plan between the benchmark reader and the scorer.
//!
//! The benchmark path has both ends — [`crate::bench_read`] turns a run's
//! captured stdout into measurements, and [`crate::task_contract::score`]
//! turns a verification plus measurements into a
//! [`crate::task_contract::Score`] — and no middle. This module is that
//! middle: a pure function of the tally of what has happened so far that
//! decides whether to run another trial, stop with enough measurements, or
//! abandon the run because the instrument, not the candidate, failed. It runs
//! nothing, reads no clock, and never inspects a measurement; [`Progress`] is
//! already the tally of what `bench_read` decided.

use crate::task_contract::TaskManifest;

/// What has happened across the trials attempted so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// Trials attempted, successful or not.
    pub attempted: u32,
    /// Measurements successfully read.
    pub good: u32,
    /// Runs that failed to read, for any reason.
    pub bad: u32,
}

/// What the caller should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Run another trial.
    Run,
    /// Stop: enough measurements. Carries how many were gathered.
    Enough(u32),
    /// Stop: too many trials failed to produce a measurement, so the
    /// INSTRUMENT is suspect and the candidate must not be blamed.
    /// Carries the failure count.
    Abandon(u32),
}

/// Decide what to do next.
///
/// `max_attempts` bounds total work. `max_bad` is how many unreadable runs are
/// tolerated before the instrument is judged unreliable.
///
/// The checks are ordered because some overrides are explicit. A zero attempt
/// budget abandons whatever else holds — nothing may be attempted. Failures
/// beyond tolerance abandon even a run that already has enough measurements.
/// Enough measurements stop the loop early. An exhausted budget with too few
/// measurements abandons: the candidate was never measured. Only a run short
/// on both measurements and attempts continues.
pub fn next(m: &TaskManifest, p: &Progress, max_attempts: u32, max_bad: u32) -> Next {
    if max_attempts == 0 || p.bad > max_bad {
        return Next::Abandon(p.bad);
    }
    if p.good >= m.min_trials {
        return Next::Enough(p.good);
    }
    if p.attempted >= max_attempts {
        return Next::Abandon(p.bad);
    }
    Next::Run
}

/// Whether a run of trials ended in a state that may be scored at all.
///
/// Only [`Next::Enough`] may be scored. `Run` has not finished, and an
/// abandoned run would blame the candidate for the harness.
pub fn scorable(n: &Next) -> bool {
    matches!(n, Next::Enough(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_contract::{TaskName, Verification};

    fn manifest(min_trials: u32) -> TaskManifest {
        TaskManifest {
            name: TaskName("t".into()),
            description: String::new(),
            verification: Verification::Benchmark,
            timeout_s: 0,
            exclusive: Vec::new(),
            min_trials,
            correctness_precision: "fp32".to_string(),
            correctness_trials: 1,
            determinism: "required".to_string(),
        }
    }

    fn progress(attempted: u32, good: u32, bad: u32) -> Progress {
        Progress {
            attempted,
            good,
            bad,
        }
    }

    /// Clause 1: short on measurements, within tolerance, attempts remain.
    /// The second case sits at `bad == max_bad`: tolerance allows the limit.
    #[test]
    fn continues_while_short_and_within_tolerance() {
        assert_eq!(next(&manifest(5), &progress(3, 2, 1), 10, 2), Next::Run);
        assert_eq!(next(&manifest(5), &progress(4, 2, 2), 10, 2), Next::Run);
    }

    /// Clause 2: enough measurements stop the loop early — at equality and
    /// above — and `Enough` carries the count gathered.
    #[test]
    fn enough_stops_before_attempts_run_out() {
        assert_eq!(
            next(&manifest(3), &progress(3, 3, 0), 10, 2),
            Next::Enough(3)
        );
        assert_eq!(
            next(&manifest(3), &progress(7, 4, 0), 10, 2),
            Next::Enough(4)
        );
    }

    /// Boundary: `min_trials: 0` asks for no minimum; zero samples are
    /// `Enough(0)`.
    #[test]
    fn zero_minimum_is_enough_at_zero_samples() {
        assert_eq!(
            next(&manifest(0), &progress(0, 0, 0), 10, 0),
            Next::Enough(0)
        );
    }

    /// Clause 3, pinned against clause 2 with the same `good`: failures
    /// beyond tolerance abandon even a run with enough measurements; at the
    /// limit exactly, the run is still scorable. The payload is the failure
    /// count.
    #[test]
    fn failures_beyond_tolerance_abandon_despite_enough_good() {
        assert_eq!(
            next(&manifest(3), &progress(9, 5, 3), 10, 2),
            Next::Abandon(3)
        );
        assert_eq!(
            next(&manifest(3), &progress(9, 5, 2), 10, 2),
            Next::Enough(5)
        );
    }

    /// Clauses 4 and 5 as a pair: the same `attempted` and `max_attempts`,
    /// differing only in `good`. With enough measurements the run is scored;
    /// with too few it is abandoned, including at `bad == max_bad`, which is
    /// within tolerance.
    #[test]
    fn exhausted_attempts_split_on_good() {
        assert_eq!(
            next(&manifest(3), &progress(6, 3, 1), 6, 2),
            Next::Enough(3)
        );
        assert_eq!(
            next(&manifest(3), &progress(6, 2, 1), 6, 2),
            Next::Abandon(1)
        );
        assert_eq!(
            next(&manifest(3), &progress(6, 2, 2), 6, 2),
            Next::Abandon(2)
        );
    }

    /// Boundary: `max_attempts: 0` abandons before anything may be attempted.
    #[test]
    fn zero_attempt_budget_abandons_immediately() {
        assert_eq!(
            next(&manifest(3), &progress(0, 0, 0), 0, 2),
            Next::Abandon(0)
        );
    }

    /// Boundary: `max_bad: 0` — a single unreadable run abandons, while zero
    /// bad runs are within tolerance.
    #[test]
    fn zero_tolerance_abandons_on_first_bad_run() {
        assert_eq!(
            next(&manifest(3), &progress(2, 1, 1), 10, 0),
            Next::Abandon(1)
        );
        assert_eq!(next(&manifest(3), &progress(2, 1, 0), 10, 0), Next::Run);
        assert_eq!(
            next(&manifest(2), &progress(2, 2, 0), 10, 0),
            Next::Enough(2)
        );
    }

    /// `attempted` is not required to equal `good + bad`: a trial may be in
    /// flight, tallied as neither. The decision reads `attempted` itself,
    /// never the sum.
    #[test]
    fn in_flight_trials_do_not_fool_the_decision() {
        assert_eq!(
            next(&manifest(5), &progress(10, 3, 1), 5, 2),
            Next::Abandon(1)
        );
        assert_eq!(
            next(&manifest(5), &progress(10, 5, 1), 5, 2),
            Next::Enough(5)
        );
    }

    /// Clause 6: only `Enough` may be scored; `Run` has not finished and an
    /// abandoned run blames the candidate for the harness.
    #[test]
    fn only_enough_is_scorable() {
        assert!(scorable(&Next::Enough(1)));
        assert!(!scorable(&Next::Run));
        assert!(!scorable(&Next::Abandon(1)));
    }

    /// Clause 7: the decision reads `min_trials` and nothing else from the
    /// manifest — two manifests differing only in `name` or `description`
    /// decide identically across every outcome.
    #[test]
    fn decision_reads_only_min_trials() {
        let mut renamed = manifest(2);
        renamed.name = TaskName("other".into());
        renamed.description = "a different task".into();
        let cases = [
            progress(0, 0, 0),
            progress(4, 2, 1),
            progress(5, 1, 1),
            progress(5, 1, 3),
        ];
        for p in &cases {
            assert_eq!(next(&manifest(2), p, 5, 2), next(&renamed, p, 5, 2));
        }
    }
}
