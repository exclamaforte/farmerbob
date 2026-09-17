//! Scoring gate: require positive evidence that work happened.
//!
//! A naive gate reads "nothing broke" as "it worked": `cargo test` on a crate
//! with no tests exits 0, so an arm that wrote nothing scores
//! `build=pass, test=pass`, indistinguishable from one that did the task. This
//! module closes that trap by distinguishing *absent* evidence (`None`, not
//! measured) from *negative* evidence (`Some(false)`, `Some(0)`).
//!
//! Pure logic only: no I/O, no process spawning. The caller runs the tools and
//! feeds the results in as an [`Observation`]; [`judge`] maps it to a
//! [`Verdict`] by the first applicable rule.

/// What the harness observed.
///
/// `None` means NOT MEASURED, which is never the same as zero. Collapsing the
/// two is the bug this module exists to prevent.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// Whether the code built. `None` means the build was never performed.
    pub built: Option<bool>,
    /// Whether the executed tests passed. `None` means no result was recorded.
    pub tests_passed: Option<bool>,
    /// Tests that actually EXECUTED. Zero is a real measurement; None means unknown.
    pub tests_run: Option<u32>,
    /// Lines added to the declared target. Zero is real; None means unknown.
    pub lines_added: Option<u32>,
    /// Files the run was declared to create that now exist.
    pub declared_targets_present: Option<bool>,
}

/// The gate's decision. Exactly these verdicts and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Positive evidence on every axis: built, tests passed, tests ran,
    /// lines written, declared deliverable present or unstated.
    Pass,
    /// Wrote nothing.
    NoOp,
    /// Wrote something that does not build.
    NoCompile,
    /// Builds, tests executed, some failed.
    TestsFail,
    /// Builds and "passes", but NOTHING executed. The empty-suite trap.
    NoTests,
    /// The declared deliverable is absent though lines were written elsewhere.
    WrongTarget,
    /// Not enough was measured to decide. Never a failure.
    Indeterminate,
}

impl Verdict {
    /// Only Pass. Everything else, including Indeterminate, is not a pass.
    pub fn is_pass(self) -> bool {
        matches!(self, Self::Pass)
    }

    /// True when the verdict reflects the ARM's work. Indeterminate does not:
    /// a gate that cannot see cannot fail an arm.
    pub fn blames_arm(self) -> bool {
        !matches!(self, Self::Indeterminate)
    }
}

/// Decide. Checked in this order, and the FIRST applicable verdict wins:
/// Indeterminate, NoOp, NoCompile, WrongTarget, TestsFail, NoTests, Pass.
///
/// A fully evidenced run falls through every rule to [`Verdict::Pass`]. A run
/// that falls through every rule but is missing [`Observation::lines_added`]
/// cannot meet the positive-evidence bar for `Pass`, so it resolves to
/// [`Verdict::Indeterminate`]: not enough was measured to decide.
pub fn judge(o: &Observation) -> Verdict {
    if o.built.is_none() || o.tests_passed.is_none() || o.tests_run.is_none() {
        return Verdict::Indeterminate;
    }
    if o.lines_added == Some(0) {
        return Verdict::NoOp;
    }
    if o.built == Some(false) {
        return Verdict::NoCompile;
    }
    let lines_positive = matches!(o.lines_added, Some(n) if n > 0);
    if o.declared_targets_present == Some(false) && lines_positive {
        return Verdict::WrongTarget;
    }
    if o.tests_passed == Some(false) {
        return Verdict::TestsFail;
    }
    if o.tests_run == Some(0) {
        return Verdict::NoTests;
    }
    if lines_positive && o.declared_targets_present != Some(false) {
        return Verdict::Pass;
    }
    Verdict::Indeterminate
}

/// Which fields were not measured. Empty means fully observed.
///
/// Returned in the declaration order of [`Observation`]: `built`,
/// `tests_passed`, `tests_run`, `lines_added`, `declared_targets_present`.
pub fn unmeasured(o: &Observation) -> Vec<&'static str> {
    let mut out = Vec::new();
    if o.built.is_none() {
        out.push("built");
    }
    if o.tests_passed.is_none() {
        out.push("tests_passed");
    }
    if o.tests_run.is_none() {
        out.push("tests_run");
    }
    if o.lines_added.is_none() {
        out.push("lines_added");
    }
    if o.declared_targets_present.is_none() {
        out.push("declared_targets_present");
    }
    out
}

/// A one-line explanation naming the deciding evidence, for a human reading a
/// run record.
///
/// It names the field that decided, so a wrong verdict can be traced to its
/// input. For [`Verdict::Pass`] it states the positive evidence.
pub fn explain(o: &Observation, v: Verdict) -> String {
    match v {
        Verdict::Indeterminate => {
            let missing = unmeasured(o);
            if missing.is_empty() {
                String::from(
                    "indeterminate: the measured fields decide no verdict, so pass-level evidence is missing",
                )
            } else {
                format!("indeterminate: unmeasured field(s): {}", missing.join(", "))
            }
        }
        Verdict::NoOp => format!(
            "noop: lines_added is 0, so nothing was written (built={}, tests_passed={})",
            show_bool(o.built),
            show_bool(o.tests_passed),
        ),
        Verdict::NoCompile => format!(
            "no_compile: built is false, so the written code does not build (lines_added={})",
            show_count(o.lines_added),
        ),
        Verdict::WrongTarget => format!(
            "wrong_target: declared_targets_present is false while lines_added is {}",
            show_count(o.lines_added),
        ),
        Verdict::TestsFail => format!(
            "tests_fail: tests_passed is false with tests_run at {}",
            show_count(o.tests_run),
        ),
        Verdict::NoTests => String::from(
            "no_tests: tests_run is 0, so a passing tests_passed proves nothing (empty suite)",
        ),
        Verdict::Pass => format!(
            "pass: built is true, tests_passed is true, tests_run is {} (> 0), lines_added is {} (> 0), declared_targets_present is {}",
            show_count(o.tests_run),
            show_count(o.lines_added),
            show_bool(o.declared_targets_present),
        ),
    }
}

/// Renders an optional boolean via matching, never via extraction.
fn show_bool(v: Option<bool>) -> &'static str {
    match v {
        None => "unmeasured",
        Some(true) => "true",
        Some(false) => "false",
    }
}

/// Renders an optional count via matching, never via extraction.
fn show_count(v: Option<u32>) -> String {
    match v {
        None => String::from("unmeasured"),
        Some(n) => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully evidenced passing run: every field measured, every bar met.
    fn passing() -> Observation {
        Observation {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(7),
            lines_added: Some(42),
            declared_targets_present: Some(true),
        }
    }

    #[test]
    fn indeterminate_when_core_evidence_unmeasured_and_checked_first() {
        // `built` unmeasured, yet everything else screams NoOp: the gate that
        // cannot see still cannot fail the arm.
        let blind_but_idle = Observation {
            built: None,
            tests_passed: Some(true),
            tests_run: Some(3),
            lines_added: Some(0),
            declared_targets_present: Some(true),
        };
        assert_eq!(judge(&blind_but_idle), Verdict::Indeterminate);

        for missing in [
            Observation {
                built: None,
                ..passing()
            },
            Observation {
                tests_passed: None,
                ..passing()
            },
            Observation {
                tests_run: None,
                ..passing()
            },
        ] {
            assert_eq!(judge(&missing), Verdict::Indeterminate);
        }
    }

    #[test]
    fn noop_when_nothing_written_regardless_of_build_or_tests() {
        // Even a green build with green tests is a NoOp when nothing was written.
        let idle = Observation {
            lines_added: Some(0),
            ..passing()
        };
        assert_eq!(judge(&idle), Verdict::NoOp);

        let idle_broken = Observation {
            built: Some(false),
            tests_passed: Some(false),
            tests_run: Some(5),
            lines_added: Some(0),
            declared_targets_present: Some(false),
        };
        assert_eq!(judge(&idle_broken), Verdict::NoOp);
    }

    #[test]
    fn noop_beats_nocompile_when_both_apply() {
        let neither_written_nor_building = Observation {
            built: Some(false),
            lines_added: Some(0),
            ..passing()
        };
        assert_eq!(judge(&neither_written_nor_building), Verdict::NoOp);
    }

    #[test]
    fn nocompile_when_build_fails() {
        let broken = Observation {
            built: Some(false),
            tests_passed: None,
            tests_run: None,
            lines_added: Some(0),
            declared_targets_present: None,
        };
        // Unmeasured core evidence takes precedence over the broken build.
        assert_eq!(judge(&broken), Verdict::Indeterminate);

        let broken_measured = Observation {
            built: Some(false),
            tests_passed: Some(false),
            tests_run: Some(4),
            lines_added: Some(9),
            declared_targets_present: None,
        };
        assert_eq!(judge(&broken_measured), Verdict::NoCompile);
    }

    #[test]
    fn wrong_target_when_declared_deliverable_absent_despite_lines_elsewhere() {
        let elsewhere = Observation {
            declared_targets_present: Some(false),
            ..passing()
        };
        assert_eq!(judge(&elsewhere), Verdict::WrongTarget);
    }

    #[test]
    fn wrong_target_needs_lines_written() {
        // No lines, absent target: NoOp (nothing written) wins over WrongTarget.
        let idle = Observation {
            lines_added: Some(0),
            declared_targets_present: Some(false),
            ..passing()
        };
        assert_eq!(judge(&idle), Verdict::NoOp);

        // Unknown lines, absent target: not WrongTarget, and never a Pass.
        let unknown_lines = Observation {
            lines_added: None,
            declared_targets_present: Some(false),
            ..passing()
        };
        assert_ne!(judge(&unknown_lines), Verdict::WrongTarget);
        assert_ne!(judge(&unknown_lines), Verdict::Pass);
    }

    #[test]
    fn none_target_does_not_block_pass() {
        // Many tasks declare no target at all.
        for target in [None, Some(true)] {
            let o = Observation {
                declared_targets_present: target,
                ..passing()
            };
            assert_eq!(judge(&o), Verdict::Pass);
        }
    }

    #[test]
    fn testsfail_when_executed_tests_fail() {
        let failing = Observation {
            tests_passed: Some(false),
            ..passing()
        };
        assert_eq!(judge(&failing), Verdict::TestsFail);
    }

    #[test]
    fn empty_suite_pass_is_no_tests_not_pass() {
        // The empty-suite trap: "passed" with nothing executed must never read as Pass.
        let empty = Observation {
            tests_run: Some(0),
            ..passing()
        };
        let v = judge(&empty);
        assert_eq!(v, Verdict::NoTests);
        assert!(!v.is_pass());
    }

    #[test]
    fn missing_tests_run_is_indeterminate_not_no_tests() {
        // Unknown execution count is absent evidence, not the measured zero above.
        let unknown = Observation {
            tests_run: None,
            ..passing()
        };
        assert_eq!(judge(&unknown), Verdict::Indeterminate);
    }

    #[test]
    fn tests_run_zero_vs_none_differ() {
        let zero = Observation {
            tests_run: Some(0),
            ..passing()
        };
        let none = Observation {
            tests_run: None,
            ..passing()
        };
        assert_ne!(judge(&zero), judge(&none));
        assert_eq!(judge(&zero), Verdict::NoTests);
        assert_eq!(judge(&none), Verdict::Indeterminate);
    }

    #[test]
    fn pass_requires_positive_evidence_on_every_axis() {
        assert_eq!(judge(&passing()), Verdict::Pass);

        // Each axis removed in turn must cost the Pass.
        let cases = [
            (
                Observation {
                    built: Some(false),
                    ..passing()
                },
                Verdict::NoCompile,
            ),
            (
                Observation {
                    tests_passed: Some(false),
                    ..passing()
                },
                Verdict::TestsFail,
            ),
            (
                Observation {
                    tests_run: Some(0),
                    ..passing()
                },
                Verdict::NoTests,
            ),
            (
                Observation {
                    lines_added: Some(0),
                    ..passing()
                },
                Verdict::NoOp,
            ),
            (
                Observation {
                    declared_targets_present: Some(false),
                    ..passing()
                },
                Verdict::WrongTarget,
            ),
        ];
        for (o, expected) in cases {
            assert_eq!(judge(&o), expected);
        }

        // Boundary: `lines_added` unknown with everything else green is not a
        // Pass — a zero and an unknown must never collapse.
        let unknown_lines = Observation {
            lines_added: None,
            ..passing()
        };
        assert_ne!(judge(&unknown_lines), Verdict::Pass);
        assert_eq!(judge(&unknown_lines), Verdict::Indeterminate);
    }

    #[test]
    fn first_applicable_verdict_wins_down_the_chain() {
        // Every later rule's trigger is held on while earlier rules are peeled
        // off one by one: each step proves the earlier rule shadows the later.
        let all_bad = Observation {
            built: Some(false),
            tests_passed: Some(false),
            tests_run: Some(0),
            lines_added: Some(0),
            declared_targets_present: Some(false),
        };
        assert_eq!(judge(&all_bad), Verdict::NoOp);

        let wrote = Observation {
            lines_added: Some(5),
            ..all_bad.clone()
        };
        assert_eq!(judge(&wrote), Verdict::NoCompile);

        let builds = Observation {
            built: Some(true),
            ..wrote.clone()
        };
        assert_eq!(judge(&builds), Verdict::WrongTarget);

        let on_target = Observation {
            declared_targets_present: Some(true),
            ..builds.clone()
        };
        assert_eq!(judge(&on_target), Verdict::TestsFail);

        let green = Observation {
            tests_passed: Some(true),
            ..on_target.clone()
        };
        assert_eq!(judge(&green), Verdict::NoTests);

        let executed = Observation {
            tests_run: Some(5),
            ..green.clone()
        };
        assert_eq!(judge(&executed), Verdict::Pass);
    }

    #[test]
    fn is_pass_only_for_pass() {
        assert!(Verdict::Pass.is_pass());
        for v in [
            Verdict::NoOp,
            Verdict::NoCompile,
            Verdict::TestsFail,
            Verdict::NoTests,
            Verdict::WrongTarget,
            Verdict::Indeterminate,
        ] {
            assert!(!v.is_pass());
        }
    }

    #[test]
    fn blames_arm_false_only_for_indeterminate() {
        assert!(!Verdict::Indeterminate.blames_arm());
        for v in [
            Verdict::Pass,
            Verdict::NoOp,
            Verdict::NoCompile,
            Verdict::TestsFail,
            Verdict::NoTests,
            Verdict::WrongTarget,
        ] {
            assert!(v.blames_arm(), "{v:?} should blame the arm");
        }
    }

    #[test]
    fn explain_names_deciding_field_and_stays_one_line() {
        let cases = [
            (
                Observation {
                    built: None,
                    ..passing()
                },
                Verdict::Indeterminate,
                "built",
            ),
            (
                Observation {
                    lines_added: Some(0),
                    ..passing()
                },
                Verdict::NoOp,
                "lines_added",
            ),
            (
                Observation {
                    built: Some(false),
                    tests_passed: Some(true),
                    tests_run: Some(2),
                    lines_added: Some(5),
                    declared_targets_present: None,
                },
                Verdict::NoCompile,
                "built",
            ),
            (
                Observation {
                    declared_targets_present: Some(false),
                    ..passing()
                },
                Verdict::WrongTarget,
                "declared_targets_present",
            ),
            (
                Observation {
                    tests_passed: Some(false),
                    ..passing()
                },
                Verdict::TestsFail,
                "tests_passed",
            ),
            (
                Observation {
                    tests_run: Some(0),
                    ..passing()
                },
                Verdict::NoTests,
                "tests_run",
            ),
            (passing(), Verdict::Pass, "tests_run"),
        ];
        for (o, v, field) in cases {
            assert_eq!(judge(&o), v);
            let text = explain(&o, v);
            assert!(
                text.contains(field),
                "{v:?} explanation should name {field}: {text}"
            );
            assert!(!text.contains('\n'), "explanation must be one line: {text}");
        }

        // A Pass explanation states positive evidence across the axes.
        let pass_text = explain(&passing(), Verdict::Pass);
        for field in [
            "built",
            "tests_passed",
            "tests_run",
            "lines_added",
            "declared_targets_present",
        ] {
            assert!(
                pass_text.contains(field),
                "pass explanation should state {field}: {pass_text}"
            );
        }
    }

    #[test]
    fn unmeasured_empty_for_fully_observed_run() {
        assert!(unmeasured(&passing()).is_empty());
    }

    #[test]
    fn unmeasured_lists_fields_in_declaration_order() {
        let o = Observation {
            built: None,
            tests_passed: Some(true),
            tests_run: None,
            lines_added: None,
            declared_targets_present: Some(true),
        };
        assert_eq!(unmeasured(&o), vec!["built", "tests_run", "lines_added"]);

        let all_missing = Observation {
            built: None,
            tests_passed: None,
            tests_run: None,
            lines_added: None,
            declared_targets_present: None,
        };
        assert_eq!(
            unmeasured(&all_missing),
            vec![
                "built",
                "tests_passed",
                "tests_run",
                "lines_added",
                "declared_targets_present"
            ]
        );
    }
}
