//! Deciding the fate of confirmed critique findings.
//!
//! Escalation turns a confirmed critique finding into a permanent regression test.
//! It works, and it has one critical hole: when a critic identifies a real defect
//! in the arm that goes on to WIN, escalation writes the test, runs it against the
//! merged reference, watches it fail (because the defect is real and unfixed), and
//! retracts it:
//!
//! ```text
//! glm-53-flash: escalated 1 finding(s) from gemini-38-flash
//! glm-53-flash: VETOED against the merged reference -- retracting
//! ```
//!
//! The veto correctly refuses to admit a failing test into a green test suite.
//! However, the finding was ACCEPTED by the adjudicator, is real, and subsequently
//! exists nowhere except a hand-written bead or note. The better the critic, the
//! more likely its finding concerns the winner, and the more certain it is to vanish.
//!
//! This module decides what should happen to a finding instead of leaving it to
//! whoever is reading.
//!
//! # Totality and Decision Table
//!
//! [`decide`] is total: every one of the six combinations of `(subject_merged, veto)`
//! has a defined answer. There are no invalid inputs, no error cases, and no [`Option`].
//!
//! | `subject_merged` | `veto` | Outcome ([`Fate`]) | Rationale |
//! |---|---|---|---|
//! | `false` | [`VetoRun::Passed`] | [`Fate::NoSubject`] | Subject lost; test against unmerged code establishes nothing about the tree. |
//! | `false` | [`VetoRun::Failed`] | [`Fate::NoSubject`] | Subject lost; unmerged code defect is not in the merged tree. |
//! | `false` | [`VetoRun::DidNotRun`] | [`Fate::NoSubject`] | Subject lost; Clause 1 has no exceptions, outcome is `NoSubject` regardless of veto run. |
//! | `true` | [`VetoRun::Passed`] | [`Fate::Escalate`] | Subject won and test passes against merged code: defect was fixed or never applied; escalate as regression guard. |
//! | `true` | [`VetoRun::Failed`] | [`Fate::FileAsKnownDefect`] | Subject won and test fails against merged code: defect is real and shipped; file as known defect rather than dropping or failing the suite. |
//! | `true` | [`VetoRun::DidNotRun`] | [`Fate::Unverifiable`] | Subject won but test could not run: established nothing, so neither escalated nor filed as known defect. |
//!
//! # Closed Enums
//!
//! [`Fate`] is closed at four variants and [`VetoRun`] is closed at three.
//! There is deliberately no `Fate::Ignore`: a confirmed finding that an adjudicator
//! accepted must land somewhere, and a variant meaning "drop it" would restore
//! exactly the silent loss this module is designed to eliminate.
//!
//! # Survival
//!
//! [`survives`] identifies whether a fate preserves the finding somewhere visible
//! to future readers. It returns `true` for [`Fate::Escalate`] and [`Fate::FileAsKnownDefect`].
//! It returns `false` for [`Fate::NoSubject`] and [`Fate::Unverifiable`] -- those two
//! are where findings are lost today, and this module names them explicitly so that
//! loss is visible rather than silent.

/// Where a confirmed finding should end up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// The subject lost. Its code is not merged, so a test has nothing to
    /// run against and the finding is recorded against the critic's credit
    /// and nothing more.
    NoSubject,
    /// The subject won and the test PASSES against the merged code: the
    /// defect was fixed on the way in, or never applied to the merged
    /// version. Escalate it -- it is a regression guard.
    Escalate,
    /// The subject won and the test FAILS against the merged code: the
    /// defect is real and shipped. Do not put a failing test in the suite;
    /// file it as known-broken so it survives.
    FileAsKnownDefect {
        /// Why it cannot be escalated, for the bead body. Never empty.
        reason: String,
    },
    /// The test could not be run at all, so nothing is known. NOT the same
    /// as a test that failed.
    Unverifiable {
        /// What stopped it, for a human. Never empty.
        reason: String,
    },
}

/// What happened when the escalated test ran against the merged reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VetoRun {
    /// It ran and passed.
    Passed,
    /// It ran and failed.
    Failed,
    /// It did not run: no tests matched, the build broke, nothing executed.
    DidNotRun,
}

/// Decide a finding's fate.
///
/// `subject_merged` is whether the arm the finding is ABOUT is the one that
/// was merged.
///
/// This function is total: every input combination maps to a defined [`Fate`].
///
/// | `subject_merged` | `veto` | Outcome |
/// |---|---|---|
/// | `false` | [`VetoRun::Passed`] | [`Fate::NoSubject`] |
/// | `false` | [`VetoRun::Failed`] | [`Fate::NoSubject`] |
/// | `false` | [`VetoRun::DidNotRun`] | [`Fate::NoSubject`] |
/// | `true` | [`VetoRun::Passed`] | [`Fate::Escalate`] |
/// | `true` | [`VetoRun::Failed`] | [`Fate::FileAsKnownDefect`] |
/// | `true` | [`VetoRun::DidNotRun`] | [`Fate::Unverifiable`] |
///
/// Every `reason` produced in [`Fate::FileAsKnownDefect`] and [`Fate::Unverifiable`]
/// is non-empty by construction: string literals describing the outcome are assigned
/// directly, rather than accepting unbounded caller input.
pub fn decide(subject_merged: bool, veto: VetoRun) -> Fate {
    match (subject_merged, veto) {
        (false, _) => Fate::NoSubject,
        (true, VetoRun::Passed) => Fate::Escalate,
        (true, VetoRun::Failed) => Fate::FileAsKnownDefect {
            reason: "test fails against merged code".to_string(),
        },
        (true, VetoRun::DidNotRun) => Fate::Unverifiable {
            reason: "test did not run against merged code".to_string(),
        },
    }
}

/// Whether this fate means the finding survives somewhere a later reader
/// will see it.
///
/// True for [`Fate::Escalate`] and [`Fate::FileAsKnownDefect`]. False for
/// [`Fate::NoSubject`] and [`Fate::Unverifiable`] -- those two are where
/// findings are lost today, and this module names them so the loss is visible
/// rather than silent.
pub fn survives(f: &Fate) -> bool {
    match f {
        Fate::Escalate | Fate::FileAsKnownDefect { .. } => true,
        Fate::NoSubject | Fate::Unverifiable { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Explicit test for each cell in the 2x3 input space.
    #[test]
    fn cell1_unmerged_passed_is_no_subject() {
        assert_eq!(decide(false, VetoRun::Passed), Fate::NoSubject);
    }

    #[test]
    fn cell2_unmerged_failed_is_no_subject() {
        assert_eq!(decide(false, VetoRun::Failed), Fate::NoSubject);
    }

    #[test]
    fn cell3_unmerged_did_not_run_is_no_subject() {
        assert_eq!(decide(false, VetoRun::DidNotRun), Fate::NoSubject);
    }

    #[test]
    fn cell4_merged_passed_is_escalate() {
        assert_eq!(decide(true, VetoRun::Passed), Fate::Escalate);
    }

    #[test]
    fn cell5_merged_failed_is_file_as_known_defect() {
        match decide(true, VetoRun::Failed) {
            Fate::FileAsKnownDefect { reason } => {
                assert!(!reason.is_empty(), "reason must be non-empty");
            }
            other => panic!("expected FileAsKnownDefect, got {other:?}"),
        }
    }

    #[test]
    fn cell6_merged_did_not_run_is_unverifiable() {
        match decide(true, VetoRun::DidNotRun) {
            Fate::Unverifiable { reason } => {
                assert!(!reason.is_empty(), "reason must be non-empty");
            }
            other => panic!("expected Unverifiable, got {other:?}"),
        }
    }

    // Clause 1: subject_merged == false is NoSubject, whatever veto says.
    // A test against code that is not in the tree establishes nothing about the tree.
    #[test]
    fn clause1_unmerged_subject_is_always_no_subject() {
        let veto_runs = [VetoRun::Passed, VetoRun::Failed, VetoRun::DidNotRun];
        for veto in veto_runs {
            assert_eq!(
                decide(false, veto),
                Fate::NoSubject,
                "unmerged subject must yield NoSubject regardless of veto: {veto:?}"
            );
        }
    }

    // Clause 2: subject_merged == true with VetoRun::Passed is Escalate.
    #[test]
    fn clause2_merged_subject_with_passed_veto_is_escalate() {
        assert_eq!(decide(true, VetoRun::Passed), Fate::Escalate);
    }

    // Clause 3: subject_merged == true with VetoRun::Failed is FileAsKnownDefect,
    // never Escalate and never silently dropped. Its reason is non-empty and
    // says the test fails against merged code.
    #[test]
    fn clause3_merged_subject_with_failed_veto_files_as_known_defect() {
        let fate = decide(true, VetoRun::Failed);

        // Never Escalate, never NoSubject, never Unverifiable
        assert_ne!(fate, Fate::Escalate);
        assert_ne!(fate, Fate::NoSubject);
        assert!(!matches!(fate, Fate::Unverifiable { .. }));

        match fate {
            Fate::FileAsKnownDefect { ref reason } => {
                assert!(!reason.is_empty(), "reason must not be empty");
                let lower = reason.to_lowercase();
                assert!(
                    lower.contains("fail") || lower.contains("merged") || lower.contains("defect"),
                    "reason should state that test fails against merged code: {reason}"
                );
            }
            other => panic!("expected FileAsKnownDefect, got {other:?}"),
        }
    }

    // Clause 4: VetoRun::DidNotRun is Unverifiable when the subject was merged.
    // Pin that it is NOT Escalate and NOT FileAsKnownDefect: a test that did not
    // run has established nothing, and both of those assert something.
    #[test]
    fn clause4_merged_subject_with_did_not_run_veto_is_unverifiable() {
        let fate = decide(true, VetoRun::DidNotRun);

        assert_ne!(fate, Fate::Escalate);
        assert_ne!(fate, Fate::NoSubject);
        assert!(!matches!(fate, Fate::FileAsKnownDefect { .. }));

        match fate {
            Fate::Unverifiable { ref reason } => {
                assert!(!reason.is_empty(), "reason must not be empty");
            }
            other => panic!("expected Unverifiable, got {other:?}"),
        }
    }

    // Clause 5: VetoRun::DidNotRun with subject_merged == false is NoSubject.
    // Clause 1 has no exceptions, and which of the two reasons applies does
    // not change the outcome.
    #[test]
    fn clause5_unmerged_subject_with_did_not_run_veto_is_no_subject() {
        let fate = decide(false, VetoRun::DidNotRun);
        assert_eq!(
            fate,
            Fate::NoSubject,
            "Clause 1 has no exceptions: unmerged subject with DidNotRun must be NoSubject, not Unverifiable"
        );
        assert!(!matches!(fate, Fate::Unverifiable { .. }));
    }

    // Clause 6: survives is true for exactly Escalate and FileAsKnownDefect.
    // Pin all four variants explicitly.
    #[test]
    fn clause6_survives_pins_all_four_variants() {
        assert!(
            survives(&Fate::Escalate),
            "Fate::Escalate survives as a regression test"
        );
        assert!(
            survives(&Fate::FileAsKnownDefect {
                reason: "fails against merged reference".into(),
            }),
            "Fate::FileAsKnownDefect survives as a documented defect"
        );
        assert!(
            !survives(&Fate::NoSubject),
            "Fate::NoSubject does not survive"
        );
        assert!(
            !survives(&Fate::Unverifiable {
                reason: "build broke".into(),
            }),
            "Fate::Unverifiable does not survive"
        );
    }

    // Clause 7: Every reason this module produces is non-empty.
    #[test]
    fn clause7_all_produced_reasons_are_non_empty() {
        if let Fate::FileAsKnownDefect { reason } = decide(true, VetoRun::Failed) {
            assert!(
                !reason.trim().is_empty(),
                "FileAsKnownDefect reason must not be whitespace or empty"
            );
        } else {
            panic!("expected FileAsKnownDefect");
        }

        if let Fate::Unverifiable { reason } = decide(true, VetoRun::DidNotRun) {
            assert!(
                !reason.trim().is_empty(),
                "Unverifiable reason must not be whitespace or empty"
            );
        } else {
            panic!("expected Unverifiable");
        }
    }

    // Comprehensive matrix test verifying survives matches expected outcome across all inputs.
    #[test]
    fn matrix_of_decide_and_survives() {
        let cases = [
            (false, VetoRun::Passed, false),
            (false, VetoRun::Failed, false),
            (false, VetoRun::DidNotRun, false),
            (true, VetoRun::Passed, true),
            (true, VetoRun::Failed, true),
            (true, VetoRun::DidNotRun, false),
        ];

        for (subject_merged, veto, expected_survives) in cases {
            let fate = decide(subject_merged, veto);
            assert_eq!(
                survives(&fate),
                expected_survives,
                "survives mismatch for subject_merged={subject_merged}, veto={veto:?}"
            );
        }
    }

    // Verifying traits (Clone, PartialEq, Eq, Debug, Copy) work as declared.
    #[test]
    fn enum_derives_and_traits() {
        let v1 = VetoRun::Passed;
        let v2 = v1; // Copy
        assert_eq!(v1, v2);
        assert_eq!(format!("{v1:?}"), "Passed");

        let f1 = Fate::FileAsKnownDefect {
            reason: "broken".into(),
        };
        let f2 = f1.clone();
        assert_eq!(f1, f2);
        assert_eq!(
            format!("{f1:?}"),
            "FileAsKnownDefect { reason: \"broken\" }"
        );
    }
}
