//! Turn what the harness observed about one candidate worktree into the fate
//! that should befall it.
//!
//! `fb-verify.sh` collapses four facts -- did the crate build, did its tests
//! run, did the harness's own timeout fire, and did the arm write the declared
//! deliverable -- into one pass/fail line. Three of the four failures print
//! the same thing, which hides the only distinction that matters: a missing
//! deliverable is a no-op BY THE ARM, a broken build or a failing or empty
//! suite is a defect IN THE ARM, and a harness timeout is a property of the
//! MACHINE, which must never be reported as an arm's failure.
//!
//! This module is the layer above [`crate::build_verdict`]: that module
//! decides build-versus-test from the logs; this one consumes its answer plus
//! the other three facts and decides what happens to the candidate.

use crate::build_verdict::BuildVerdict;

/// What the harness observed about one candidate worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// Whether the declared deliverable exists and is non-empty.
    pub deliverable: bool,
    /// The build-or-test verdict, already decided by `build_verdict`.
    /// `None` when the build was never attempted.
    pub build: Option<BuildVerdict>,
    /// Whether the run was cut short by the harness's own timeout.
    pub timed_out: bool,
}

/// What should happen to a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// Measure it: it built, its suite ran, and it wrote the deliverable.
    Measure {
        /// How many tests passed, carried unchanged from the verdict.
        tests_passed: u32,
    },
    /// The arm wrote nothing. This is a no-op BY THE ARM.
    NoOp,
    /// The arm's code does not build or its suite fails. A defect IN THE ARM.
    Failed(
        /// Why the gate failed, for a human. Never empty.
        String,
    ),
    /// The harness cut the run short. NOT the arm's failure, and it must
    /// never be reported as one. Carries what was observed before the cut.
    Cut {
        /// Whether the deliverable existed at the moment of the cut.
        had_deliverable: bool,
    },
}

/// Decide one candidate's fate.
///
/// The timeout dominates everything: it is the machine's, not the arm's, so
/// no observation made before the cut can change it. Otherwise the build
/// verdict carries the decision, and the deliverable separates `Measure`
/// from `NoOp` on the two non-failing arms.
pub fn fate(o: &Observed) -> Fate {
    if o.timed_out {
        return Fate::Cut {
            had_deliverable: o.deliverable,
        };
    }

    match o.build {
        // The build never ran. Without a deliverable the arm did nothing
        // observable (clause 3); with one, the arm declared an artifact it
        // never once tried to build, which is an arm defect.
        None if o.deliverable => {
            Fate::Failed("the deliverable exists but the build was never attempted".to_string())
        }
        None => Fate::NoOp,
        Some(BuildVerdict::Passed { passed }) => {
            if o.deliverable {
                Fate::Measure {
                    tests_passed: passed,
                }
            } else {
                // Writing tests without the deliverable is not a deliverable.
                Fate::NoOp
            }
        }
        Some(BuildVerdict::Failed { failed }) => Fate::Failed(format!("{failed} tests failed")),
        // A suite that executed nothing is not a suite that passed nothing.
        Some(BuildVerdict::NoTests) => Fate::Failed("the suite ran no tests".to_string()),
        Some(BuildVerdict::BuildFailed) => {
            Fate::Failed("the crate failed while building".to_string())
        }
    }
}

/// How many candidates of each fate a field contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Candidates whose fate is `Measure`.
    pub measurable: usize,
    /// Candidates whose fate is `NoOp`.
    pub no_op: usize,
    /// Candidates whose fate is `Failed`.
    pub failed: usize,
    /// Candidates whose fate is `Cut`.
    pub cut: usize,
}

/// Tally a field.
///
/// Every fate lands in exactly one bucket, so the four counters always sum to
/// the input length.
pub fn field(fates: &[Fate]) -> Field {
    let mut f = Field {
        measurable: 0,
        no_op: 0,
        failed: 0,
        cut: 0,
    };
    for fate in fates {
        match fate {
            Fate::Measure { .. } => f.measurable += 1,
            Fate::NoOp => f.no_op += 1,
            Fate::Failed(_) => f.failed += 1,
            Fate::Cut { .. } => f.cut += 1,
        }
    }
    f
}

/// Whether a field can be ranked at all.
///
/// Ranking needs at least two measurable candidates: one candidate is not a
/// comparison, and zero is not a field. Failures and cuts do not unrank a
/// field; only the measurable count is read.
pub fn rankable(f: &Field) -> bool {
    f.measurable >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(deliverable: bool, build: Option<BuildVerdict>, timed_out: bool) -> Observed {
        Observed {
            deliverable,
            build,
            timed_out,
        }
    }

    fn reason_of(f: Fate) -> String {
        match f {
            Fate::Failed(reason) => reason,
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn a_timeout_cuts_whatever_else_was_observed() {
        // Clause 1 is unconditional, so it must hold against an observation
        // that would otherwise be Measure and one that would otherwise be
        // Failed -- and the two cuts must not collapse (clause 2).
        let cut_candidates = [
            (
                observed(true, Some(BuildVerdict::Passed { passed: 2 }), true),
                true,
            ),
            (
                observed(false, Some(BuildVerdict::BuildFailed), true),
                false,
            ),
            // A timeout with no deliverable and no build is a Cut, not a NoOp.
            (observed(false, None, true), false),
        ];

        for (candidate, had_deliverable) in cut_candidates {
            assert_eq!(fate(&candidate), Fate::Cut { had_deliverable });
        }
    }

    #[test]
    fn an_arm_that_never_started_is_a_no_op() {
        assert_eq!(fate(&observed(false, None, false)), Fate::NoOp);
    }

    #[test]
    fn passing_tests_without_the_deliverable_are_still_a_no_op() {
        // The case an implementation keyed on the build verdict alone gets
        // wrong: a passing suite is not a deliverable.
        assert_eq!(
            fate(&observed(
                false,
                Some(BuildVerdict::Passed { passed: 5 }),
                false
            )),
            Fate::NoOp
        );
    }

    #[test]
    fn a_build_failure_names_the_build() {
        let reason = reason_of(fate(&observed(
            true,
            Some(BuildVerdict::BuildFailed),
            false,
        )));

        assert!(!reason.is_empty());
        assert!(reason.to_lowercase().contains("building"));
    }

    #[test]
    fn a_test_failure_names_the_tests() {
        let reason = reason_of(fate(&observed(
            true,
            Some(BuildVerdict::Failed { failed: 3 }),
            false,
        )));

        assert!(!reason.is_empty());
        assert!(reason.to_lowercase().contains("tests"));
    }

    #[test]
    fn build_and_test_failures_need_different_fixes() {
        let build = reason_of(fate(&observed(
            true,
            Some(BuildVerdict::BuildFailed),
            false,
        )));
        let tests = reason_of(fate(&observed(
            true,
            Some(BuildVerdict::Failed { failed: 3 }),
            false,
        )));

        assert_ne!(build, tests);
    }

    #[test]
    fn measure_carries_the_observed_count_unchanged() {
        assert_eq!(
            fate(&observed(
                true,
                Some(BuildVerdict::Passed { passed: 7 }),
                false
            )),
            Fate::Measure { tests_passed: 7 }
        );
    }

    #[test]
    fn a_suite_that_ran_nothing_is_not_a_suite_that_passed_nothing() {
        // Clauses 8 and 9 as a pair: mapping NoTests to a zero count passes
        // clause 8 alone and fails here.
        assert!(matches!(
            fate(&observed(true, Some(BuildVerdict::NoTests), false)),
            Fate::Failed(_)
        ));
        assert_eq!(
            fate(&observed(
                true,
                Some(BuildVerdict::Passed { passed: 2 }),
                false
            )),
            Fate::Measure { tests_passed: 2 }
        );
    }

    #[test]
    fn field_partitions_its_input() {
        let fates = [
            Fate::Measure { tests_passed: 4 },
            Fate::NoOp,
            Fate::Failed("broken".to_string()),
            Fate::Cut {
                had_deliverable: true,
            },
            Fate::Measure { tests_passed: 1 },
            Fate::Cut {
                had_deliverable: false,
            },
        ];

        let f = field(&fates);

        assert_eq!(
            f,
            Field {
                measurable: 2,
                no_op: 1,
                failed: 1,
                cut: 2,
            }
        );
        assert_eq!(f.measurable + f.no_op + f.failed + f.cut, fates.len());
    }

    #[test]
    fn an_empty_field_counts_nothing_and_ranks_nothing() {
        assert_eq!(
            field(&[]),
            Field {
                measurable: 0,
                no_op: 0,
                failed: 0,
                cut: 0,
            }
        );
        assert!(!rankable(&field(&[])));
    }

    #[test]
    fn rankability_is_the_two_measurable_boundary() {
        let one = field(&[Fate::Measure { tests_passed: 3 }]);
        assert!(!rankable(&one));

        let two = field(&[
            Fate::Measure { tests_passed: 3 },
            Fate::Measure { tests_passed: 4 },
        ]);
        assert!(rankable(&two));

        // Two candidates of which one is Cut is one measurable, not two.
        let one_of_two = field(&[
            Fate::Measure { tests_passed: 3 },
            Fate::Cut {
                had_deliverable: true,
            },
        ]);
        assert!(!rankable(&one_of_two));

        // rankable reads only `measurable`: failures and no-ops do not unrank
        // a field that has its two measurable candidates.
        let noisy = field(&[
            Fate::Measure { tests_passed: 3 },
            Fate::Measure { tests_passed: 4 },
            Fate::NoOp,
            Fate::Failed("broken".to_string()),
            Fate::Cut {
                had_deliverable: false,
            },
        ]);
        assert!(rankable(&noisy));
    }
}
