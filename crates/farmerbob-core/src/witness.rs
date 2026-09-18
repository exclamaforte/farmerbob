//! Interpretation of test suite failure patterns across an implementation field.
//!
//! When evaluating candidate implementations against a suite of tests, observing
//! that multiple implementations failed does not inherently mean the suite is
//! over-fitted to its author. An over-fitted suite fails everywhere for the same
//! reason (a single disagreement or shared idiosyncratic reading of the spec),
//! whereas a strict, correct suite meeting a field of flawed rivals fails for
//! different reasons (separate, independent defects).
//!
//! This module inspects test failure witnesses across an arm field and
//! distinguishes these cases.

use std::collections::{BTreeMap, BTreeSet};

/// Which of a suite's tests failed against one implementation.
///
/// Test names as the runner printed them. An empty set means the suite passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    /// The implementation the suite ran against.
    pub impl_arm: String,
    /// Names of the failing tests, sorted, without duplicates.
    ///
    /// If an unsorted or non-deduplicated vector is passed, functions in this
    /// module normalize it by sorting and deduplicating without panicking.
    pub failed: Vec<String>,
}

/// What a suite's failures across a field actually indicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The suite passed against every implementation it ran on.
    Clean,
    /// Every failing implementation failed on the SAME set of tests. One
    /// disagreement, repeated -- the suite encodes a reading its author's
    /// code satisfies and the field does not.
    ///
    /// Even if every witness in the field fails on the identical set (including
    /// the suite's own author), this remains [`Verdict::OneDisagreement`].
    /// That a suite fails the whole field is a different fact and is not this
    /// function's job to classify separately.
    OneDisagreement {
        /// The tests every failing implementation failed on, sorted.
        ///
        /// This vector is sorted and non-empty by construction:
        /// [`Verdict::OneDisagreement`] is produced only when at least two
        /// witnesses fail, and by definition a failing witness has at least
        /// one failing test. Because all failing witnesses share the exact same
        /// non-empty set of tests, `shared` cannot be empty.
        shared: Vec<String>,
    },
    /// Failing implementations failed on DIFFERENT tests. Not one
    /// disagreement but several, which is what a strict correct suite
    /// looks like against a flawed field.
    ///
    /// Partial overlap is [`Verdict::SeparateFaults`], not [`Verdict::OneDisagreement`].
    /// If arm A fails `{x, y}` and arm B fails `{x}`, the suite found something
    /// in A that it did not find in B, so there is more than one disagreement.
    SeparateFaults,
    /// Fewer than two implementations failed, so "same or different" has no
    /// content. NOT a judgement: one failure is one data point.
    ///
    /// With one failure there is nothing to compare it against, and calling it
    /// either [`Verdict::OneDisagreement`] or [`Verdict::SeparateFaults`] is
    /// an assertion the data does not support.
    TooFewFailures,
    /// No witness carried any test name. The cells recorded pass/fail and
    /// nothing else, so nothing can be concluded. Distinct from [`Verdict::Clean`]:
    /// a suite that passed and a suite nobody recorded are different.
    ///
    /// Note: Witnesses that all carry empty `failed` yield [`Verdict::Clean`],
    /// but a slice where every entry has an empty `failed` AND the caller had
    /// no test names to give is indistinguishable from it. The caller must not
    /// pass empty sets to mean "unknown".
    NoEvidence,
}

/// Read the witnesses for ONE suite across a field.
///
/// `witnesses` holds one entry per implementation the suite ran against,
/// including the suite's own author.
///
/// # Falsifiable Rules
///
/// 1. An empty `witnesses` slice yields [`Verdict::NoEvidence`].
/// 2. If every witness in `witnesses` has an empty `failed` list, yields
///    [`Verdict::Clean`]. There is no minimum number of witnesses for this
///    clause (a single passing witness yields [`Verdict::Clean`]).
/// 3. Exactly ONE witness failing, on any number of tests, yields
///    [`Verdict::TooFewFailures`]. Never [`Verdict::OneDisagreement`] and never
///    [`Verdict::SeparateFaults`]. With one failure there is nothing to compare
///    it against, and calling it either way is an assertion the data does not
///    support.
/// 4. Two or more failing witnesses with the IDENTICAL set of failing test
///    names yields [`Verdict::OneDisagreement`], carrying that set in `shared`.
///    Even if every witness fails on identical sets (including the suite's own
///    author), this remains [`Verdict::OneDisagreement`].
/// 5. Two or more failing witnesses whose failing sets are not all identical
///    yields [`Verdict::SeparateFaults`]. Partial overlap is
///    [`Verdict::SeparateFaults`], not [`Verdict::OneDisagreement`]: if A fails
///    `{x, y}` and B fails `{x}`, the suite found something in A that it did
///    not find in B, so there is more than one disagreement.
/// 6. Witnesses that all carry empty `failed` yield [`Verdict::Clean`], but a
///    slice where every entry has an empty `failed` and the caller had no test
///    names to give is indistinguishable from it. The caller must not pass
///    empty sets to mean "unknown".
/// 7. Comparison of test and arm names is EXACT (case-sensitive).
/// 8. Handled gracefully without panicking: duplicate test names within a
///    witness are deduplicated and treated as once; unsorted test lists are
///    sorted; duplicate arm names are accepted without panicking.
pub fn read(witnesses: &[Witness]) -> Verdict {
    if witnesses.is_empty() {
        return Verdict::NoEvidence;
    }

    let mut failing_sets: Vec<Vec<String>> = Vec::new();

    for w in witnesses {
        if !w.failed.is_empty() {
            let mut set = w.failed.clone();
            set.sort();
            set.dedup();
            if !set.is_empty() {
                failing_sets.push(set);
            }
        }
    }

    match failing_sets.len() {
        0 => Verdict::Clean,
        1 => Verdict::TooFewFailures,
        _ => {
            if let Some(first) = failing_sets.first() {
                let all_identical = failing_sets.iter().skip(1).all(|s| s == first);
                if all_identical {
                    Verdict::OneDisagreement {
                        shared: first.clone(),
                    }
                } else {
                    Verdict::SeparateFaults
                }
            } else {
                Verdict::Clean
            }
        }
    }
}

/// The tests that failed against exactly one implementation, with that
/// implementation's name. Sorted by test name, then by arm.
///
/// These are the strongest single-defect evidence a matrix carries: a test
/// that only one arm fails is pointing at that arm, not at the spec.
///
/// # Behavior
///
/// - Returns a test name exactly when exactly one witness lists it. A test
///   failed by two or more arms is not unique and is excluded.
/// - On [`Verdict::Clean`] input or empty input, returns an empty vector.
/// - If every failure was shared across multiple implementations, returns an
///   empty vector.
/// - Returns `(test_name, arm)` pairs sorted by test name, then by arm name,
///   without duplicates.
/// - If a witness lists duplicate test names in `failed`, they are deduplicated
///   and counted as one occurrence for that witness.
pub fn unique_failures(witnesses: &[Witness]) -> Vec<(String, String)> {
    if witnesses.is_empty() {
        return Vec::new();
    }

    struct Occurrence<'a> {
        count: usize,
        arm: &'a str,
    }

    let mut occurrences: BTreeMap<&str, Occurrence<'_>> = BTreeMap::new();

    for w in witnesses {
        let mut seen_in_witness = BTreeSet::new();
        for test in &w.failed {
            if seen_in_witness.insert(test.as_str()) {
                occurrences
                    .entry(test.as_str())
                    .and_modify(|occ| occ.count += 1)
                    .or_insert(Occurrence {
                        count: 1,
                        arm: w.impl_arm.as_str(),
                    });
            }
        }
    }

    let mut unique = Vec::new();
    for (test, occ) in occurrences {
        if occ.count == 1 {
            unique.push((test.to_string(), occ.arm.to_string()));
        }
    }

    unique.sort();
    unique.dedup();
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    fn witness(arm: &str, failed: &[&str]) -> Witness {
        Witness {
            impl_arm: arm.to_string(),
            failed: failed.iter().map(|s| s.to_string()).collect(),
        }
    }

    // --- Clause 1 & Boundary: Clean ---

    #[test]
    fn clause1_all_passing_witnesses_yield_clean() {
        let witnesses = vec![
            witness("alpha", &[]),
            witness("beta", &[]),
            witness("gamma", &[]),
        ];
        assert_eq!(read(&witnesses), Verdict::Clean);
    }

    #[test]
    fn boundary_single_passing_witness_yields_clean() {
        // Clause 1 has no minimum: one passing witness yields Clean.
        let witnesses = vec![witness("solo", &[])];
        assert_eq!(read(&witnesses), Verdict::Clean);
    }

    // --- Clause 2 & Boundary: TooFewFailures ---

    #[test]
    fn clause2_single_failing_witness_alone_yields_too_few_failures() {
        let witnesses = vec![witness("alpha", &["test_fault"])];
        assert_eq!(read(&witnesses), Verdict::TooFewFailures);
    }

    #[test]
    fn clause2_single_failing_witness_among_passing_yields_too_few_failures() {
        let witnesses = vec![
            witness("author", &[]),
            witness("beta", &["test_precedence", "test_tab"]),
            witness("gamma", &[]),
        ];
        assert_eq!(read(&witnesses), Verdict::TooFewFailures);
    }

    #[test]
    fn clause2_single_failure_never_one_disagreement_or_separate_faults() {
        let single = vec![witness("beta", &["t1", "t2", "t3"])];
        let verdict = read(&single);
        assert_eq!(verdict, Verdict::TooFewFailures);
        assert_ne!(
            verdict,
            Verdict::OneDisagreement {
                shared: vec!["t1".into(), "t2".into(), "t3".into()]
            }
        );
        assert_ne!(verdict, Verdict::SeparateFaults);
    }

    // --- Clause 3 & Boundaries: OneDisagreement ---

    #[test]
    fn clause3_two_failing_witnesses_identical_tests_yield_one_disagreement() {
        let witnesses = vec![
            witness("arm1", &["tie_breaker"]),
            witness("arm2", &["tie_breaker"]),
        ];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["tie_breaker".to_string()]
            }
        );
    }

    #[test]
    fn clause3_three_failing_witnesses_multiple_identical_tests() {
        let witnesses = vec![
            witness("author", &[]),
            witness("rival_a", &["test_alpha", "test_beta"]),
            witness("rival_b", &["test_alpha", "test_beta"]),
            witness("rival_c", &["test_alpha", "test_beta"]),
        ];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["test_alpha".to_string(), "test_beta".to_string()]
            }
        );
    }

    #[test]
    fn boundary_every_witness_failing_identical_sets_is_one_disagreement() {
        // Whole field fails, author included: still OneDisagreement.
        let witnesses = vec![
            witness("author", &["bad_spec_reading"]),
            witness("rival_1", &["bad_spec_reading"]),
            witness("rival_2", &["bad_spec_reading"]),
        ];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["bad_spec_reading".to_string()]
            }
        );
    }

    #[test]
    fn one_disagreement_shared_is_sorted() {
        let witnesses = vec![
            witness("arm1", &["zebra", "apple"]),
            witness("arm2", &["apple", "zebra"]),
        ];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["apple".to_string(), "zebra".to_string()]
            }
        );
    }

    // --- Clause 4: SeparateFaults & Partial Overlap ---

    #[test]
    fn clause4_different_tests_yield_separate_faults() {
        let witnesses = vec![
            witness("author", &[]),
            witness("glm", &["tie_breaking_order_among_rejections"]),
            witness("nemotron", &["tab_mixed_with_spaces_is_still_a_tab"]),
        ];
        assert_eq!(read(&witnesses), Verdict::SeparateFaults);
    }

    #[test]
    fn clause4_partial_overlap_is_separate_faults_not_one_disagreement() {
        // Explicitly pinned case: A fails {x, y} and B fails {x}.
        let witnesses = vec![
            witness("arm_a", &["test_x", "test_y"]),
            witness("arm_b", &["test_x"]),
        ];
        let verdict = read(&witnesses);
        assert_eq!(
            verdict,
            Verdict::SeparateFaults,
            "Partial overlap must be SeparateFaults, not OneDisagreement"
        );
    }

    #[test]
    fn clause4_partial_overlap_with_three_arms() {
        let witnesses = vec![
            witness("arm_a", &["shared_bug", "extra_a"]),
            witness("arm_b", &["shared_bug"]),
            witness("arm_c", &["shared_bug"]),
        ];
        assert_eq!(read(&witnesses), Verdict::SeparateFaults);
    }

    #[test]
    fn clause4_differing_superset_and_subset() {
        let witnesses = vec![
            witness("arm_a", &["t1", "t2"]),
            witness("arm_b", &["t1", "t3"]),
        ];
        assert_eq!(read(&witnesses), Verdict::SeparateFaults);
    }

    // --- Clause 5 & Boundary: NoEvidence ---

    #[test]
    fn clause5_empty_witnesses_yields_no_evidence() {
        assert_eq!(read(&[]), Verdict::NoEvidence);
    }

    #[test]
    fn clause5_no_evidence_distinct_from_clean() {
        assert_ne!(Verdict::NoEvidence, Verdict::Clean);
        assert_eq!(read(&[]), Verdict::NoEvidence);
        assert_eq!(read(&[witness("a", &[])]), Verdict::Clean);
    }

    // --- Clause 7: unique_failures ---

    #[test]
    fn clause7_unique_failures_identifies_single_arm_failures() {
        let witnesses = vec![
            witness("author", &[]),
            witness("glm", &["tie_breaking_order"]),
            witness("nemotron", &["tab_mixed_with_spaces"]),
        ];
        let unique = unique_failures(&witnesses);
        assert_eq!(
            unique,
            vec![
                ("tab_mixed_with_spaces".to_string(), "nemotron".to_string()),
                ("tie_breaking_order".to_string(), "glm".to_string()),
            ]
        );
    }

    #[test]
    fn clause7_test_failed_by_two_arms_is_excluded() {
        let witnesses = vec![
            witness("arm_a", &["shared_failure", "unique_to_a"]),
            witness("arm_b", &["shared_failure", "unique_to_b"]),
        ];
        let unique = unique_failures(&witnesses);
        assert_eq!(
            unique,
            vec![
                ("unique_to_a".to_string(), "arm_a".to_string()),
                ("unique_to_b".to_string(), "arm_b".to_string()),
            ]
        );
    }

    #[test]
    fn clause7_all_failures_shared_yields_empty_unique_failures() {
        let witnesses = vec![
            witness("arm_a", &["defect_x"]),
            witness("arm_b", &["defect_x"]),
        ];
        assert_eq!(unique_failures(&witnesses), Vec::<(String, String)>::new());
    }

    #[test]
    fn clause7_sorting_by_test_name_then_arm() {
        let witnesses = vec![
            witness("arm_z", &["test_beta"]),
            witness("arm_a", &["test_alpha"]),
        ];
        let unique = unique_failures(&witnesses);
        assert_eq!(
            unique,
            vec![
                ("test_alpha".to_string(), "arm_a".to_string()),
                ("test_beta".to_string(), "arm_z".to_string()),
            ]
        );
    }

    #[test]
    fn clause7_single_failing_witness_tests_are_all_unique() {
        let witnesses = vec![witness("arm_a", &["t2", "t1"]), witness("arm_b", &[])];
        let unique = unique_failures(&witnesses);
        assert_eq!(
            unique,
            vec![
                ("t1".to_string(), "arm_a".to_string()),
                ("t2".to_string(), "arm_a".to_string()),
            ]
        );
    }

    // --- Clause 8 & Boundaries: unique_failures on clean and zero ---

    #[test]
    fn clause8_unique_failures_on_clean_returns_empty() {
        let witnesses = vec![witness("a", &[]), witness("b", &[])];
        assert_eq!(unique_failures(&witnesses), Vec::<(String, String)>::new());
    }

    #[test]
    fn boundary_unique_failures_on_zero_witnesses_returns_empty() {
        assert_eq!(unique_failures(&[]), Vec::<(String, String)>::new());
    }

    // --- Clause 9: Deduplication, casing, duplicate arms, panic-freedom ---

    #[test]
    fn clause9_duplicate_test_names_in_witness_treated_as_once() {
        let witnesses = vec![
            witness("arm1", &["t1", "t1", "t1"]),
            witness("arm2", &["t1"]),
        ];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["t1".to_string()]
            }
        );

        let unique = unique_failures(&witnesses);
        assert_eq!(unique, Vec::<(String, String)>::new());
    }

    #[test]
    fn clause9_unsorted_test_names_in_witness_handled_cleanly() {
        let witnesses = vec![
            witness("arm1", &["z", "a", "m"]),
            witness("arm2", &["a", "z", "m"]),
        ];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["a".to_string(), "m".to_string(), "z".to_string()]
            }
        );
    }

    #[test]
    fn clause9_exact_case_comparison() {
        let witnesses = vec![
            witness("arm1", &["test_foo"]),
            witness("arm2", &["TEST_FOO"]),
        ];
        assert_eq!(read(&witnesses), Verdict::SeparateFaults);

        let unique = unique_failures(&witnesses);
        assert_eq!(
            unique,
            vec![
                ("TEST_FOO".to_string(), "arm2".to_string()),
                ("test_foo".to_string(), "arm1".to_string()),
            ]
        );
    }

    #[test]
    fn clause9_duplicate_arm_names_do_not_panic() {
        let witnesses = vec![witness("dup_arm", &["t1"]), witness("dup_arm", &["t1"])];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["t1".to_string()]
            }
        );
    }

    #[test]
    fn clause9_duplicate_arm_names_differing_tests() {
        let witnesses = vec![witness("dup_arm", &["t1"]), witness("dup_arm", &["t2"])];
        assert_eq!(read(&witnesses), Verdict::SeparateFaults);
        let unique = unique_failures(&witnesses);
        assert_eq!(
            unique,
            vec![
                ("t1".to_string(), "dup_arm".to_string()),
                ("t2".to_string(), "dup_arm".to_string()),
            ]
        );
    }

    #[test]
    fn empty_test_names_are_handled_consistently() {
        let witnesses = vec![witness("arm1", &[""]), witness("arm2", &[""])];
        assert_eq!(
            read(&witnesses),
            Verdict::OneDisagreement {
                shared: vec!["".to_string()]
            }
        );
    }
}
