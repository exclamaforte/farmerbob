//! Pure adjudication of measured candidate evidence.

/// One candidate's objective record. `None` means not measured, which is never
/// the same as a measured zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    /// Candidate identifier.
    pub arm: String,
    /// Fraction of a hidden conformance suite passed, 0.0..=1.0.
    pub conformance: Option<f64>,
    /// Fraction of injected defects this candidate's own tests caught.
    pub defect_sensitivity: Option<f64>,
    /// Fraction of other arms' suites this implementation survives.
    pub survival: Option<f64>,
    /// Number of clippy diagnostics.
    pub clippy: Option<u32>,
    /// Crates modified. The task names one; more is scope creep.
    pub crates_touched: Option<u32>,
    /// Number of tests.
    pub tests: Option<u32>,
    /// Number of changed lines.
    pub lines: Option<u32>,
    /// Measured USD. `Some(0.0)` for a plan-based arm is a real zero.
    /// Measured dollar cost. RECORDED, never RANKED: no [`Criterion`] reads this, and the
    /// adjudicator sees it as context rather than as a tiebreak. See the note on
    /// [`Criterion`] for why cost must not decide which patch is merged.
    pub cost_usd: Option<f64>,
    /// True when cross-examination labelled this candidate's suite OVER-FITTED: it fails
    /// nearly every rival, so its failures carry no evidence about anyone. A count of tests
    /// from such a suite is not a measure of depth, it is a measure of how specifically the
    /// author wrote to their own implementation -- so TestDepth must not reward it.
    pub suite_overfitted: Option<bool>,
}

/// Which measurement decided it, in the rubric's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterion {
    /// Hidden-suite conformance.
    Conformance,
    /// Number of crates touched.
    ScopeDiscipline,
    /// Injected-defect detection rate.
    DefectSensitivity,
    /// Survival against other suites.
    Survival,
    /// Clippy diagnostic count.
    Clippy,
    // NO `Cost` CRITERION. Dollar cost was the sixth of eight criteria here, and it is a
    // category error: cost is a property of the ARM that produced a patch, never of the
    // patch. Two implementations that tie on conformance, scope, defect sensitivity,
    // survival and lint are equally good artefacts, and which one burned fewer tokens says
    // nothing about which file belongs in the crate.
    //
    // Worse than merely irrelevant, it closed a loop. Cost already lives on the Pareto board
    // and in the bandit's posterior, which answer a different question -- which arm to
    // dispatch next time. Letting it also decide merges means a cheap arm's work is merged
    // more often, which then feeds back as evidence that the cheap arm succeeds more often.
    // That manufactures the very result this project exists to measure honestly.
    //
    // A tie on quality is not a reason to reach for a different KIND of fact. The honest
    // outcomes are Simplicity, which is at least a property of the patch, or Undecided.
    //   (bead farmerbob-tcv5)
    /// Number of tests the candidate wrote. A PROXY for DefectSensitivity, consulted ONLY
    /// when both direct measures of suite quality -- DefectSensitivity and Survival -- are
    /// unmeasured for the whole field. A proxy must never outrank the thing it proxies for,
    /// but an ABSENT measurement must not silently promote Simplicity either: discarding the
    /// proxy entirely made every consensus-matrix task fall through to "fewest lines", which
    /// systematically favours the candidate that also wrote the fewest tests.
    TestDepth,
    /// Changed-line count.
    Simplicity,
}

/// The result of adjudicating a set of candidates.
#[derive(Debug, Clone, PartialEq)]
/// Named `Ruling`, not `Verdict`: [`crate::gate::Verdict`] is the gate's decision
/// about one run, and this is the adjudicator's decision about a FIELD of runs. The
/// two shared a name for 44 modules and are not the same thing.
pub enum Ruling {
    /// One candidate led on the named criterion.
    Winner {
        arm: String,
        on: Criterion,
        margin: f64,
    },
    /// No candidate led uniquely; `next` identifies unavailable measurements.
    Undecided {
        tied: Vec<String>,
        next: Vec<Criterion>,
    },
    /// Nothing passed the eligibility gate.
    NoCandidate,
}

/// The criteria, in the order they are consulted. Defined once: two copies of this list
/// drifted apart when Cost was removed from one of them.
const ORDERED_CRITERIA: [Criterion; 7] = [
    Criterion::Conformance,
    Criterion::ScopeDiscipline,
    Criterion::DefectSensitivity,
    Criterion::Survival,
    Criterion::Clippy,
    Criterion::TestDepth,
    Criterion::Simplicity,
];

/// Apply the criteria in order, stopping at the first criterion that produces
/// a unique leader among the candidates still in contention.
pub fn adjudicate(candidates: &[Evidence], epsilon: f64) -> Ruling {
    let mut contenders = eligible(candidates);
    if contenders.is_empty() {
        return Ruling::NoCandidate;
    }

    let epsilon = if epsilon.is_finite() && epsilon > 0.0 {
        epsilon
    } else {
        0.0
    };
    if contenders.len() == 1 {
        let candidate = contenders[0];
        return Ruling::Winner {
            arm: candidate.arm.clone(),
            on: Criterion::Conformance,
            margin: 1.0,
        };
    }

    let criteria = ORDERED_CRITERIA;
    let mut next = Vec::new();

    // TestDepth is a fallback, not a peer. It is consulted only when neither direct measure
    // of suite quality was obtained for ANY contender; if even one has a real measurement,
    // the proxy stays out of the ordering entirely.
    // ALL, not ANY. A criterion is only applied when every contender has a value for it, so
    // a field where one arm's survival is unmeasured SKIPS Survival -- and gating the proxy
    // on `any` then suppressed TestDepth as well, leaving Simplicity to decide. That is how
    // four adjudications came out as "fewest lines" on fields that had a real quality signal
    // for three of their four candidates.
    // Only DefectSensitivity suppresses it, NOT Survival. They measure different things:
    // Survival is a property of the IMPLEMENTATION (did rival suites break it), while
    // DefectSensitivity is a property of the SUITE (does it catch injected defects). TestDepth
    // proxies for the latter. Gating on Survival suppressed the proxy with a number that says
    // nothing about suite quality, and on resume that let a three-way survival tie fall
    // through to Simplicity -- selecting an 8-test candidate over a 12-test one on length.
    let direct_quality_measured = !contenders.is_empty()
        && contenders
            .iter()
            .all(|c| c.defect_sensitivity.is_some_and(f64::is_finite));

    for criterion in criteria {
        if criterion == Criterion::TestDepth && direct_quality_measured {
            continue;
        }
        let values: Option<Vec<f64>> = contenders
            .iter()
            .map(|candidate| value(candidate, criterion))
            .collect();
        let Some(values) = values else {
            next.push(criterion);
            continue;
        };

        let higher_is_better = matches!(
            criterion,
            Criterion::Conformance
                | Criterion::DefectSensitivity
                | Criterion::Survival
                | Criterion::TestDepth
        );
        let best = values.iter().copied().fold(values[0], |best, current| {
            if (higher_is_better && current > best) || (!higher_is_better && current < best) {
                current
            } else {
                best
            }
        });
        let leaders: Vec<usize> = values
            .iter()
            .enumerate()
            .filter(|(_, candidate)| (best - **candidate).abs() <= epsilon)
            .map(|(index, _)| index)
            .collect();

        if leaders.len() == 1 {
            let winner_index = leaders[0];
            let winner = contenders[winner_index];
            let margin = values
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != winner_index)
                .map(|(_, other)| {
                    if higher_is_better {
                        best - *other
                    } else {
                        *other - best
                    }
                })
                .fold(0.0_f64, f64::max);
            return Ruling::Winner {
                arm: winner.arm.clone(),
                on: criterion,
                margin,
            };
        }

        contenders = leaders.into_iter().map(|index| contenders[index]).collect();
    }

    let mut tied: Vec<String> = contenders
        .iter()
        .map(|candidate| candidate.arm.clone())
        .collect();
    tied.sort();
    Ruling::Undecided { tied, next }
}

/// Return candidates that have full conformance and ran at least one test.
pub fn eligible(candidates: &[Evidence]) -> Vec<&Evidence> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.conformance == Some(1.0) && candidate.tests.is_some_and(|tests| tests >= 1)
        })
        .collect()
}

/// Return criteria measured for some, but not all, input candidates.
/// A criterion the field cannot currently be ranked on, and WHY.
///
/// The distinction is the point. `Partial` means the instrument ran and some candidates have
/// a value: gather the rest. `Absent` means NO candidate has one, which usually means the
/// instrument was never run at all — and that is a fact about the harness, not about this
/// field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// Some candidates are measured, some are not.
    Partial(Criterion),
    /// No candidate is measured. The instrument may never have run.
    Absent(Criterion),
}

impl Gap {
    /// The criterion this gap concerns.
    pub fn criterion(self) -> Criterion {
        match self {
            Gap::Partial(c) | Gap::Absent(c) => c,
        }
    }
}

/// Criteria on which the field cannot be separated, classified by why.
///
/// This used to filter with `measured > 0 && measured < len`, which silently dropped every
/// criterion measured for NOBODY — so the report of what evidence was missing omitted the
/// evidence that was most missing. DefectSensitivity is the direct measure of suite quality
/// and sits third in the ordering, above Survival; it has never been measured for any task in
/// this project, and this function never once said so. "No candidate was measured" read
/// exactly like "this criterion does not apply here".
///   (bead farmerbob-jd2.11)
pub fn evidence_gaps(candidates: &[Evidence]) -> Vec<Gap> {
    if candidates.is_empty() {
        return Vec::new();
    }
    ORDERED_CRITERIA
        .into_iter()
        .filter_map(|criterion| {
            let measured = candidates
                .iter()
                .filter(|candidate| value(candidate, criterion).is_some())
                .count();
            if measured == 0 {
                Some(Gap::Absent(criterion))
            } else if measured < candidates.len() {
                Some(Gap::Partial(criterion))
            } else {
                None
            }
        })
        .collect()
}

/// Criteria only partially measured. Kept for callers that want the old, narrower question;
/// prefer [`evidence_gaps`], which also reports what was never measured at all.
pub fn missing_evidence(candidates: &[Evidence]) -> Vec<Criterion> {
    ORDERED_CRITERIA
        .into_iter()
        .filter(|criterion| {
            let measured = candidates
                .iter()
                .filter(|candidate| value(candidate, *criterion).is_some())
                .count();
            measured > 0 && measured < candidates.len()
        })
        .collect()
}

fn value(candidate: &Evidence, criterion: Criterion) -> Option<f64> {
    let value = match criterion {
        Criterion::Conformance => candidate.conformance,
        Criterion::ScopeDiscipline => candidate.crates_touched.map(f64::from),
        Criterion::DefectSensitivity => candidate.defect_sensitivity,
        Criterion::Survival => candidate.survival,
        Criterion::Clippy => candidate.clippy.map(f64::from),
        // An over-fitted suite scores ZERO depth, not "unmeasured". Returning None made the
        // whole criterion skip, so a single over-fitted candidate removed depth-ranking for
        // the entire field and handed the decision to Simplicity -- which is how resume
        // regressed to picking the shortest implementation. Zero is the honest reading: the
        // suite's count is real but worthless as evidence of depth, because a suite that
        // fails nearly every rival is measuring its author's implementation, not the spec.
        Criterion::TestDepth => {
            if candidate.suite_overfitted == Some(true) {
                Some(0.0)
            } else {
                candidate.tests.map(f64::from)
            }
        }
        Criterion::Simplicity => candidate.lines.map(f64::from),
    }?;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(arm: &str) -> Evidence {
        Evidence {
            arm: arm.to_string(),
            conformance: Some(1.0),
            defect_sensitivity: Some(0.5),
            survival: Some(0.5),
            clippy: Some(0),
            crates_touched: Some(1),
            tests: Some(1),
            lines: Some(10),
            cost_usd: Some(1.0),
            suite_overfitted: None,
        }
    }

    #[test]
    fn conformance_difference_decides_before_scope_is_consulted() {
        let mut rejected = evidence("rejected");
        rejected.conformance = Some(0.9);
        rejected.crates_touched = Some(1);
        let mut accepted = evidence("accepted");
        accepted.crates_touched = Some(9);
        assert!(
            matches!(adjudicate(&[rejected, accepted], 0.001), Ruling::Winner {
            arm, on: Criterion::Conformance, ..
        } if arm == "accepted")
        );
    }

    #[test]
    fn scope_discipline_beats_a_better_defect_sensitivity() {
        let mut narrow = evidence("narrow");
        narrow.crates_touched = Some(1);
        narrow.defect_sensitivity = Some(0.1);
        let mut broad = evidence("broad");
        broad.crates_touched = Some(2);
        broad.defect_sensitivity = Some(0.9);
        assert!(
            matches!(adjudicate(&[narrow, broad], 0.001), Ruling::Winner {
            arm, on: Criterion::ScopeDiscipline, ..
        } if arm == "narrow")
        );
    }

    #[test]
    fn none_in_one_contender_skips_criterion_and_lists_it_next() {
        let mut first = evidence("first");
        first.defect_sensitivity = None;
        let mut second = evidence("second");
        second.defect_sensitivity = None;
        first.conformance = Some(1.0);
        second.conformance = Some(1.0);
        let result = adjudicate(&[first, second], 0.001);
        assert!(matches!(result, Ruling::Undecided { next, .. }
            if next == vec![Criterion::DefectSensitivity]));
    }

    #[test]
    fn higher_test_count_does_not_win() {
        let mut fewer = evidence("fewer");
        fewer.tests = Some(1);
        let mut more = evidence("more");
        more.tests = Some(100);
        assert!(matches!(
            adjudicate(&[fewer, more], 0.001),
            Ruling::Undecided { .. }
        ));
    }

    #[test]
    fn all_tied_field_is_undecided_with_names_sorted() {
        let result = adjudicate(&[evidence("zeta"), evidence("alpha")], 0.001);
        assert_eq!(
            result,
            Ruling::Undecided {
                tied: vec!["alpha".to_string(), "zeta".to_string()],
                next: vec![]
            }
        );
    }

    #[test]
    fn margin_is_positive() {
        let mut winner = evidence("winner");
        winner.conformance = Some(1.0);
        let mut loser = evidence("loser");
        loser.conformance = Some(0.8);
        assert!(
            matches!(adjudicate(&[winner, loser], 0.001), Ruling::Winner { margin, .. } if margin > 0.0)
        );
    }

    #[test]
    fn nan_does_not_win() {
        let mut nan = evidence("nan");
        nan.conformance = Some(f64::NAN);
        let valid = evidence("valid");
        assert!(
            matches!(adjudicate(&[nan, valid], 0.001), Ruling::Winner { arm, .. } if arm == "valid")
        );
    }

    #[test]
    fn empty_field_is_no_candidate() {
        assert_eq!(adjudicate(&[], 0.001), Ruling::NoCandidate);
    }
}

#[cfg(test)]
mod test_depth_fallback {
    //! TestDepth exists because discarding the test-count proxy entirely made every
    //! consensus-matrix task fall through to Simplicity, which rewards the shortest
    //! implementation -- usually also the one with the fewest tests. It must never outrank
    //! a direct measurement, and must never be silently skipped when none exists.
    use super::*;

    fn cand(arm: &str, tests: u32, lines: u32) -> Evidence {
        Evidence {
            arm: arm.into(),
            conformance: Some(1.0),
            defect_sensitivity: None,
            survival: None,
            clippy: Some(0),
            crates_touched: Some(1),
            tests: Some(tests),
            lines: Some(lines),
            cost_usd: None,
            suite_overfitted: None,
        }
    }

    #[test]
    fn test_depth_decides_when_no_direct_quality_measure_exists() {
        // the confinement field: nothing discriminates, so the proxy is all that remains
        let field = [cand("codex-luna", 7, 226), cand("glm-53-flash", 14, 320)];
        match adjudicate(&field, 0.001) {
            Ruling::Winner { arm, on, .. } => {
                assert_eq!(arm, "glm-53-flash");
                assert_eq!(on, Criterion::TestDepth);
            }
            other => panic!("expected a TestDepth winner, got {other:?}"),
        }
    }

    #[test]
    fn survival_decides_before_the_proxy_but_does_not_suppress_it() {
        let mut field = [cand("few-tests", 7, 100), cand("many-tests", 14, 320)];
        field[0].survival = Some(1.0);
        field[1].survival = Some(0.0);
        match adjudicate(&field, 0.001) {
            Ruling::Winner { arm, on, .. } => {
                assert_eq!(arm, "few-tests", "a direct measure must beat the proxy");
                assert_eq!(on, Criterion::Survival);
            }
            other => panic!("expected a Survival winner, got {other:?}"),
        }
    }

    #[test]
    fn a_tied_survival_falls_through_to_the_proxy_not_to_length() {
        // Both survived everything. Survival is a property of the IMPLEMENTATION and says
        // nothing about suite quality, so a tie there must not suppress TestDepth -- doing
        // so picked the shorter candidate over the better-tested one.
        let mut field = [cand("short", 7, 100), cand("long", 14, 320)];
        field[0].survival = Some(1.0);
        field[1].survival = Some(1.0);
        match adjudicate(&field, 0.001) {
            Ruling::Winner { arm, on, .. } => {
                assert_eq!(arm, "long", "12 tests beats 7 when survival ties");
                assert_eq!(on, Criterion::TestDepth);
            }
            other => panic!("expected a TestDepth winner, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod overfitted_suites {
    //! A suite that fails nearly every rival is measuring its author's implementation, not
    //! the specification. Its test count is real and worthless as evidence of depth.
    use super::*;

    fn cand(arm: &str, tests: u32, lines: u32, overfitted: bool) -> Evidence {
        Evidence {
            arm: arm.into(),
            conformance: Some(1.0),
            defect_sensitivity: None,
            survival: None,
            clippy: Some(0),
            crates_touched: Some(1),
            tests: Some(tests),
            lines: Some(lines),
            cost_usd: None,
            suite_overfitted: Some(overfitted),
        }
    }

    #[test]
    fn an_overfitted_suite_does_not_win_on_its_test_count() {
        // the budget field: glm-53-flash wrote 18 tests to codex-luna's 9, and failed all
        // three rivals, so its count carries no evidence of depth
        let field = [
            cand("glm-53-flash", 18, 139, true),
            cand("codex-luna", 9, 86, false),
        ];
        match adjudicate(&field, 0.001) {
            Ruling::Winner { arm, on, .. } => {
                assert_eq!(arm, "codex-luna");
                assert_eq!(on, Criterion::TestDepth);
            }
            other => panic!("expected codex-luna on TestDepth, got {other:?}"),
        }
    }

    #[test]
    fn one_overfitted_candidate_does_not_disable_depth_for_the_field() {
        // Scoring it as unmeasured skipped the criterion entirely and handed the decision to
        // Simplicity, which picked the shortest implementation. Zero, not None.
        let field = [
            cand("overfit", 30, 100, true),
            cand("shortest", 8, 120, false),
            cand("deepest", 12, 400, false),
        ];
        match adjudicate(&field, 0.001) {
            Ruling::Winner { arm, on, .. } => {
                assert_eq!(arm, "deepest", "depth must still rank the sound suites");
                assert_eq!(on, Criterion::TestDepth);
            }
            other => panic!("expected deepest on TestDepth, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod cost_never_decides {
    use super::*;

    fn tied(arm: &str, cost: Option<f64>) -> Evidence {
        Evidence {
            arm: arm.to_string(),
            conformance: Some(1.0),
            defect_sensitivity: Some(0.5),
            survival: Some(0.5),
            clippy: Some(0),
            crates_touched: Some(1),
            tests: Some(1),
            lines: Some(10),
            cost_usd: cost,
            suite_overfitted: None,
        }
    }

    /// Two candidates identical on every quality axis and differing only in what they cost
    /// to produce must NOT yield a winner. Cost is a fact about the arm, not the patch.
    #[test]
    fn a_field_separated_only_by_cost_is_undecided() {
        let field = [tied("cheap", Some(0.01)), tied("dear", Some(9.99))];
        match adjudicate(&field, 0.0) {
            Ruling::Winner { arm, on, .. } => {
                panic!("cost decided the merge: {arm} won on {on:?}")
            }
            Ruling::Undecided { tied, .. } => {
                assert_eq!(tied.len(), 2, "both candidates remain in contention");
            }
            Ruling::NoCandidate => panic!("both candidates are eligible"),
        }
    }

    /// And the criterion is gone from the ordering entirely, so it cannot return by way of
    /// `missing_evidence` suggesting it as a next measurement to gather.
    #[test]
    fn cost_is_not_a_criterion_anywhere() {
        let field = [tied("a", Some(1.0)), tied("b", None)];
        let next = missing_evidence(&field);
        assert!(
            !format!("{next:?}").contains("Cost"),
            "cost must not be suggested as evidence to gather: {next:?}"
        );
    }
}

#[cfg(test)]
mod never_measured_is_visible {
    use super::*;

    fn ev(arm: &str) -> Evidence {
        Evidence {
            arm: arm.to_string(),
            conformance: Some(1.0),
            defect_sensitivity: None, // as it is for EVERY task in this project today
            survival: Some(0.5),
            clippy: Some(0),
            crates_touched: Some(1),
            tests: Some(1),
            lines: Some(10),
            cost_usd: None,
            suite_overfitted: None,
        }
    }

    /// Instance thirteen: a criterion measured for NOBODY was filtered out of the missing
    /// report, so "the instrument never ran" read exactly like "this does not apply here".
    #[test]
    fn a_criterion_measured_for_nobody_is_reported_as_absent() {
        let field = [ev("a"), ev("b")];
        let gaps = evidence_gaps(&field);
        assert!(
            gaps.contains(&Gap::Absent(Criterion::DefectSensitivity)),
            "defect sensitivity is unmeasured for the whole field and must say so: {gaps:?}"
        );
    }

    /// And the two kinds of gap stay distinguishable: partial means gather the rest.
    #[test]
    fn partial_and_absent_are_different_facts() {
        let mut half = ev("half");
        half.clippy = None;
        let field = [ev("whole"), half];
        let gaps = evidence_gaps(&field);
        assert!(gaps.contains(&Gap::Partial(Criterion::Clippy)), "{gaps:?}");
        assert!(
            gaps.contains(&Gap::Absent(Criterion::DefectSensitivity)),
            "{gaps:?}"
        );
        assert!(
            !gaps.contains(&Gap::Absent(Criterion::Clippy)),
            "a partially measured criterion is not absent: {gaps:?}"
        );
    }

    /// The narrower legacy question still answers as it did, so nothing silently changed.
    #[test]
    fn missing_evidence_keeps_its_old_meaning() {
        let field = [ev("a"), ev("b")];
        assert!(
            !missing_evidence(&field).contains(&Criterion::DefectSensitivity),
            "the old function deliberately still omits the wholly unmeasured"
        );
    }

    /// An empty field has no gaps to report, rather than every criterion absent.
    #[test]
    fn an_empty_field_reports_no_gaps() {
        assert!(evidence_gaps(&[]).is_empty());
    }
}
