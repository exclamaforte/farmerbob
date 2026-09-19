//! The engine behind the future `fb verify` subcommand.
//!
//! The shell observes five strings about a candidate worktree; the decision
//! about what they mean lives entirely in `farmerbob_core` --
//! [`farmerbob_core::build_verdict`] reads the test log, and
//! [`farmerbob_core::verify_plan`] decides the fate -- so this module only
//! renders what the core already decided: one line for an operator, one code
//! for a script. Nothing here matches on log text or exit codes; a decision
//! made here would be the second copy of `fb-verify.sh`'s table that this
//! layer exists to delete.
//!
//! Unwired by design: attaching this to the argument parser is a separate
//! task, and an uncalled `pub fn` in a binary crate trips `dead_code` under
//! `-D warnings`, hence the allow below.

#![allow(dead_code)]

use farmerbob_core::build_verdict;
use farmerbob_core::verify_plan::{self, Fate, Observed};

/// What the caller observed about one candidate, as strings from the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raw {
    /// Arm name.
    pub arm: String,
    /// Whether the declared deliverable exists and is non-empty.
    pub deliverable: bool,
    /// Whether the build command exited 0. `None` when no build was attempted.
    pub built: Option<bool>,
    /// The test log, read verbatim. Empty when no suite ran.
    pub test_log: String,
    /// Whether the harness cut the run short.
    pub timed_out: bool,
}

/// Render one candidate's fate as a line for an operator, and a code for a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The rendered line. Never empty, never ends in a newline.
    pub text: String,
    /// 0 measurable, 1 failed, 2 no-op, 3 cut.
    pub code: i32,
}

/// Decide and render one candidate.
pub fn assess(r: &Raw) -> Line {
    let fate = fate_of(r);
    render(&r.arm, &fate)
}

/// Assess a field, in the order given.
///
/// Returns one [`Line`] per input, in input order, and one field code. The
/// lines are independent of the code: even a refused field carries every
/// candidate's line, so an operator can see which candidates were fine.
///
/// The field code is 0 when the field is rankable -- at least two code-0
/// candidates, which is what [`verify_plan::rankable`] measures -- and every
/// line is measurable. It is 4, NOT APPLICABLE, when fewer than two
/// candidates are measurable and every line is still measurable: an empty
/// field or a lone measurable one is refused without meaning failed. Any
/// other field takes the worst code in it -- 1 failed, 2 no-op, 3 cut --
/// because a line that says what happened is the more specific fact; that
/// includes rankable fields carrying a failed, no-op or cut line.
pub fn assess_field(raws: &[Raw]) -> (Vec<Line>, i32) {
    let mut lines = Vec::with_capacity(raws.len());
    let mut fates = Vec::with_capacity(raws.len());
    for r in raws {
        let fate = fate_of(r);
        lines.push(render(&r.arm, &fate));
        fates.push(fate);
    }

    let tally = verify_plan::field(&fates);
    let worst = lines.iter().map(|line| line.code).max().unwrap_or(0);
    let code = if worst > 0 {
        worst
    } else if verify_plan::rankable(&tally) {
        0
    } else {
        // Every line is measurable and there are fewer than two of them:
        // nothing failed, and one candidate is not a comparison.
        4
    };
    (lines, code)
}

/// The core's decision for one candidate: the shell's `built` flag goes
/// through `build_verdict::read`, and `verify_plan::fate` decides.
fn fate_of(r: &Raw) -> Fate {
    let build = r.built.map(|ok| build_verdict::read(ok, &r.test_log));
    verify_plan::fate(&Observed {
        deliverable: r.deliverable,
        build,
        timed_out: r.timed_out,
    })
}

/// The code pinned for each `Fate`: 0 measurable, 1 failed, 2 no-op, 3 cut.
fn code_of(fate: &Fate) -> i32 {
    match fate {
        Fate::Measure { .. } => 0,
        Fate::Failed(_) => 1,
        Fate::NoOp => 2,
        Fate::Cut { .. } => 3,
    }
}

/// One line for an operator: the arm, the fate, and whatever the core can say
/// about it. Never empty and never newline-terminated, even for an empty arm
/// name, because the fate's own words follow the separator.
fn render(arm: &str, fate: &Fate) -> Line {
    let text = match fate {
        Fate::Measure { tests_passed } => {
            format!("{arm}: measurable, {tests_passed} tests passed")
        }
        Fate::Failed(reason) => format!("{arm}: failed -- {reason}"),
        Fate::NoOp => format!("{arm}: no-op, the arm wrote no deliverable"),
        Fate::Cut { .. } => format!("{arm}: cut by the harness timeout, not the arm's failure"),
    };
    Line {
        text,
        code: code_of(fate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const PASSING: &str =
        "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
    const FAILING: &str =
        "test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out";

    /// Build a `Raw` from the pinned public fields; nothing else.
    fn raw(
        arm: &str,
        deliverable: bool,
        built: Option<bool>,
        test_log: &str,
        timed_out: bool,
    ) -> Raw {
        Raw {
            arm: arm.to_string(),
            deliverable,
            built,
            test_log: test_log.to_string(),
            timed_out,
        }
    }

    /// The fate the core decides for the same observation, computed straight
    /// from `farmerbob_core` so the expectation never passes through this
    /// module's own code.
    fn core_fate(r: &Raw) -> Fate {
        let build = r.built.map(|ok| build_verdict::read(ok, &r.test_log));
        verify_plan::fate(&Observed {
            deliverable: r.deliverable,
            build,
            timed_out: r.timed_out,
        })
    }

    /// The code the spec pins to each fate: 0 measurable, 1 failed, 2 no-op, 3 cut.
    fn pinned_code(f: &Fate) -> i32 {
        match f {
            Fate::Measure { .. } => 0,
            Fate::Failed(_) => 1,
            Fate::NoOp => 2,
            Fate::Cut { .. } => 3,
        }
    }

    #[test]
    fn a_timeout_is_code_three_over_every_other_fate() {
        // Clause 2: the cut dominates a would-be-measurable, a would-be-failed
        // and a would-be-no-op candidate alike. An implementation that checks
        // the build first fails this.
        let would_be = [
            (raw("alpha", true, Some(true), PASSING, true), "measurable"),
            (raw("beta", true, Some(false), FAILING, true), "failed"),
            (raw("gamma", false, None, "", true), "no-op"),
        ];
        for (r, otherwise) in would_be {
            let line = assess(&r);
            assert_eq!(
                line.code, 3,
                "timeout over a would-be-{otherwise} candidate"
            );
            assert!(line.text.contains(r.arm.as_str()));
        }
    }

    #[test]
    fn a_measurable_candidate_names_the_arm_and_the_count() {
        // Clause 1.
        let r = raw("alpha", true, Some(true), PASSING, false);
        let line = assess(&r);
        assert_eq!(line.code, 0);
        assert!(line.text.contains("alpha"));
        assert!(
            line.text.contains("7"),
            "must mention the passing count: {}",
            line.text
        );
    }

    #[test]
    fn a_candidate_that_never_started_is_a_no_op() {
        // Clause 3.
        let r = raw("alpha", false, None, "", false);
        assert_eq!(assess(&r).code, 2);
    }

    #[test]
    fn a_broken_build_is_a_failure() {
        // Clause 4, with and without the deliverable: the core fails the
        // build on its own branch, before the deliverable is consulted.
        for deliverable in [true, false] {
            let r = raw("alpha", deliverable, Some(false), FAILING, false);
            assert_eq!(assess(&r).code, 1);
        }
    }

    #[test]
    fn every_code_agrees_with_verify_plan_fate() {
        // Clause 7: the code is the fate's, rendered -- never re-derived. The
        // expected code is computed from the core on the spot, so whatever
        // `build_verdict` makes of a log, `assess` agrees (the empty-log
        // boundary included). The passing-suite-without-deliverable row is
        // the one an implementation keyed on the build alone gets wrong.
        let cases = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(true), "", false),
            raw("gamma", true, Some(false), FAILING, false),
            raw("delta", false, None, "", false),
            raw("epsilon", true, None, "", false),
            raw("zeta", false, Some(true), PASSING, false),
            raw("theta", false, Some(false), "", false),
            raw("iota", true, Some(true), PASSING, true),
        ];
        for r in cases {
            let line = assess(&r);
            assert_eq!(
                line.code,
                pinned_code(&core_fate(&r)),
                "arm {}: {line:?}",
                r.arm
            );
        }
    }

    #[test]
    fn every_line_is_operator_readable_across_all_four_codes() {
        // Clauses 5 and 6 as one test over all four codes: non-empty, no
        // trailing newline, and naming the arm. Sentence and layout are not
        // pinned, so nothing here asserts them.
        let cases = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(false), FAILING, false),
            raw("gamma", false, None, "", false),
            raw("delta", true, Some(true), PASSING, true),
        ];
        let mut codes = HashSet::new();
        for r in cases {
            let line = assess(&r);
            assert!(!line.text.is_empty());
            assert!(!line.text.ends_with('\n'));
            assert!(line.text.contains(r.arm.as_str()));
            codes.insert(line.code);
        }
        assert_eq!(codes, [0, 1, 2, 3].into_iter().collect());
    }

    #[test]
    fn assess_field_returns_one_line_per_input_in_order() {
        // Clause 8: same length, input order -- checked by arm name, since
        // each row has a distinct one.
        let raws = [
            raw("alpha", false, None, "", false),
            raw("beta", true, Some(true), PASSING, false),
            raw("gamma", true, Some(false), FAILING, true),
        ];
        let (lines, _) = assess_field(&raws);
        assert_eq!(lines.len(), raws.len());
        assert!(lines[0].text.contains("alpha"));
        assert!(lines[1].text.contains("beta"));
        assert!(lines[2].text.contains("gamma"));
        assert_eq!([lines[0].code, lines[1].code, lines[2].code], [2, 0, 3]);
    }

    #[test]
    fn one_measurable_field_is_not_applicable_its_duplicate_is_rankable() {
        // Clauses 1 and 2 from ONE Raw: alone the field is NOT APPLICABLE,
        // duplicated it is rankable -- they differ only in count. Clause 7
        // rides along: the lone line is still returned, naming its arm.
        let r = raw("alpha", true, Some(true), PASSING, false);
        let (lines, code) = assess_field(std::slice::from_ref(&r));
        assert_eq!(code, 4);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].code, 0);
        assert!(lines[0].text.contains("alpha"));

        let (_, code) = assess_field(&[r.clone(), r]);
        assert_eq!(code, 0);
    }

    #[test]
    fn a_failed_line_outranks_unrankability() {
        // Clause 5: one measurable plus one failed -- the failed code, not 4.
        // Something genuinely went wrong, and that outranks unrankability.
        let raws = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(false), FAILING, false),
        ];
        let (lines, code) = assess_field(&raws);
        assert_eq!(code, 1);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].code, 0);
        assert_eq!(lines[1].code, 1);
    }

    #[test]
    fn a_cut_line_outranks_rankability() {
        // Clause 6: two measurable plus one cut -- the cut code.
        let raws = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(true), PASSING, false),
            raw("gamma", true, Some(true), PASSING, true),
        ];
        let (lines, code) = assess_field(&raws);
        assert_eq!(code, 3);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn a_no_op_line_outranks_not_applicable() {
        // Boundary: one measurable, one no-op -- the no-op code, not 4. A
        // no-op is a statement about an arm; unrankability is not.
        let raws = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", false, None, "", false),
        ];
        let (lines, code) = assess_field(&raws);
        assert_eq!(code, 2);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn an_all_cut_field_is_the_cut_code_not_not_applicable() {
        // Boundary: three lines, none measurable, all cut -- the cut code,
        // not 4. Every line says something happened; unrankability is the
        // less specific fact.
        let raws = [
            raw("alpha", true, Some(true), PASSING, true),
            raw("beta", true, Some(false), FAILING, true),
            raw("gamma", false, None, "", true),
        ];
        let (lines, code) = assess_field(&raws);
        assert_eq!(code, 3);
        assert!(lines.iter().all(|l| l.code == 3));
    }

    #[test]
    fn rankability_is_two_measurable_candidates() {
        // Clause 9: one measurable is refused, two are rankable, and a field
        // that looks like two -- one measurable plus one cut -- is still one.
        let one = [raw("alpha", true, Some(true), PASSING, false)];
        let (lines, code) = assess_field(&one);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].code, 0);
        assert_ne!(code, 0);

        let two = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(true), PASSING, false),
        ];
        let (_, code) = assess_field(&two);
        assert_eq!(code, 0);

        let looks_like_two = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(true), PASSING, true),
        ];
        let (lines, code) = assess_field(&looks_like_two);
        assert_eq!(lines.len(), 2);
        assert_ne!(code, 0);
    }

    #[test]
    fn an_all_cut_field_is_refused_but_fully_rendered() {
        // Clause 10: nothing "failed", yet the field is refused -- and every
        // line is still code 3.
        let raws = [
            raw("alpha", true, Some(true), PASSING, true),
            raw("beta", false, None, "", true),
        ];
        let (lines, code) = assess_field(&raws);
        assert_ne!(code, 0);
        assert!(lines.iter().all(|l| l.code == 3));
    }

    #[test]
    fn a_refused_field_still_returns_its_measurable_lines() {
        // Composition: an operator reading a refused field must still see
        // which candidates were fine, code-0 lines included.
        let raws = [
            raw("alpha", true, Some(true), PASSING, false),
            raw("beta", true, Some(true), PASSING, true),
        ];
        let (lines, code) = assess_field(&raws);
        assert_ne!(code, 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].code, 0);
        assert!(lines[0].text.contains("alpha"));
    }

    #[test]
    fn an_empty_field_is_not_applicable_with_no_lines() {
        // Clause 4: zero lines return 4 -- an empty field is not a failed
        // field -- and the line vector is empty.
        let (lines, code) = assess_field(&[]);
        assert!(lines.is_empty());
        assert_eq!(code, 4);
    }
}
