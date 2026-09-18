//! Classification of a test suite's results across an implementation field.
//!
//! Rather than judging a suite's quality solely by the aggregate count or shape
//! of implementation failures (which misclassifies suites catching separate real
//! defects across arms as over-fitted), this module inspects *which tests* failed
//! across the rival implementations.
//!
//! Failure pattern analysis is delegated to [`crate::witness::read`], mapping its
//! [`WitnessVerdict`] to a [`Suite`] classification. Neither [`Suite::OneDisagreement`]
//! nor [`Suite::FoundSeparateFaults`] claims a single definitive cause:
//! - [`Suite::OneDisagreement`] is consistent with both author-specific over-fitting
//!   and a correct suite encountering a shared defect across rival implementations.
//! - [`Suite::FoundSeparateFaults`] is consistent with both multiple genuine defects
//!   and a suite containing one over-fitted test alongside distinct incidental failures
//!   (partial overlap).

use crate::witness::{Verdict as WitnessVerdict, Witness};

/// What a suite's results across a field support saying about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suite {
    /// Passed against every implementation. Says nothing about quality:
    /// a suite that catches nothing and a field with no defects look
    /// identical from here.
    NoSignal,
    /// Failing implementations failed on DIFFERENT tests.
    ///
    /// This is the shape several real defects make, and it is the shape a
    /// suite with ONE over-fitted test makes when the field also has
    /// unrelated faults: a partially-overlapping set -- A fails `{x, y}`,
    /// B fails `{x}` -- lands here, and the shared `x` may be the
    /// over-fitted one. Like [`Suite::OneDisagreement`], this variant
    /// reports what the evidence shows and does NOT claim which cause
    /// produced it. `reap-plan` was exactly this shape.
    FoundSeparateFaults {
        /// How many implementations failed.
        implementations: u32,
    },
    /// Every failing implementation failed on the SAME tests. One
    /// disagreement repeated, which is what over-fitting looks like -- and
    /// also what a correct suite meeting a field with one shared defect
    /// looks like. The two are NOT distinguishable from this evidence and
    /// this variant must not claim otherwise.
    OneDisagreement {
        /// The shared failing tests, sorted.
        ///
        /// This vector is sorted and non-empty by construction:
        /// [`WitnessVerdict::OneDisagreement`] is produced by [`crate::witness::read`]
        /// only when at least two witnesses fail, and by definition a failing
        /// witness has at least one failing test. Because all failing witnesses
        /// share the exact same non-empty set of tests, `shared` cannot be empty.
        shared: Vec<String>,
        /// How many implementations failed.
        implementations: u32,
    },
    /// Not enough failures to compare, or no test names were recorded.
    /// Carries which, because "one arm failed" and "the instrument recorded
    /// nothing" are different states.
    Inconclusive {
        /// Why, in prose. Never empty.
        reason: String,
    },
}

/// Classify one suite from the witnesses of every implementation it ran
/// against, including its own author's.
///
/// Delegates to [`crate::witness::read`] to analyze same-versus-different failure
/// patterns across implementations and maps the resulting [`WitnessVerdict`] to a
/// [`Suite`].
///
/// `implementations` counts witnesses whose `failed` list is non-empty, never
/// the total number of witnesses in `witnesses`. An `implementations` count of zero
/// is reachable only for [`Suite::NoSignal`] and [`Suite::Inconclusive`], neither
/// of which carries the field.
pub fn classify(witnesses: &[Witness]) -> Suite {
    let failed_count = witnesses.iter().filter(|w| !w.failed.is_empty()).count() as u32;

    match crate::witness::read(witnesses) {
        WitnessVerdict::Clean => Suite::NoSignal,
        WitnessVerdict::SeparateFaults => Suite::FoundSeparateFaults {
            implementations: failed_count,
        },
        WitnessVerdict::OneDisagreement { shared } => Suite::OneDisagreement {
            shared,
            implementations: failed_count,
        },
        WitnessVerdict::TooFewFailures => Suite::Inconclusive {
            reason: "too few failures to compare failure patterns across arms".to_string(),
        },
        WitnessVerdict::NoEvidence => Suite::Inconclusive {
            reason: "empty witness slice: no implementations or test outcomes were recorded"
                .to_string(),
        },
    }
}

/// Whether this verdict supports discounting the suite.
///
/// True ONLY for [`Suite::OneDisagreement`], and even then it is a
/// suggestion: see the doc on that variant.
pub fn suggests_overfit(s: &Suite) -> bool {
    matches!(s, Suite::OneDisagreement { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_witness(arm: &str) -> Witness {
        Witness {
            impl_arm: arm.to_string(),
            failed: Vec::new(),
        }
    }

    fn failing_witness(arm: &str, failed: &[&str]) -> Witness {
        Witness {
            impl_arm: arm.to_string(),
            failed: failed.iter().map(|s| s.to_string()).collect(),
        }
    }

    // --- Clause 1 & 2: Clean -> NoSignal ---

    #[test]
    fn single_passing_witness_yields_no_signal() {
        let witnesses = vec![passing_witness("arm1")];
        assert_eq!(classify(&witnesses), Suite::NoSignal);
    }

    #[test]
    fn multiple_passing_witnesses_yield_no_signal() {
        let witnesses = vec![
            passing_witness("arm1"),
            passing_witness("arm2"),
            passing_witness("arm3"),
        ];
        assert_eq!(classify(&witnesses), Suite::NoSignal);
    }

    // --- Clause 3: SeparateFaults -> FoundSeparateFaults ---

    #[test]
    fn separate_faults_disjoint_failures_maps_to_found_separate_faults() {
        let witnesses = vec![
            failing_witness("arm1", &["test_a"]),
            failing_witness("arm2", &["test_b"]),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::FoundSeparateFaults { implementations: 2 }
        );
    }

    #[test]
    fn separate_faults_partial_overlap_maps_to_found_separate_faults() {
        // Partial overlap: arm1 fails {x, y}, arm2 fails {x}.
        let witnesses = vec![
            failing_witness("arm1", &["test_x", "test_y"]),
            failing_witness("arm2", &["test_x"]),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::FoundSeparateFaults { implementations: 2 }
        );
    }

    #[test]
    fn separate_faults_with_interspersed_passers() {
        let witnesses = vec![
            failing_witness("arm1", &["test_x"]),
            passing_witness("arm2"),
            failing_witness("arm3", &["test_y"]),
            passing_witness("arm4"),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::FoundSeparateFaults { implementations: 2 }
        );
    }

    // --- Clause 4: OneDisagreement -> OneDisagreement ---

    #[test]
    fn two_witnesses_failing_identical_single_element_sets() {
        let witnesses = vec![
            failing_witness("arm1", &["test_a"]),
            failing_witness("arm2", &["test_a"]),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::OneDisagreement {
                shared: vec!["test_a".to_string()],
                implementations: 2,
            }
        );
    }

    #[test]
    fn three_witnesses_failing_identical_multi_element_sets() {
        let witnesses = vec![
            failing_witness("arm1", &["test_b", "test_a"]),
            failing_witness("arm2", &["test_a", "test_b"]),
            failing_witness("arm3", &["test_a", "test_b"]),
        ];
        // witness::read sorts and deduplicates `shared`
        assert_eq!(
            classify(&witnesses),
            Suite::OneDisagreement {
                shared: vec!["test_a".to_string(), "test_b".to_string()],
                implementations: 3,
            }
        );
    }

    // --- Clause 5: TooFewFailures & NoEvidence -> Inconclusive ---

    #[test]
    fn empty_witness_slice_maps_to_inconclusive_naming_empty_input() {
        let verdict = classify(&[]);
        match verdict {
            Suite::Inconclusive { reason } => {
                assert!(!reason.is_empty());
                let lower = reason.to_lowercase();
                assert!(
                    lower.contains("empty"),
                    "empty slice reason must name empty input: {reason}"
                );
            }
            other => panic!("expected Inconclusive, got {other:?}"),
        }
    }

    #[test]
    fn single_failing_witness_maps_to_inconclusive_via_too_few_failures() {
        let witnesses = vec![failing_witness("arm1", &["test_fail"])];
        let verdict = classify(&witnesses);
        match verdict {
            Suite::Inconclusive { reason } => {
                assert!(!reason.is_empty());
            }
            other => panic!("expected Inconclusive, got {other:?}"),
        }
    }

    #[test]
    fn one_failure_with_multiple_passers_maps_to_inconclusive() {
        let witnesses = vec![
            passing_witness("arm1"),
            failing_witness("arm2", &["test_fail"]),
            passing_witness("arm3"),
        ];
        assert!(matches!(classify(&witnesses), Suite::Inconclusive { .. }));
    }

    #[test]
    fn reasons_for_too_few_failures_and_no_evidence_differ() {
        let inconclusive_empty = classify(&[]);
        let inconclusive_one_fail = classify(&[failing_witness("arm1", &["test_x"])]);

        let (reason_empty, reason_one_fail) = match (&inconclusive_empty, &inconclusive_one_fail) {
            (Suite::Inconclusive { reason: r1 }, Suite::Inconclusive { reason: r2 }) => (r1, r2),
            _ => panic!("both must be Inconclusive"),
        };

        assert_ne!(
            reason_empty, reason_one_fail,
            "reasons for NoEvidence and TooFewFailures must differ"
        );
        assert!(!reason_empty.is_empty());
        assert!(!reason_one_fail.is_empty());
    }

    // --- Clause 6: suggests_overfit pinned for all four variants ---

    #[test]
    fn suggests_overfit_pinned_across_all_four_variants() {
        let no_signal = Suite::NoSignal;
        let separate_faults = Suite::FoundSeparateFaults { implementations: 2 };
        let one_disagreement = Suite::OneDisagreement {
            shared: vec!["test_x".to_string()],
            implementations: 2,
        };
        let inconclusive = Suite::Inconclusive {
            reason: "cannot compare".to_string(),
        };

        assert!(
            suggests_overfit(&one_disagreement),
            "suggests_overfit must be true for OneDisagreement"
        );
        assert!(
            !suggests_overfit(&no_signal),
            "suggests_overfit must be false for NoSignal"
        );
        assert!(
            !suggests_overfit(&separate_faults),
            "suggests_overfit must be false for FoundSeparateFaults"
        );
        assert!(
            !suggests_overfit(&inconclusive),
            "suggests_overfit must be false for Inconclusive"
        );
    }

    // --- Clause 8: implementations counts failing witnesses, never total ---

    #[test]
    fn implementations_counts_failing_witnesses_not_total_witnesses() {
        // A suite ran against four implementations and two failed.
        let witnesses = vec![
            passing_witness("arm1"),
            failing_witness("arm2", &["test_shared"]),
            passing_witness("arm3"),
            failing_witness("arm4", &["test_shared"]),
        ];

        let verdict = classify(&witnesses);
        assert_eq!(
            verdict,
            Suite::OneDisagreement {
                shared: vec!["test_shared".to_string()],
                implementations: 2,
            }
        );
    }

    #[test]
    fn implementations_counts_failing_witnesses_in_separate_faults() {
        // Four implementations: two failed on different tests.
        let witnesses = vec![
            passing_witness("arm1"),
            failing_witness("arm2", &["test_a"]),
            passing_witness("arm3"),
            failing_witness("arm4", &["test_b"]),
        ];

        let verdict = classify(&witnesses);
        assert_eq!(verdict, Suite::FoundSeparateFaults { implementations: 2 });
    }

    // --- Boundaries & Robustness ---

    #[test]
    fn duplicate_test_names_in_witness_deduplicated() {
        let witnesses = vec![
            failing_witness("arm1", &["test_x", "test_x", "test_x"]),
            failing_witness("arm2", &["test_x"]),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::OneDisagreement {
                shared: vec!["test_x".to_string()],
                implementations: 2,
            }
        );
    }

    #[test]
    fn duplicate_arm_names_accepted_without_panicking() {
        let witnesses = vec![
            failing_witness("arm1", &["test_x"]),
            failing_witness("arm1", &["test_x"]),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::OneDisagreement {
                shared: vec!["test_x".to_string()],
                implementations: 2,
            }
        );
    }

    #[test]
    fn whole_field_failing_on_same_test_is_one_disagreement() {
        let witnesses = vec![
            failing_witness("author", &["test_x"]),
            failing_witness("rival1", &["test_x"]),
            failing_witness("rival2", &["test_x"]),
        ];
        assert_eq!(
            classify(&witnesses),
            Suite::OneDisagreement {
                shared: vec!["test_x".to_string()],
                implementations: 3,
            }
        );
    }
}
