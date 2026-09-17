/// Why a run ended. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeClass {
    /// The arm was given a fair chance and this is what it did.
    ArmResult,
    /// Harness, launcher, credential or machine fault.
    Infrastructure,
    /// The provider refused on quota grounds.
    QuotaLimited,
    /// The task was unrunnable as specified, e.g. its deliverable already existed.
    TaskInvalid,
    /// The orchestrator stopped it.
    Cancelled,
    /// Not yet classified. Must be treated as unusable, never as a failure.
    Unknown,
}

impl OutcomeClass {
    /// Only ArmResult reaches a posterior. This is the whole point of the enum.
    pub fn counts_for_posterior(self) -> bool {
        matches!(self, Self::ArmResult)
    }
}

/// The gate verdict. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The changes passed the gate.
    Pass,
    /// The arm made no changes.
    NoOp,
    /// The changes did not compile.
    NoCompile,
    /// The test suite failed.
    TestsFail,
    /// No tests were available or run.
    NoTests,
}

/// How much orchestrator work a PASS required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rescue {
    /// Merged as delivered.
    None,
    /// Lints or formatting fixed.
    Cosmetic,
    /// Build or tests repaired.
    Repaired,
    /// Substantially rewritten. A PASS here is barely the arm's.
    Rewritten,
}

/// The measured result of one arm attempting one task.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The arm that produced the result.
    pub arm: String,
    /// The task attempted by the arm.
    pub task: String,
    /// The gate's verdict.
    pub verdict: Verdict,
    /// Why the run ended.
    pub class: OutcomeClass,
    /// Orchestrator work needed before accepting the result.
    pub rescue: Rescue,
    /// Number of changed lines.
    pub lines: u32,
    /// Number of tests run.
    pub tests_run: u32,
    /// Measured USD. `Some(0.0)` is a real zero; `None` is unmeasured.
    pub usd: Option<f64>,
}

impl Outcome {
    /// A success the posterior may learn from: ArmResult, Pass, and rescue at most
    /// `max_rescue`. An orchestrator-repaired pass is not evidence the arm can do it.
    pub fn is_clean_success(&self, max_rescue: Rescue) -> bool {
        self.class == OutcomeClass::ArmResult
            && self.verdict == Verdict::Pass
            && self.rescue <= max_rescue
    }

    /// Self-consistency. A record can be wrong about itself and must say so rather than
    /// be silently believed.
    pub fn validate(&self) -> Result<(), String> {
        if self.verdict == Verdict::Pass && self.tests_run == 0 {
            return Err("Pass requires tests_run > 0".to_string());
        }
        if self.verdict == Verdict::Pass && self.lines == 0 {
            return Err("Pass requires lines > 0".to_string());
        }
        if self.verdict == Verdict::NoOp && self.lines > 0 {
            return Err("NoOp requires lines == 0".to_string());
        }
        if self.class != OutcomeClass::ArmResult && self.rescue > Rescue::None {
            return Err("non-ArmResult cannot carry rescue above None".to_string());
        }
        if let Some(usd) = self.usd
            && (usd < 0.0 || !usd.is_finite())
        {
            return Err("usd must be non-negative and finite".to_string());
        }
        Ok(())
    }
}

/// Beta update over records, skipping everything that does not count.
/// Returns (alpha increment, beta increment).
pub fn posterior_delta(rs: &[Outcome], max_rescue: Rescue) -> (u32, u32) {
    let mut alpha: u32 = 0;
    let mut beta: u32 = 0;
    for outcome in rs {
        if !outcome.class.counts_for_posterior() {
            continue;
        }
        if outcome.is_clean_success(max_rescue) {
            alpha = alpha.saturating_add(1);
        } else if outcome.verdict != Verdict::Pass || outcome.rescue <= max_rescue {
            beta = beta.saturating_add(1);
        }
    }
    (alpha, beta)
}

/// Records whose class is Unknown. These are a backlog to triage, never a failure count.
pub fn needs_triage(rs: &[Outcome]) -> Vec<&Outcome> {
    rs.iter()
        .filter(|outcome| outcome.class == OutcomeClass::Unknown)
        .collect()
}

/// Per-arm counts of every class, omitting zero counts and sorting by arm name.
pub fn class_census(rs: &[Outcome]) -> Vec<(String, Vec<(OutcomeClass, u32)>)> {
    let mut census: Vec<(String, [u32; 6])> = Vec::new();
    for outcome in rs {
        let position = census.iter().position(|(arm, _)| arm == &outcome.arm);
        let index = class_index(outcome.class);
        match position {
            Some(position) => {
                census[position].1[index] = census[position].1[index].saturating_add(1);
            }
            None => {
                let mut counts = [0; 6];
                counts[index] = 1;
                census.push((outcome.arm.clone(), counts));
            }
        }
    }

    census.sort_by(|left, right| left.0.cmp(&right.0));
    census
        .into_iter()
        .map(|(arm, counts)| {
            let classes = [
                OutcomeClass::ArmResult,
                OutcomeClass::Infrastructure,
                OutcomeClass::QuotaLimited,
                OutcomeClass::TaskInvalid,
                OutcomeClass::Cancelled,
                OutcomeClass::Unknown,
            ];
            let counts = classes
                .into_iter()
                .enumerate()
                .filter_map(|(index, class)| (counts[index] > 0).then_some((class, counts[index])))
                .collect();
            (arm, counts)
        })
        .collect()
}

fn class_index(class: OutcomeClass) -> usize {
    match class {
        OutcomeClass::ArmResult => 0,
        OutcomeClass::Infrastructure => 1,
        OutcomeClass::QuotaLimited => 2,
        OutcomeClass::TaskInvalid => 3,
        OutcomeClass::Cancelled => 4,
        OutcomeClass::Unknown => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(class: OutcomeClass, verdict: Verdict, rescue: Rescue) -> Outcome {
        Outcome {
            arm: "arm".to_string(),
            task: "task".to_string(),
            verdict,
            class,
            rescue,
            lines: if verdict == Verdict::NoOp { 0 } else { 1 },
            tests_run: 1,
            usd: None,
        }
    }

    #[test]
    fn unknown_does_not_count_for_the_posterior() {
        let records = [outcome(OutcomeClass::Unknown, Verdict::NoCompile, Rescue::None)];
        assert_eq!(posterior_delta(&records, Rescue::Rewritten), (0, 0));
        assert!(!OutcomeClass::Unknown.counts_for_posterior());
    }

    #[test]
    fn pass_with_zero_tests_fails_validation() {
        let mut record = outcome(OutcomeClass::ArmResult, Verdict::Pass, Rescue::None);
        record.tests_run = 0;
        assert!(record.validate().is_err());
    }

    #[test]
    fn validation_order_is_deterministic_when_two_rules_are_violated_at_once() {
        let mut record = outcome(OutcomeClass::Infrastructure, Verdict::Pass, Rescue::Cosmetic);
        record.tests_run = 0;
        record.lines = 0;
        assert_eq!(record.validate(), Err("Pass requires tests_run > 0".to_string()));
    }

    #[test]
    fn repaired_pass_above_max_rescue_increments_neither_alpha_nor_beta() {
        let records = [outcome(OutcomeClass::ArmResult, Verdict::Pass, Rescue::Repaired)];
        assert_eq!(posterior_delta(&records, Rescue::Cosmetic), (0, 0));
    }

    #[test]
    fn quota_limited_run_is_absent_from_both_posterior_counts() {
        let records = [outcome(OutcomeClass::QuotaLimited, Verdict::NoCompile, Rescue::None)];
        assert_eq!(posterior_delta(&records, Rescue::Rewritten), (0, 0));
    }

    #[test]
    fn some_zero_usd_validates_and_none_validates() {
        let mut zero = outcome(OutcomeClass::ArmResult, Verdict::NoOp, Rescue::None);
        zero.usd = Some(0.0);
        assert!(zero.validate().is_ok());
        let unmeasured = outcome(OutcomeClass::ArmResult, Verdict::NoOp, Rescue::None);
        assert!(unmeasured.validate().is_ok());
    }

    #[test]
    fn negative_or_non_finite_usd_fails_validation() {
        let mut negative = outcome(OutcomeClass::ArmResult, Verdict::NoOp, Rescue::None);
        negative.usd = Some(-0.01);
        assert!(negative.validate().is_err());
        let mut nan = outcome(OutcomeClass::ArmResult, Verdict::NoOp, Rescue::None);
        nan.usd = Some(f64::NAN);
        assert!(nan.validate().is_err());
    }

    #[test]
    fn non_arm_rescue_fails_validation() {
        let record = outcome(OutcomeClass::Infrastructure, Verdict::NoOp, Rescue::Cosmetic);
        assert!(record.validate().is_err());
    }

    #[test]
    fn arm_result_non_pass_is_beta_evidence() {
        let records = [outcome(
            OutcomeClass::ArmResult,
            Verdict::TestsFail,
            Rescue::Rewritten,
        )];
        assert_eq!(posterior_delta(&records, Rescue::None), (0, 1));
    }

    #[test]
    fn clean_success_requires_arm_result_pass_and_allowed_rescue() {
        assert!(outcome(OutcomeClass::ArmResult, Verdict::Pass, Rescue::None)
            .is_clean_success(Rescue::None));
        assert!(!outcome(OutcomeClass::Unknown, Verdict::Pass, Rescue::None)
            .is_clean_success(Rescue::None));
        assert!(!outcome(OutcomeClass::ArmResult, Verdict::NoTests, Rescue::None)
            .is_clean_success(Rescue::None));
        assert!(!outcome(OutcomeClass::ArmResult, Verdict::Pass, Rescue::Repaired)
            .is_clean_success(Rescue::Cosmetic));
    }

    #[test]
    fn census_omits_zero_counts() {
        let records = [outcome(OutcomeClass::Infrastructure, Verdict::NoOp, Rescue::None)];
        assert_eq!(
            class_census(&records),
            vec![(
                "arm".to_string(),
                vec![(OutcomeClass::Infrastructure, 1)]
            )]
        );
    }

    #[test]
    fn needs_triage_keeps_input_order() {
        let mut first = outcome(OutcomeClass::Unknown, Verdict::NoOp, Rescue::None);
        first.arm = "first".to_string();
        let second = outcome(OutcomeClass::ArmResult, Verdict::NoOp, Rescue::None);
        let mut third = outcome(OutcomeClass::Unknown, Verdict::NoOp, Rescue::None);
        third.arm = "third".to_string();
        let records = [first, second, third];
        let triage = needs_triage(&records);
        assert_eq!(triage.len(), 2);
        assert_eq!(triage[0].arm, "first");
        assert_eq!(triage[1].arm, "third");
    }
}

#[cfg(test)]
mod promoted_claims {
    // Promoted from codex-luna's critique of or-deepseek-v4-flash: a negative-zero usd
    // slips past a `< 0.0` check because -0.0 < 0.0 is false and -0.0 is finite. Executed
    // against the MERGED winner rather than the arm it was aimed at -- the interesting
    // question is whether the code we shipped has the hole, not whether a rejected
    // candidate did.
    use super::*;

    #[test]
    fn claim_negative_zero_usd_is_not_smuggled_past_validation() {
        let o = Outcome {
            arm: "a".into(), task: "t".into(),
            verdict: Verdict::Pass, class: OutcomeClass::ArmResult,
            rescue: Rescue::None, lines: 10, tests_run: 3,
            usd: Some(-0.0),
        };
        // -0.0 is a legitimate zero, not a negative cost: it must VALIDATE.
        assert!(o.validate().is_ok(), "-0.0 should be accepted as zero: {:?}", o.validate());
        // and a genuinely negative cost must not be.
        let neg = Outcome { usd: Some(-1.0), ..o };
        assert!(neg.validate().is_err(), "a negative usd must be rejected");
    }
}


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
