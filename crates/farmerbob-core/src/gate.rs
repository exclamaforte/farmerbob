//! Scoring gate: require positive evidence that work happened.
//!
//! A naive gate reads "nothing broke" as "it worked": `cargo test` on a crate
//! with no tests exits 0, so an arm that wrote nothing scores
//! `build=pass, test=pass`, indistinguishable from one that did the task. This
//! module closes that trap by distinguishing *absent* evidence (`None`, not
//! measured) from *negative* evidence (`Some(false)`, `Some(0)`).
//!
//! The same discipline governs scope: [`Observation::scope_departures`] is
//! `None` when scope was never assessed, and an unmeasured scope can never
//! underwrite a [`Verdict::Pass`] — a gate that measures a violation and then
//! returns `Pass` is worse than one that never measured, because the record
//! now carries evidence that the run was fine.
//!
//! Pure logic only: no I/O, no process spawning. The caller runs the tools and
//! feeds the results in as an [`Observation`]; [`judge`] maps it to a
//! [`Verdict`] by the first applicable rule.

/// What the harness observed.
///
/// `None` means NOT MEASURED, which is never the same as zero. Collapsing the
/// two is the bug this module exists to prevent.
///
/// `Default` gives `None` everywhere — every field unmeasured — and is a
/// starting point for construction, never a description of a run.
#[derive(Debug, Clone, PartialEq, Default)]
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
    /// Paths changed outside the declared deliverable, as counted by
    /// [`crate::scope::assess`]. `Some(0)` means measured and clean.
    /// `None` means scope was never assessed, which is NOT the same as clean.
    pub scope_departures: Option<u32>,
}

/// The gate's decision. Exactly these verdicts and no others — and the set is
/// closed on purpose: a verdict is a decision the harness makes, so every
/// caller can match it exhaustively. The refusal patterns in
/// [`crate::limit_signal`] are the opposite: an open subset of the ways a
/// provider can say no, grown as providers surprise us. Recognising an open
/// world and deciding a closed one are different jobs, and the types keep
/// them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Positive evidence on every axis: built, tests passed, tests ran,
    /// lines written, declared deliverable present or unstated, and scope
    /// measured clean.
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
    /// Built, tested and wrote code, but touched files outside its declared
    /// deliverable. A statement about what the run was PERMITTED to change,
    /// not about whether it can write working code.
    OutOfScope,
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
/// [`Verdict::OutOfScope`] is reached only where `Pass` was: it is the answer
/// to a fully evidenced run that also departed scope.
///
/// Scope precedence, decided here so it is not decided by accident:
/// [`Verdict::OutOfScope`] displaces [`Verdict::Pass`] and NOTHING ELSE. A run
/// that departed scope AND failed to build reports [`Verdict::NoCompile`]. A
/// run that departed scope and whose tests failed reports
/// [`Verdict::TestsFail`]. Those verdicts describe the code the arm wrote;
/// scope describes what it was allowed to touch, and when the code is already
/// unusable the more actionable fact wins. But a run that built, ran tests,
/// passed them, wrote lines, and departed scope reports
/// [`Verdict::OutOfScope`] and must never report [`Verdict::Pass`].
///
/// Unmeasured scope is not clean scope: `scope_departures` being `None`
/// resolves to [`Verdict::Indeterminate`]. Scope that was never assessed is
/// not scope that was clean, and a missing measurement must never be read as
/// a fine one. It blocks the `Pass` and nothing else — every verdict above it
/// was already decidable without it.
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
    if o.scope_departures.is_none() {
        return Verdict::Indeterminate;
    }
    if lines_positive && o.declared_targets_present != Some(false) {
        // OutOfScope displaces Pass and NOTHING ELSE.
        if o.scope_departures == Some(0) {
            return Verdict::Pass;
        }
        return Verdict::OutOfScope;
    }
    Verdict::Indeterminate
}

/// Which fields were not measured. Empty means fully observed.
///
/// Returned in the declaration order of [`Observation`]: `built`,
/// `tests_passed`, `tests_run`, `lines_added`, `declared_targets_present`,
/// `scope_departures`.
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
    if o.scope_departures.is_none() {
        out.push("scope_departures");
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
            "pass: built is true, tests_passed is true, tests_run is {} (> 0), lines_added is {} (> 0), declared_targets_present is {}, scope_departures is {}",
            show_count(o.tests_run),
            show_count(o.lines_added),
            show_bool(o.declared_targets_present),
            show_count(o.scope_departures),
        ),
        Verdict::OutOfScope => format!(
            "out_of_scope: scope_departures is {}, so the run changed files outside its declared deliverable",
            show_count(o.scope_departures),
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
            scope_departures: Some(0),
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
            scope_departures: Some(0),
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
            scope_departures: Some(0),
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
            scope_departures: None,
        };
        // Unmeasured core evidence takes precedence over the broken build.
        assert_eq!(judge(&broken), Verdict::Indeterminate);

        let broken_measured = Observation {
            built: Some(false),
            tests_passed: Some(false),
            tests_run: Some(4),
            lines_added: Some(9),
            declared_targets_present: None,
            scope_departures: Some(0),
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
            scope_departures: Some(0),
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
            Verdict::OutOfScope,
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
            Verdict::OutOfScope,
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
                    scope_departures: Some(0),
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
            scope_departures: Some(0),
        };
        assert_eq!(unmeasured(&o), vec!["built", "tests_run", "lines_added"]);

        let all_missing = Observation {
            built: None,
            tests_passed: None,
            tests_run: None,
            lines_added: None,
            declared_targets_present: None,
            scope_departures: None,
        };
        assert_eq!(
            unmeasured(&all_missing),
            vec![
                "built",
                "tests_passed",
                "tests_run",
                "lines_added",
                "declared_targets_present",
                "scope_departures"
            ]
        );
    }
}

/// The scope axis and the incident it closes: a run that left its declared
/// deliverable must not be able to read PASS, and scope that was never
/// measured must not be read as clean.
#[cfg(test)]
mod scope_gate {
    use super::*;

    /// Fully positive evidence, scope measured clean.
    fn passing() -> Observation {
        Observation {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(7),
            lines_added: Some(42),
            declared_targets_present: Some(true),
            scope_departures: Some(0),
        }
    }

    // Clause 1: measured and clean leaves Pass reachable.
    #[test]
    fn scope_measured_clean_leaves_pass_reachable() {
        assert_eq!(judge(&passing()), Verdict::Pass);
    }

    // Clause 2: one departure is a departure; there is no tolerance band.
    #[test]
    fn one_departure_is_a_departure_no_tolerance_band() {
        let o = Observation {
            scope_departures: Some(1),
            ..passing()
        };
        let v = judge(&o);
        assert_eq!(v, Verdict::OutOfScope);
        assert!(!v.is_pass());
    }

    // Clause 3: the real case — the run that rewrote two crates and read PASS.
    #[test]
    fn fifty_one_departures_still_out_of_scope() {
        let o = Observation {
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::OutOfScope);
    }

    // Clause 4, the clause that matters most: scope never assessed is not
    // scope assessed clean. It must not read as Pass, and it has not earned
    // OutOfScope either — a missing measurement decides nothing.
    #[test]
    fn scope_never_assessed_is_indeterminate_not_pass_not_out_of_scope() {
        let o = Observation {
            scope_departures: None,
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::Indeterminate);
        assert_ne!(judge(&o), Verdict::Pass);
        assert_ne!(judge(&o), Verdict::OutOfScope);
    }

    // Boundary: `None` scope alongside ANOTHER unmeasured field lands in
    // `Indeterminate` by either route — the verdict is the same either way,
    // so the test only pins that neither route reads as `Pass` or
    // `OutOfScope`.
    #[test]
    fn unmeasured_scope_with_another_unmeasured_field_stays_indeterminate() {
        let lines_unknown = Observation {
            scope_departures: None,
            lines_added: None,
            ..passing()
        };
        let v = judge(&lines_unknown);
        assert_eq!(v, Verdict::Indeterminate);
        assert_ne!(v, Verdict::Pass);
        assert_ne!(v, Verdict::OutOfScope);

        let build_unknown = Observation {
            scope_departures: None,
            built: None,
            ..passing()
        };
        assert_eq!(judge(&build_unknown), Verdict::Indeterminate);
    }

    // Boundary: an all-`None` Observation is `Indeterminate`, exactly as it
    // was before the scope field existed; adding a field must not change
    // that. `Default` is every field unmeasured.
    #[test]
    fn fully_unmeasured_observation_is_indeterminate() {
        assert_eq!(judge(&Observation::default()), Verdict::Indeterminate);
    }

    // Clause 5: the code's own verdict outranks scope when the code is
    // unusable.
    #[test]
    fn broken_build_outranks_scope() {
        let o = Observation {
            built: Some(false),
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::NoCompile);
    }

    // Clause 6: failing tests outrank scope.
    #[test]
    fn failing_tests_outrank_scope() {
        let o = Observation {
            tests_passed: Some(false),
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::TestsFail);
    }

    // Clause 7: an arm that wrote nothing to its target cannot have departed
    // scope in any interesting sense, and NoOp is the more informative
    // answer.
    #[test]
    fn noop_outranks_scope() {
        let o = Observation {
            lines_added: Some(0),
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::NoOp);
    }

    // Clause 8: the empty-suite trap still outranks scope.
    #[test]
    fn empty_suite_outranks_scope() {
        let o = Observation {
            tests_run: Some(0),
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::NoTests);
    }

    // The last member of the displacement family: WrongTarget describes the
    // code too, and OutOfScope displaces Pass and NOTHING ELSE.
    #[test]
    fn wrong_target_outranks_scope() {
        let o = Observation {
            declared_targets_present: Some(false),
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&o), Verdict::WrongTarget);
    }

    // Boundaries at N: the smallest departure, the real one, and the top of
    // the range all read the same. The count is measured, not graded, and
    // nothing is computed from it, so `u32::MAX` overflows nothing.
    #[test]
    fn departure_count_boundaries_all_read_out_of_scope() {
        for n in [1u32, 51, u32::MAX] {
            let o = Observation {
                scope_departures: Some(n),
                ..passing()
            };
            assert_eq!(judge(&o), Verdict::OutOfScope, "n = {n}");
        }
    }

    // Two compositions of already-pinned behaviours. `lines_added: None`
    // never reached `Pass` before scope existed, so with departures it still
    // cannot reach `OutOfScope` — scope only takes the `Pass`'s place. And
    // `declared_targets_present: None` never blocked `Pass`, so it does not
    // block `OutOfScope` either.
    #[test]
    fn scope_takes_only_the_place_pass_would_have_taken() {
        let lines_unknown = Observation {
            lines_added: None,
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&lines_unknown), Verdict::Indeterminate);

        let target_unstated = Observation {
            declared_targets_present: None,
            scope_departures: Some(51),
            ..passing()
        };
        assert_eq!(judge(&target_unstated), Verdict::OutOfScope);
    }

    // Clause 9, directly: OutOfScope is a failure, and the arm owns it.
    #[test]
    fn out_of_scope_is_not_a_pass_and_blames_the_arm() {
        assert!(!Verdict::OutOfScope.is_pass());
        assert!(Verdict::OutOfScope.blames_arm());
    }

    // The explanation names the deciding field and stays one line, like
    // every other verdict's.
    #[test]
    fn out_of_scope_explanation_names_scope_and_stays_one_line() {
        let o = Observation {
            scope_departures: Some(51),
            ..passing()
        };
        let text = explain(&o, Verdict::OutOfScope);
        assert!(text.contains("scope_departures"), "{text}");
        assert!(!text.contains('\n'), "{text}");
    }
}

// ESCALATED from cross-examination: or-muse-spark's suite discriminated on gate.
// Not a CLAIM -- cross-examination found it directly. Kept only because it passes
// against the merged winner, which is what separates a discovery from an
// over-fitted suite.
#[cfg(test)]
mod cx_gate_or_muse_spark {
    use super::*;

    /// A fully evidenced passing run: every field measured, every bar met.
    fn passing() -> Observation {
        Observation {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(7),
            lines_added: Some(42),
            declared_targets_present: Some(true),
            scope_departures: Some(0),
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
            scope_departures: Some(0),
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
            scope_departures: Some(0),
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
            scope_departures: None,
        };
        // Unmeasured core evidence takes precedence over the broken build.
        assert_eq!(judge(&broken), Verdict::Indeterminate);

        let broken_measured = Observation {
            built: Some(false),
            tests_passed: Some(false),
            tests_run: Some(4),
            lines_added: Some(9),
            declared_targets_present: None,
            scope_departures: Some(0),
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
            scope_departures: Some(0),
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
            Verdict::OutOfScope,
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
            Verdict::OutOfScope,
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
                    scope_departures: Some(0),
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
            scope_departures: Some(0),
        };
        assert_eq!(unmeasured(&o), vec!["built", "tests_run", "lines_added"]);

        let all_missing = Observation {
            built: None,
            tests_passed: None,
            tests_run: None,
            lines_added: None,
            declared_targets_present: None,
            scope_departures: None,
        };
        assert_eq!(
            unmeasured(&all_missing),
            vec![
                "built",
                "tests_passed",
                "tests_run",
                "lines_added",
                "declared_targets_present",
                "scope_departures"
            ]
        );
    }
}
