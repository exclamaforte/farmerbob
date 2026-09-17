//! Grades with provenance: the orchestrator's judgement about one candidate,
//! recorded so the bandit can learn from it.
//!
//! A grade is only as trustworthy as the evidence it rests on. Every grade
//! records which evidence it was built from, so that evidence can later be
//! retracted and the grades built on it go with it. Grades of different
//! provenance are never silently averaged: [`weighted`] weights each grade by
//! its basis strength, and [`is_undecided`] lets an orchestrator learn that
//! its evidence does not separate the field.
//!
//! This module is pure logic: no I/O.

/// Where a grade's authority comes from. Exactly these and no others.
///
/// Variants are declared weakest-first so that the derived `Ord` matches
/// strength: `Conformance > CrossExam > ProvenClaim > Judgement`. The strongest
/// basis is the most authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// The orchestrator's own reading. Weakest, and never sufficient alone.
    Judgement,
    /// A confirmed, veto-surviving claim from a critic.
    ProvenClaim,
    /// Cross-examination against rival suites.
    CrossExam,
    /// A frozen conformance suite executed against the candidate.
    Conformance,
}

/// A recorded judgement about one candidate on one task.
#[derive(Debug, Clone, PartialEq)]
pub struct Grade {
    /// Candidate identifier.
    pub arm: String,
    /// Task identifier.
    pub task: String,
    /// Score in `0.0..=1.0`. Rejected outside that range.
    pub score: f64,
    /// Where this grade's authority comes from.
    pub basis: Basis,
    /// Identifier of the evidence this rests on, e.g. a suite name or claim id.
    /// Empty is rejected: a grade with no traceable evidence cannot be retracted.
    pub evidence: String,
    /// When the grade was recorded, in milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// Why a grade was refused by [`Ledger::record`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    /// The score was outside `0.0..=1.0`.
    ScoreOutOfRange,
    /// The score was not finite (`NaN` or infinite and in range).
    NotFinite,
    /// The evidence was empty or whitespace-only.
    NoEvidence,
    /// A Judgement grade submitted with no corroborating grade of a stronger basis.
    UnsupportedJudgement,
}

/// A store of live grades, in the order they were recorded.
///
/// Grades rest on evidence ids; [`Ledger::retract`] removes every grade resting
/// on a retracted id. Retraction does not cascade: a stored Judgement left
/// without its supporting grade stays stored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    /// All live grades, oldest first.
    grades: Vec<Grade>,
}

impl Ledger {
    /// Creates an empty ledger.
    pub fn new() -> Self {
        Self { grades: Vec::new() }
    }

    /// Records a grade, validating fields before storing.
    ///
    /// Validation is applied in a deterministic order: score range, then
    /// finiteness, then evidence, and finally Judgement support. A grade that
    /// fails is not stored.
    ///
    /// A Judgement is accepted only when this ledger already holds a grade for
    /// the same `(arm, task)` whose basis is stronger than Judgement.
    pub fn record(&mut self, g: Grade) -> Result<(), Invalid> {
        if g.score < 0.0 || g.score > 1.0 {
            return Err(Invalid::ScoreOutOfRange);
        }
        if !g.score.is_finite() {
            return Err(Invalid::NotFinite);
        }
        if g.evidence.trim().is_empty() {
            return Err(Invalid::NoEvidence);
        }
        if g.basis == Basis::Judgement
            && !self
                .grades
                .iter()
                .any(|live| live.arm == g.arm && live.task == g.task && live.basis > Basis::Judgement)
        {
            return Err(Invalid::UnsupportedJudgement);
        }
        self.grades.push(g);
        Ok(())
    }

    /// Retracts every grade resting on this evidence id, returning how many went.
    ///
    /// Matching is by exact evidence id. Everything else is left untouched, and
    /// retraction does not cascade to grades that cited other evidence.
    pub fn retract(&mut self, evidence: &str) -> usize {
        let before = self.grades.len();
        self.grades.retain(|g| g.evidence != evidence);
        before - self.grades.len()
    }

    /// All live grades for one candidate, newest first.
    pub fn grades(&self, arm: &str, task: &str) -> Vec<&Grade> {
        let mut out: Vec<&Grade> = self
            .grades
            .iter()
            .filter(|g| g.arm == arm && g.task == task)
            .collect();
        // Stable: equal timestamps keep their recorded order.
        out.sort_by_key(|g| std::cmp::Reverse(g.at_ms));
        out
    }

    /// The grade that should be believed: the one with the strongest basis,
    /// and among equals the newest. `None` when there are none.
    ///
    /// An `at_ms` tie keeps the first recorded. A stored Judgement left
    /// unsupported by retraction is still returned when it is what remains.
    pub fn authoritative(&self, arm: &str, task: &str) -> Option<&Grade> {
        let mut best: Option<&Grade> = None;
        for g in self
            .grades
            .iter()
            .filter(|g| g.arm == arm && g.task == task)
        {
            let replace = match best {
                None => true,
                Some(b) => g.basis > b.basis || (g.basis == b.basis && g.at_ms > b.at_ms),
            };
            if replace {
                best = Some(g);
            }
        }
        best
    }
}

/// Returns the weight of one basis in [`weighted`].
fn weight(basis: Basis) -> f64 {
    match basis {
        Basis::Conformance => 4.0,
        Basis::CrossExam => 3.0,
        Basis::ProvenClaim => 2.0,
        Basis::Judgement => 1.0,
    }
}

/// Mean score weighted by basis strength, where Conformance counts 4,
/// CrossExam 3, ProvenClaim 2 and Judgement 1. `None` for an empty set --
/// not 0.0 -- and `None` rather than a non-finite value if the inputs do not
/// admit a finite mean.
pub fn weighted(grades: &[&Grade]) -> Option<f64> {
    if grades.is_empty() {
        return None;
    }
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for g in grades {
        let w = weight(g.basis);
        numerator += g.score * w;
        denominator += w;
    }
    if denominator <= 0.0 {
        return None;
    }
    let mean = numerator / denominator;
    if mean.is_finite() { Some(mean) } else { None }
}

/// True when no grade separates the candidates: every arm's authoritative
/// score is within `epsilon`, or some arm has none at all.
///
/// Returns true when any named arm has no authoritative grade for `task`,
/// and for an empty arm list. Separation is read as `max - min <= epsilon`.
pub fn is_undecided(l: &Ledger, task: &str, arms: &[String], epsilon: f64) -> bool {
    let mut scores = Vec::with_capacity(arms.len());
    for arm in arms {
        match l.authoritative(arm, task) {
            Some(g) => scores.push(g.score),
            None => return true,
        }
    }
    let mut iter = scores.iter();
    let first = match iter.next() {
        Some(v) => *v,
        None => return true,
    };
    let mut min = first;
    let mut max = first;
    for &s in iter {
        if s < min {
            min = s;
        }
        if s > max {
            max = s;
        }
    }
    max - min <= epsilon
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a grade for tests, defaulting to a supported shape the caller adjusts.
    fn grade(arm: &str, task: &str, score: f64, basis: Basis, evidence: &str, at_ms: u64) -> Grade {
        Grade {
            arm: arm.to_string(),
            task: task.to_string(),
            score,
            basis,
            evidence: evidence.to_string(),
            at_ms,
        }
    }

    #[test]
    fn bare_judgement_is_rejected() {
        let mut ledger = Ledger::new();
        let err = ledger
            .record(grade("a", "t", 0.8, Basis::Judgement, "note-1", 1))
            .unwrap_err();
        assert_eq!(err, Invalid::UnsupportedJudgement);
        assert!(ledger.grades("a", "t").is_empty());
    }

    #[test]
    fn judgement_after_conformance_is_accepted() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.6, Basis::Conformance, "suite-1", 1))
            .unwrap();
        ledger
            .record(grade("a", "t", 0.7, Basis::Judgement, "note-1", 2))
            .unwrap();
        assert_eq!(ledger.grades("a", "t").len(), 2);
    }

    #[test]
    fn validation_order_is_deterministic_when_two_rules_break_at_once() {
        let mut ledger = Ledger::new();
        // Range before finiteness: +infinity is out of range first.
        let err = ledger
            .record(grade("a", "t", f64::INFINITY, Basis::Conformance, "", 1))
            .unwrap_err();
        assert_eq!(err, Invalid::ScoreOutOfRange);
        // Finiteness before evidence: NaN beats empty evidence.
        let err = ledger
            .record(grade("a", "t", f64::NAN, Basis::Conformance, "   ", 1))
            .unwrap_err();
        assert_eq!(err, Invalid::NotFinite);
        // Evidence before support: an empty-evidence Judgement reports NoEvidence.
        let err = ledger
            .record(grade("a", "t", 0.5, Basis::Judgement, "  ", 1))
            .unwrap_err();
        assert_eq!(err, Invalid::NoEvidence);
        assert!(ledger.grades("a", "t").is_empty());
    }

    #[test]
    fn retract_returns_the_count_and_does_not_cascade() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.6, Basis::Conformance, "suite-1", 1))
            .unwrap();
        ledger
            .record(grade("a", "t", 0.9, Basis::Judgement, "note-1", 2))
            .unwrap();
        ledger
            .record(grade("b", "t", 0.4, Basis::CrossExam, "suite-1", 3))
            .unwrap();
        assert_eq!(ledger.retract("suite-1"), 2);
        assert_eq!(ledger.retract("suite-1"), 0);
        // The Judgement on other evidence survives: no cascade.
        let remaining = ledger.grades("a", "t");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].basis, Basis::Judgement);
        // The orphaned Judgement is what remains, so it is authoritative.
        let auth = ledger.authoritative("a", "t").unwrap();
        assert_eq!(auth.basis, Basis::Judgement);
        assert!(ledger.grades("b", "t").is_empty());
    }

    #[test]
    fn authoritative_prefers_basis_over_recency() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.9, Basis::Conformance, "suite-1", 1))
            .unwrap();
        ledger
            .record(grade("a", "t", 0.1, Basis::ProvenClaim, "claim-1", 99))
            .unwrap();
        let auth = ledger.authoritative("a", "t").unwrap();
        assert_eq!(auth.basis, Basis::Conformance);
        assert_eq!(auth.score, 0.9);
    }

    #[test]
    fn weighted_is_none_on_empty() {
        assert_eq!(weighted(&[]), None);
    }

    #[test]
    fn arm_with_no_grade_makes_the_field_undecided() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.9, Basis::Conformance, "suite-1", 1))
            .unwrap();
        assert!(is_undecided(
            &ledger,
            "t",
            &["a".to_string(), "b".to_string()],
            0.01
        ));
    }

    #[test]
    fn basis_strength_orders_conformance_first() {
        assert!(Basis::Conformance > Basis::CrossExam);
        assert!(Basis::CrossExam > Basis::ProvenClaim);
        assert!(Basis::ProvenClaim > Basis::Judgement);
    }

    #[test]
    fn authoritative_breaks_a_basis_tie_by_newest_then_first_recorded() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.3, Basis::CrossExam, "m-1", 5))
            .unwrap();
        ledger
            .record(grade("a", "t", 0.8, Basis::CrossExam, "m-2", 9))
            .unwrap();
        assert_eq!(ledger.authoritative("a", "t").unwrap().score, 0.8);

        let mut tied = Ledger::new();
        tied.record(grade("a", "t", 0.3, Basis::CrossExam, "m-1", 5))
            .unwrap();
        tied.record(grade("a", "t", 0.8, Basis::CrossExam, "m-2", 5))
            .unwrap();
        assert_eq!(tied.authoritative("a", "t").unwrap().score, 0.3);
    }

    #[test]
    fn grades_come_back_newest_first() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.2, Basis::Conformance, "s-1", 1))
            .unwrap();
        ledger
            .record(grade("a", "t", 0.5, Basis::ProvenClaim, "c-1", 7))
            .unwrap();
        ledger
            .record(grade("a", "other", 0.9, Basis::Conformance, "s-2", 9))
            .unwrap();
        let got = ledger.grades("a", "t");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].at_ms, 7);
        assert_eq!(got[1].at_ms, 1);
    }

    #[test]
    fn weighted_counts_conformance_four_and_judgement_one() {
        let strong = grade("a", "t", 1.0, Basis::Conformance, "s-1", 1);
        let weak = grade("a", "t", 0.0, Basis::Judgement, "n-1", 2);
        assert_eq!(weighted(&[&strong, &weak]), Some(0.8));
        let single = grade("a", "t", 0.5, Basis::ProvenClaim, "c-1", 3);
        assert_eq!(weighted(&[&single]), Some(0.5));
    }

    #[test]
    fn close_scores_are_undecided_and_spread_scores_are_not() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.80, Basis::Conformance, "s-a", 1))
            .unwrap();
        ledger
            .record(grade("b", "t", 0.84, Basis::Conformance, "s-b", 2))
            .unwrap();
        let arms = ["a".to_string(), "b".to_string()];
        assert!(is_undecided(&ledger, "t", &arms, 0.1));
        assert!(!is_undecided(&ledger, "t", &arms, 0.01));
    }

    #[test]
    fn judgement_needs_support_on_the_same_arm_and_task() {
        let mut ledger = Ledger::new();
        ledger
            .record(grade("a", "t", 0.6, Basis::Conformance, "suite-1", 1))
            .unwrap();
        // Same arm, different task: still unsupported.
        let err = ledger
            .record(grade("a", "other", 0.7, Basis::Judgement, "n-1", 2))
            .unwrap_err();
        assert_eq!(err, Invalid::UnsupportedJudgement);
        // Different arm, same task: still unsupported.
        let err = ledger
            .record(grade("b", "t", 0.7, Basis::Judgement, "n-2", 3))
            .unwrap_err();
        assert_eq!(err, Invalid::UnsupportedJudgement);
    }
}


