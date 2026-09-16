//! Attempt log: append-only storage of raw execution evidence for posterior recomputation.

use serde::{Deserialize, Serialize};

/// Classification of an attempt outcome.
///
/// Only `ArmResult` outcomes contribute to an arm's success statistics.
/// All other classes represent failures that are not the arm's fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutcomeClass {
    /// The arm produced a result that was judged.
    ArmResult,
    /// Infrastructure failure (rate limits, network, adapter unavailable, etc.).
    Infrastructure,
    /// The orchestrator cancelled the run.
    OrchestratorCancelled,
    /// The task itself was invalid.
    TaskInvalid,
    /// A bug in the harness/judging infrastructure.
    HarnessBug,
    /// Unclassified/unknown failure.
    Unknown,
}

impl OutcomeClass {
    /// Returns true only for `ArmResult` — the only class that updates an arm's posterior.
    pub fn counts_for_posterior(self) -> bool {
        matches!(self, OutcomeClass::ArmResult)
    }
}

/// A single recorded attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// Unique identifier for this attempt.
    pub attempt_id: String,
    /// The arm that was attempted.
    pub arm: String,
    /// Bucket the router used, e.g. "feature:routine".
    pub bucket: String,
    /// Classification of the outcome.
    pub outcome: OutcomeClass,
    /// Whether the arm's work was accepted. Only meaningful when `outcome` is `ArmResult`.
    pub accepted: Option<bool>,
    /// Cost in USD.
    pub cost_usd: f64,
    /// Latency in seconds.
    pub latency_s: u64,
    /// 1 for a first attempt; higher for a retry from a failing state.
    pub attempt_number: u32,
    /// Version of the scoring suite this was judged against.
    pub suite_version: u32,
}

/// Beta distribution posterior parameters.
///
/// Starts from Beta(1, 1) (uniform prior). Each accepted `ArmResult` with
/// `attempt_number == 1` and matching `suite_version` adds 1 to alpha;
/// each rejected such attempt adds 1 to beta.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Posterior {
    pub alpha: f64,
    pub beta: f64,
}

impl Posterior {
    /// Mean of the Beta distribution: alpha / (alpha + beta).
    pub fn mean(&self) -> f64 {
        let denom = self.alpha + self.beta;
        if denom == 0.0 {
            0.5
        } else {
            self.alpha / denom
        }
    }

    /// Effective sample size: alpha + beta - 2 (given Beta(1,1) prior).
    pub fn n(&self) -> f64 {
        (self.alpha + self.beta - 2.0).max(0.0)
    }
}

/// Append-only log of attempts.
///
/// Never mutated in place; `record` appends, all queries recompute from raw evidence.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptLog {
    attempts: Vec<Attempt>,
}

impl AttemptLog {
    /// Create a new empty log.
    pub fn new() -> Self {
        Self { attempts: Vec::new() }
    }

    /// Append an attempt. The log is never mutated in place.
    pub fn record(&mut self, a: Attempt) {
        self.attempts.push(a);
    }

    /// Number of recorded attempts.
    pub fn len(&self) -> usize {
        self.attempts.len()
    }

    /// True if no attempts have been recorded.
    pub fn is_empty(&self) -> bool {
        self.attempts.is_empty()
    }

    /// Recompute a posterior for one arm in one bucket, from raw evidence.
    ///
    /// Filtering rules:
    /// - Only `OutcomeClass::ArmResult` attempts contribute
    /// - Only `attempt_number == 1` (retries are easier problems and corrupt the posterior)
    /// - Only attempts whose `suite_version` equals the given `suite_version`
    /// - `accepted == Some(true)` adds to alpha, `Some(false)` adds to beta, `None` is skipped
    /// - Starts from Beta(1, 1)
    pub fn posterior(&self, arm: &str, bucket: &str, suite_version: u32) -> Posterior {
        let mut alpha = 1.0;
        let mut beta = 1.0;

        for attempt in &self.attempts {
            if attempt.arm != arm {
                continue;
            }
            if attempt.bucket != bucket {
                continue;
            }
            if attempt.suite_version != suite_version {
                continue;
            }
            if attempt.attempt_number != 1 {
                continue;
            }
            if attempt.outcome != OutcomeClass::ArmResult {
                continue;
            }

            match attempt.accepted {
                Some(true) => alpha += 1.0,
                Some(false) => beta += 1.0,
                None => {} // skipped
            }
        }

        Posterior { alpha, beta }
    }

    /// Total measured spend for an arm, across every attempt including excluded ones.
    ///
    /// A run that failed for infrastructure reasons still cost money.
    pub fn spend(&self, arm: &str) -> f64 {
        self.attempts
            .iter()
            .filter(|a| a.arm == arm)
            .map(|a| a.cost_usd)
            .sum()
    }

    /// Mean latency over contributing attempts only.
    ///
    /// Contributing attempts are those that would be included in `posterior`:
    /// `ArmResult`, `attempt_number == 1`, matching `suite_version`, and `accepted != None`.
    /// Returns `None` when there are no contributing attempts.
    pub fn mean_latency(&self, arm: &str, bucket: &str) -> Option<f64> {
        let mut sum = 0u64;
        let mut count = 0usize;

        for attempt in &self.attempts {
            if attempt.arm != arm {
                continue;
            }
            if attempt.bucket != bucket {
                continue;
            }
            if attempt.outcome != OutcomeClass::ArmResult {
                continue;
            }
            if attempt.attempt_number != 1 {
                continue;
            }
            if attempt.accepted.is_none() {
                continue;
            }

            sum += attempt.latency_s;
            count += 1;
        }

        if count == 0 {
            None
        } else {
            Some(sum as f64 / count as f64)
        }
    }

    /// Attempts excluded from the posterior, by class, for one arm.
    ///
    /// Returns a vector of (OutcomeClass, count) pairs for all non-ArmResult outcomes
    /// for the given arm, across all buckets and suite versions.
    pub fn excluded(&self, arm: &str) -> Vec<(OutcomeClass, usize)> {
        use std::collections::HashMap;

        let mut counts: HashMap<OutcomeClass, usize> = HashMap::new();

        for attempt in &self.attempts {
            if attempt.arm != arm {
                continue;
            }
            if attempt.outcome == OutcomeClass::ArmResult {
                continue;
            }
            *counts.entry(attempt.outcome).or_insert(0) += 1;
        }

        let mut result: Vec<_> = counts.into_iter().collect();
        result.sort_by_key(|(oc, _)| *oc as u8);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_attempt(arm: &str, bucket: &str, outcome: OutcomeClass) -> Attempt {
        Attempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            arm: arm.to_string(),
            bucket: bucket.to_string(),
            outcome,
            accepted: None,
            cost_usd: 0.01,
            latency_s: 10,
            attempt_number: 1,
            suite_version: 1,
        }
    }

    #[test]
    fn infrastructure_outcome_does_not_move_posterior_but_counts_toward_spend() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::Infrastructure));
        log.attempts.last_mut().unwrap().cost_usd = 0.05;

        let post = log.posterior("arm1", "feature:routine", 1);
        assert_eq!(post.alpha, 2.0); // prior 1 + 1 accepted
        assert_eq!(post.beta, 1.0); // prior 1

        let spend = log.spend("arm1");
        assert!((spend - 0.06).abs() < f64::EPSILON);
    }

    #[test]
    fn attempt_number_two_is_excluded_from_posterior() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);
        log.attempts.last_mut().unwrap().attempt_number = 1;

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(false);
        log.attempts.last_mut().unwrap().attempt_number = 2;

        let post = log.posterior("arm1", "feature:routine", 1);
        assert_eq!(post.alpha, 2.0); // only attempt_number == 1 counts
        assert_eq!(post.beta, 1.0);
    }

    #[test]
    fn different_suite_version_is_excluded_from_posterior() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);
        log.attempts.last_mut().unwrap().suite_version = 1;

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(false);
        log.attempts.last_mut().unwrap().suite_version = 2;

        let post = log.posterior("arm1", "feature:routine", 1);
        assert_eq!(post.alpha, 2.0); // only suite_version == 1 counts
        assert_eq!(post.beta, 1.0);
    }

    #[test]
    fn accepted_none_on_arm_result_is_skipped_not_counted_as_failure() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = None;

        let post = log.posterior("arm1", "feature:routine", 1);
        assert_eq!(post.alpha, 2.0); // only the Some(true) counts
        assert_eq!(post.beta, 1.0); // None is skipped, not counted as failure
    }

    #[test]
    fn posterior_on_arm_with_no_attempts_returns_beta_1_1_mean_0_5() {
        let log = AttemptLog::new();
        let post = log.posterior("nonexistent", "feature:routine", 1);
        assert_eq!(post.alpha, 1.0);
        assert_eq!(post.beta, 1.0);
        assert!((post.mean() - 0.5).abs() < f64::EPSILON);
        assert_eq!(post.n(), 0.0);
    }

    #[test]
    fn mean_latency_returns_none_when_no_contributing_attempts() {
        let log = AttemptLog::new();
        assert_eq!(log.mean_latency("arm1", "feature:routine"), None);
    }

    #[test]
    fn mean_latency_only_averages_contributing_attempts() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);
        log.attempts.last_mut().unwrap().latency_s = 10;

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(false);
        log.attempts.last_mut().unwrap().latency_s = 20;

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::Infrastructure));
        log.attempts.last_mut().unwrap().latency_s = 100; // excluded

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = None;
        log.attempts.last_mut().unwrap().latency_s = 50; // excluded (accepted is None)

        let mean = log.mean_latency("arm1", "feature:routine");
        assert!((mean.unwrap() - 15.0).abs() < f64::EPSILON); // (10 + 20) / 2
    }

    #[test]
    fn excluded_counts_by_outcome_class_for_an_arm() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::Infrastructure));
        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::Infrastructure));
        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::TaskInvalid));
        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);

        let excluded = log.excluded("arm1");
        assert_eq!(excluded.len(), 2);
        // Sorted by OutcomeClass discriminant
        assert_eq!(excluded[0].0, OutcomeClass::Infrastructure);
        assert_eq!(excluded[0].1, 2);
        assert_eq!(excluded[1].0, OutcomeClass::TaskInvalid);
        assert_eq!(excluded[1].1, 1);
    }

    #[test]
    fn spend_includes_all_attempts_regardless_of_outcome() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().cost_usd = 0.10;

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::Infrastructure));
        log.attempts.last_mut().unwrap().cost_usd = 0.05;

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::OrchestratorCancelled));
        log.attempts.last_mut().unwrap().cost_usd = 0.02;

        let spend = log.spend("arm1");
        assert!((spend - 0.17).abs() < f64::EPSILON);
    }

    #[test]
    fn posterior_filters_by_bucket() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);

        log.record(base_attempt("arm1", "feature:complex", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(false);

        let post_routine = log.posterior("arm1", "feature:routine", 1);
        assert_eq!(post_routine.alpha, 2.0);
        assert_eq!(post_routine.beta, 1.0);

        let post_complex = log.posterior("arm1", "feature:complex", 1);
        assert_eq!(post_complex.alpha, 1.0);
        assert_eq!(post_complex.beta, 2.0);
    }

    #[test]
    fn posterior_n_returns_effective_sample_size() {
        let mut log = AttemptLog::new();

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(true);

        log.record(base_attempt("arm1", "feature:routine", OutcomeClass::ArmResult));
        log.attempts.last_mut().unwrap().accepted = Some(false);

        let post = log.posterior("arm1", "feature:routine", 1);
        // Beta(1+2, 1+1) = Beta(3, 2), n = 3+2-2 = 3
        assert_eq!(post.n(), 3.0);
    }
}