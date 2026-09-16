// farmerbob's own acceptance tests for the verify-router spec.
// Targets the rules most likely to be skimmed, not the easy ones.
#[cfg(test)]
mod fb_conformance {
    use crate::vrouter::*;

    fn stats(rd: u32, md: u32, aa: u32, ra: u32) -> VerifierStats {
        VerifierStats { rejected_defective: rd, missed_defective: md,
                        approved_acceptable: aa, rejected_acceptable: ra,
                        ..Default::default() }
    }
    fn ev(found: Vec<(Check, Finding)>) -> Evidence {
        Evidence { family: TaskFamily("t".into()), completed: found,
                   excluded: vec![], spent: 0 }
    }

    #[test] fn default_stats_never_divide_by_zero() {
        let s = VerifierStats::default();
        assert_eq!(s.sensitivity(), None);
        assert_eq!(s.specificity(), None);
        assert_eq!(s.precision(), None);
        assert_eq!(s.n(), 0);
    }
    #[test] fn sensitivity_and_specificity_are_separate() {
        // approves everything: perfect specificity, zero sensitivity
        let s = stats(0, 10, 10, 0);
        assert_eq!(s.sensitivity(), Some(0.0));
        assert_eq!(s.specificity(), Some(1.0));
    }
    #[test] fn spec_ambiguous_outranks_demonstrated_defect() {
        let d = decide(&ev(vec![
            (Check::ExecutedGate, Finding::DemonstratedDefect { detail: "x".into() }),
            (Check::CodeReview(VerifierId("v".into())),
             Finding::SpecAmbiguous { detail: "y".into() }),
        ]));
        assert_eq!(d, Decision::Undetermined,
            "an unspecified requirement cannot be violated; the task is at fault");
    }
    #[test] fn demonstrated_defect_rejects() {
        assert_eq!(decide(&ev(vec![
            (Check::ExecutedGate, Finding::DemonstratedDefect { detail: "x".into() })])),
            Decision::Reject);
    }
    #[test] fn suspicion_without_demonstration_is_repair() {
        assert_eq!(decide(&ev(vec![
            (Check::CodeReview(VerifierId("v".into())),
             Finding::SuspectedDefect { detail: "x".into() })])),
            Decision::Repair);
    }
    #[test] fn no_further_check_once_rejected() {
        let cfg = RouterConfig { review_budget: 100, cost_per_check: 1,
                                 min_n_for_calibration: 5, roster: vec![] };
        assert_eq!(next_check(&ev(vec![
            (Check::ExecutedGate, Finding::DemonstratedDefect { detail: "x".into() })]), &cfg),
            None, "review cannot change a settled reject");
    }
    #[test] fn budget_exhaustion_stops() {
        let cfg = RouterConfig { review_budget: 1, cost_per_check: 5,
                                 min_n_for_calibration: 5, roster: vec![] };
        assert_eq!(next_check(&ev(vec![]), &cfg), None);
    }
    #[test] fn uncalibrated_verifier_is_shadow() {
        let cfg = RouterConfig { review_budget: 100, cost_per_check: 1, min_n_for_calibration: 5,
            roster: vec![(VerifierId("new".into()), "fam".into(), VerifierStats::default())] };
        assert!(is_shadow(&VerifierId("new".into()), &cfg),
            "a verifier with no adjudicated history must not label anything");
        assert!(is_shadow(&VerifierId("absent".into()), &cfg));
    }
    #[test] fn calibration_requires_seeing_a_defective_case() {
        // plenty of observations, but never a defective one -> sensitivity unmeasured
        assert!(!stats(0, 0, 50, 0).is_calibrated(5),
            "specificity alone cannot establish calibration");
    }
}
