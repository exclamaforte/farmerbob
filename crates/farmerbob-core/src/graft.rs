//! Graft planning for single-cell cross-examination reproduction.
//!
//! When investigating why one cross-examination cell failed, `fb-repro`
//! reconstructs the graft: take the implementation arm's files, strip any
//! implementation-side tests so the candidate does not grade itself, and append
//! the suite arm's test file.
//!
//! This module computes the [`Plan`] for that graft or refuses with a [`Refusal`]
//! when the cell cannot be built meaningfully.

/// One file that goes into a grafted tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Repo-relative path.
    pub path: String,
    /// Whether the file carries a `#[cfg(test)]` module.
    pub has_tests: bool,
}

/// What a graft needs, decided from the two arms' file lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Implementation files taken from the impl arm, sorted by path.
    pub take: Vec<String>,
    /// Implementation files whose test modules must be stripped first,
    /// sorted by path. A subset of `take`.
    pub strip: Vec<String>,
    /// The suite file appended from the suite arm.
    pub append: String,
}

/// Why a cell cannot be built. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The implementation arm has no file at the declared target.
    NoImpl,
    /// The suite arm has no file at the declared target.
    NoSuite,
    /// The suite arm's file carries no tests, so the cell would assert
    /// nothing and pass vacuously.
    SuiteHasNoTests,
    /// Both arms are the same. A diagonal cell is run by the caller, not
    /// grafted.
    SameArm,
}

/// Plan one cell, or refuse with a stated reason.
pub fn plan_cell(
    impl_arm: &str,
    suite_arm: &str,
    impl_files: &[Source],
    suite_files: &[Source],
    target: &str,
) -> Result<Plan, Refusal> {
    if impl_arm == suite_arm {
        return Err(Refusal::SameArm);
    }

    if !impl_files.iter().any(|s| s.path == target) {
        return Err(Refusal::NoImpl);
    }

    let suite_target = match suite_files.iter().find(|s| s.path == target) {
        Some(s) => s,
        None => return Err(Refusal::NoSuite),
    };

    if !suite_target.has_tests {
        return Err(Refusal::SuiteHasNoTests);
    }

    let mut take: Vec<String> = impl_files.iter().map(|s| s.path.clone()).collect();
    take.sort();
    take.dedup();

    let mut strip: Vec<String> = impl_files
        .iter()
        .filter(|s| s.has_tests)
        .map(|s| s.path.clone())
        .collect();
    strip.sort();
    strip.dedup();

    Ok(Plan {
        take,
        strip,
        append: target.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_arm_refusal_precedence_and_empty_lists() {
        // Clause 1: impl_arm == suite_arm is Err(SameArm), checked BEFORE file lists,
        // so it holds even when both lists are empty.
        let res = plan_cell("arm-a", "arm-a", &[], &[], "target.rs");
        assert_eq!(res, Err(Refusal::SameArm));

        // SameArm takes precedence over NoImpl even when target is missing in impl_files
        let res = plan_cell(
            "arm-a",
            "arm-a",
            &[],
            &[Source {
                path: "target.rs".to_string(),
                has_tests: true,
            }],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::SameArm));

        // SameArm takes precedence over NoSuite
        let res = plan_cell(
            "arm-a",
            "arm-a",
            &[Source {
                path: "target.rs".to_string(),
                has_tests: true,
            }],
            &[],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::SameArm));

        // SameArm takes precedence over SuiteHasNoTests
        let res = plan_cell(
            "arm-a",
            "arm-a",
            &[Source {
                path: "target.rs".to_string(),
                has_tests: true,
            }],
            &[Source {
                path: "target.rs".to_string(),
                has_tests: false,
            }],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::SameArm));
    }

    #[test]
    fn test_no_impl_refusal_precedence() {
        // Clause 2: No Source in impl_files whose path == target is Err(NoImpl).
        // Empty impl_files boundary
        let res = plan_cell(
            "arm-a",
            "arm-b",
            &[],
            &[Source {
                path: "target.rs".to_string(),
                has_tests: true,
            }],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::NoImpl));

        // NoImpl > NoSuite precedence: both lists empty or missing target
        let res = plan_cell("arm-a", "arm-b", &[], &[], "target.rs");
        assert_eq!(res, Err(Refusal::NoImpl));

        // NoImpl > SuiteHasNoTests precedence
        let res = plan_cell(
            "arm-a",
            "arm-b",
            &[Source {
                path: "other.rs".to_string(),
                has_tests: true,
            }],
            &[Source {
                path: "target.rs".to_string(),
                has_tests: false,
            }],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::NoImpl));
    }

    #[test]
    fn test_no_suite_refusal_precedence() {
        // Clause 3: No Source in suite_files whose path == target is Err(NoSuite).
        // Empty suite_files boundary
        let res = plan_cell(
            "arm-a",
            "arm-b",
            &[Source {
                path: "target.rs".to_string(),
                has_tests: true,
            }],
            &[],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::NoSuite));

        // Suite files contains other files, but not target
        let res = plan_cell(
            "arm-a",
            "arm-b",
            &[Source {
                path: "target.rs".to_string(),
                has_tests: true,
            }],
            &[Source {
                path: "other.rs".to_string(),
                has_tests: true,
            }],
            "target.rs",
        );
        assert_eq!(res, Err(Refusal::NoSuite));
    }

    #[test]
    fn test_clauses_4_and_7_suite_tests_and_append_coupling() {
        // Clause 4: suite arm's target file with has_tests == false is Err(SuiteHasNoTests).
        // Clause 7: append is target, taken from suite arm.
        // These two must be tested together: clause 4 refuses a suite with no tests,
        // and clause 7 pins whose copy of the file is appended. An implementation that appends
        // the IMPL arm's file passes clause 4 (since impl arm's target may have tests) while
        // producing a cell where an arm grades itself.
        let impl_files = [Source {
            path: "crates/target.rs".to_string(),
            has_tests: true,
        }];
        let suite_files_no_tests = [Source {
            path: "crates/target.rs".to_string(),
            has_tests: false,
        }];

        // Even though impl arm has tests on target, suite arm's target has none -> Refusal::SuiteHasNoTests
        let res = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files_no_tests,
            "crates/target.rs",
        );
        assert_eq!(res, Err(Refusal::SuiteHasNoTests));

        // When suite arm has tests on target, append must be the target path
        let suite_files_with_tests = [Source {
            path: "crates/target.rs".to_string(),
            has_tests: true,
        }];
        let res = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files_with_tests,
            "crates/target.rs",
        );
        let plan = res.expect("valid plan");
        assert_eq!(plan.append, "crates/target.rs");
    }

    #[test]
    fn test_smallest_whole_graft_single_target_file() {
        // Boundary: an impl arm with ONE file, the target, carrying tests:
        // take and strip are both that one path.
        let impl_files = [Source {
            path: "src/lib.rs".to_string(),
            has_tests: true,
        }];
        let suite_files = [Source {
            path: "src/lib.rs".to_string(),
            has_tests: true,
        }];

        let plan = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files,
            "src/lib.rs",
        )
        .expect("should produce plan");

        assert_eq!(plan.take, vec!["src/lib.rs".to_string()]);
        assert_eq!(plan.strip, vec!["src/lib.rs".to_string()]);
        assert_eq!(plan.take, plan.strip);
        assert_eq!(plan.append, "src/lib.rs");
    }

    #[test]
    fn test_every_impl_file_carrying_tests() {
        // Boundary: every impl file carrying tests: strip == take.
        let impl_files = [
            Source {
                path: "src/b.rs".to_string(),
                has_tests: true,
            },
            Source {
                path: "src/a.rs".to_string(),
                has_tests: true,
            },
            Source {
                path: "src/target.rs".to_string(),
                has_tests: true,
            },
        ];
        let suite_files = [Source {
            path: "src/target.rs".to_string(),
            has_tests: true,
        }];

        let plan = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files,
            "src/target.rs",
        )
        .expect("should produce plan");

        assert_eq!(plan.strip, plan.take);
        assert_eq!(
            plan.take,
            vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "src/target.rs".to_string()
            ]
        );
    }

    #[test]
    fn test_no_impl_file_carrying_tests() {
        // Boundary: no impl file carrying tests: strip is empty and take is not.
        // Target present in impl with has_tests == false.
        let impl_files = [
            Source {
                path: "src/b.rs".to_string(),
                has_tests: false,
            },
            Source {
                path: "src/target.rs".to_string(),
                has_tests: false,
            },
        ];
        let suite_files = [Source {
            path: "src/target.rs".to_string(),
            has_tests: true,
        }];

        let plan = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files,
            "src/target.rs",
        )
        .expect("should produce plan");

        assert!(plan.strip.is_empty());
        assert!(!plan.take.is_empty());
        assert_eq!(
            plan.take,
            vec!["src/b.rs".to_string(), "src/target.rs".to_string()]
        );
    }

    #[test]
    fn test_target_with_tests_in_both_take_and_strip() {
        // Boundary: the target present in impl_files with has_tests == true:
        // it is in BOTH take and strip.
        let impl_files = [
            Source {
                path: "src/target.rs".to_string(),
                has_tests: true,
            },
            Source {
                path: "src/helper.rs".to_string(),
                has_tests: false,
            },
        ];
        let suite_files = [Source {
            path: "src/target.rs".to_string(),
            has_tests: true,
        }];

        let plan = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files,
            "src/target.rs",
        )
        .expect("should produce plan");

        assert!(plan.take.contains(&"src/target.rs".to_string()));
        assert!(plan.strip.contains(&"src/target.rs".to_string()));
        assert!(!plan.strip.contains(&"src/helper.rs".to_string()));
    }

    #[test]
    fn test_take_and_strip_sorting_deduplication_and_subset_relationship() {
        // Clauses 5 & 6:
        // take contains every path in impl_files, sorted, with no duplicates.
        // strip contains exactly those impl_files whose has_tests is true, sorted,
        // and every element of strip is also in take.
        // Pin relationship as assertion over the plan.
        let impl_files = [
            Source {
                path: "z/file.rs".to_string(),
                has_tests: true,
            },
            Source {
                path: "a/file.rs".to_string(),
                has_tests: false,
            },
            Source {
                path: "m/target.rs".to_string(),
                has_tests: true,
            },
            Source {
                path: "a/file.rs".to_string(),
                has_tests: false,
            },
            Source {
                path: "z/file.rs".to_string(),
                has_tests: true,
            },
        ];
        let suite_files = [Source {
            path: "m/target.rs".to_string(),
            has_tests: true,
        }];

        let plan = plan_cell(
            "impl-arm",
            "suite-arm",
            &impl_files,
            &suite_files,
            "m/target.rs",
        )
        .expect("should produce plan");

        // Verify take is sorted and deduplicated
        assert_eq!(
            plan.take,
            vec![
                "a/file.rs".to_string(),
                "m/target.rs".to_string(),
                "z/file.rs".to_string()
            ]
        );

        // Verify strip is sorted and deduplicated
        assert_eq!(
            plan.strip,
            vec!["m/target.rs".to_string(), "z/file.rs".to_string()]
        );

        // Pin relationship: every element of strip is in take
        for s in &plan.strip {
            assert!(plan.take.contains(s), "strip element {s} must be in take");
        }

        // Composition invariants: no duplicates in take or strip
        for window in plan.take.windows(2) {
            assert_ne!(window[0], window[1], "duplicate in take: {}", window[0]);
        }
        for window in plan.strip.windows(2) {
            assert_ne!(window[0], window[1], "duplicate in strip: {}", window[0]);
        }
    }
}
