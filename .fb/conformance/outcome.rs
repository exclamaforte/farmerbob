
// ESCALATED from cross-examination: codex-luna's suite discriminated on outcome.
// Not a CLAIM -- cross-examination found it directly. Kept only because it passes
// against the merged winner, which is what separates a discovery from an
// over-fitted suite.
#[cfg(test)]
mod cx_outcome_codex_luna {
    use super::*;

    fn outcome(class: OutcomeClass, verdict: Verdict) -> Outcome {
        Outcome {
            arm: "arm".to_owned(),
            task: "task".to_owned(),
            verdict,
            class,
            rescue: Rescue::None,
            lines: if verdict == Verdict::Pass { 1 } else { 0 },
            tests_run: if verdict == Verdict::Pass { 1 } else { 0 },
            usd: None,
        }
    }

    #[test]
    fn unknown_does_not_count_for_the_posterior() {
        assert!(!OutcomeClass::Unknown.counts_for_posterior());
        assert_eq!(
            posterior_delta(
                &[outcome(OutcomeClass::Unknown, Verdict::NoOp)],
                Rescue::None
            ),
            (0, 0)
        );
    }

    #[test]
    fn a_pass_with_zero_tests_fails_validation() {
        let mut record = outcome(OutcomeClass::ArmResult, Verdict::Pass);
        record.tests_run = 0;
        assert!(record.validate().is_err());
    }

    #[test]
    fn validation_order_is_deterministic_when_two_rules_are_violated_at_once() {
        let mut record = outcome(OutcomeClass::ArmResult, Verdict::Pass);
        record.tests_run = 0;
        record.lines = 0;
        assert_eq!(
            record.validate().err().as_deref(),
            Some("Pass requires tests_run > 0")
        );
    }

    #[test]
    fn a_repaired_pass_above_max_rescue_increments_neither_alpha_nor_beta() {
        let mut record = outcome(OutcomeClass::ArmResult, Verdict::Pass);
        record.rescue = Rescue::Repaired;
        assert_eq!(posterior_delta(&[record], Rescue::Cosmetic), (0, 0));
    }

    #[test]
    fn a_quota_limited_run_is_absent_from_both_posterior_counts() {
        assert_eq!(
            posterior_delta(
                &[outcome(OutcomeClass::QuotaLimited, Verdict::NoCompile)],
                Rescue::None
            ),
            (0, 0)
        );
    }

    #[test]
    fn zero_and_unmeasured_cost_validate() {
        let mut zero = outcome(OutcomeClass::ArmResult, Verdict::NoOp);
        zero.usd = Some(0.0);
        assert!(zero.validate().is_ok());
        let unmeasured = outcome(OutcomeClass::ArmResult, Verdict::NoOp);
        assert!(unmeasured.validate().is_ok());
    }

    #[test]
    fn the_census_omits_zero_counts() {
        let records = vec![outcome(OutcomeClass::Unknown, Verdict::NoOp)];
        assert_eq!(
            class_census(&records),
            vec![("arm".to_owned(), vec![(OutcomeClass::Unknown, 1)])]
        );
    }

    #[test]
    fn needs_triage_keeps_input_order() {
        let mut first = outcome(OutcomeClass::Unknown, Verdict::NoOp);
        first.task = "first".to_owned();
        let middle = outcome(OutcomeClass::ArmResult, Verdict::NoOp);
        let mut last = outcome(OutcomeClass::Unknown, Verdict::NoOp);
        last.task = "last".to_owned();
        let records = vec![first, middle, last];
        let triage = needs_triage(&records);
        assert_eq!(triage[0].task, "first");
        assert_eq!(triage[1].task, "last");
    }
}
