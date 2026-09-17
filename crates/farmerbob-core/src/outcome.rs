//! A run's outcome: what the gate decided, why the run ended, and how much
//! orchestrator work a pass required.
//!
//! The verdict itself is NOT defined here. It is [`crate::gate::Verdict`],
//! re-exported below. This module used to carry its own five-variant copy,
//! written without sight of `gate`, which silently lacked `WrongTarget` and
//! `Indeterminate` -- so an outcome recorded through this module could not
//! express "the gate could not see enough to decide" and had to record some
//! failure instead. That is the project's recurring meta-bug reached by a new
//! road: a second model of one concept, where the weaker model loses exactly
//! the distinction the stronger one was built to preserve.
//!   (bead farmerbob-jd2.1)
//!
//! The DECISION that produces an [`OutcomeClass`] from a finished run
//! ([`RunFacts`] through [`classify`]) lives here too, beside the
//! [`crate::limit_signal`] rules it consults. It spent the week in the binary
//! instead and was wrong four times, each time scoring a launcher refusal as
//! the model producing nothing -- four distinct strings, a missing lowercase,
//! a colour escape, a quoted pattern. The rules and the signal now share a
//! module, so the fifth string is a new value in `SignalRules` and nothing
//! more.

pub use crate::gate::Verdict;

use crate::limit_signal::{Classification, SignalRules, classify as classify_signal, normalise};

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

/// What a finished run looked like, as facts rather than conclusions.
///
/// Every field is optional because every field is independently missable: a
/// run can fail to be reaped, or be reaped without its line count ever being
/// taken. An absent fact must stay absent — `None` here is "never known",
/// never a dressed-up zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFacts {
    /// Process exit status. `None` when the run was never reaped.
    pub exit_code: Option<i32>,
    /// Lines added to the declared deliverable. `None` when never measured.
    pub lines_added: Option<u32>,
    /// The head of the run's own log, already ANSI-stripped by the caller.
    ///
    /// The text is normalised once more before matching (stripping is
    /// idempotent, so a caller that forgot costs nothing). How much head to
    /// supply is the caller's curation: every line supplied is eligible to
    /// match, and nothing beyond it — there is no line window here.
    pub log_head: Option<String>,
    /// An explicit class the harness recorded, which is authoritative when present.
    pub declared: Option<OutcomeClass>,
}

/// Classify one finished run.
///
/// The decision order, fixed here so it is not rediscovered:
///
/// 1. `declared` wins outright when present. The harness knows things this
///    function does not.
/// 2. A run never reaped (`exit_code: None`) is [`OutcomeClass::Unknown`].
///    Every later rule conditions on the exit status, so without one no rule
///    may speak for the run. `Unknown` is honest about that and routes the
///    run to [`needs_triage`], instead of silently charging it to the arm
///    (`ArmResult`) or silently excusing it (`Infrastructure`).
/// 3. Exit 143 or 137 is [`OutcomeClass::Cancelled`]. The orchestrator killed
///    it; that is not the arm.
/// 4. Exit 127 is [`OutcomeClass::Infrastructure`]. "Command not found" is
///    always the harness and never the model, which was never reached.
/// 5. A refusal recognised by the [`SignalRules`] is
///    [`OutcomeClass::QuotaLimited`], via `limit_signal::classify`. A
///    configured limit exit code recognises by itself. A pattern recognises
///    only where the text it matched sits at the START of a log-head line:
///    an arm quoting a pattern mid-sentence is discussing quotas, not being
///    refused (the 2026-09-17 incident). A line's leading whitespace is
///    ignored before that check.
/// 6. The shape rule: a non-zero exit, a MEASURED zero lines added, and a
///    log-head line beginning `error:` mean the launcher failed before the
///    agent ran. All three are required. The line alone is not enough — an
///    agent may print an error while working — and the measured-zero
///    condition is what stops a genuinely failing arm being excused. Its
///    hits are [`OutcomeClass::Infrastructure`]: the launcher failing before
///    the agent ran is a harness-side fault, and where no pattern knows the
///    string, the run is excused from the arm without claiming a quota
///    refusal nobody detected. The ways a provider says no are an open set,
///    so a fifth string lands here — never charged to the arm — until a
///    mis-scored run proves the gap and the string joins the
///    [`SignalRules`], which is data, not code.
/// 7. Otherwise [`OutcomeClass::ArmResult`].
///
/// Boundaries:
///
/// - 143, 137 and 127 are a **known subset** of the exit codes that mean the
///   harness rather than the arm. A code outside the subset falls to the
///   ordinary rules; it is never guessed at.
/// - `log_head: None` or `Some("")`: classify on the exit code alone, and
///   never `QuotaLimited` — nothing was read, and an absent log is not
///   evidence of a clean run. Even a configured limit exit code goes
///   unhonoured without text.
/// - `lines_added: None` is not `Some(0)`: the shape rule requires a
///   measured zero, so an unmeasured count leaves the shape rule silent.
/// - Exit codes `i32::MIN` and `i32::MAX` are ordinary non-zero codes and
///   fall through the ordinary rules; neither panics.
/// - A [`SignalRules`] with no patterns and no exit codes makes rule 5
///   inert; the shape rule still applies.
///
/// There is no clock here: `limit_signal::classify` is consulted with `now`
/// fixed at 0, which affects only the reset instant it reports, and that
/// instant is discarded — recognition, not recovery, is this function's job.
pub fn classify(facts: &RunFacts, rules: &SignalRules) -> OutcomeClass {
    if let Some(declared) = facts.declared {
        return declared;
    }
    let Some(exit_code) = facts.exit_code else {
        return OutcomeClass::Unknown;
    };
    if matches!(exit_code, 143 | 137) {
        return OutcomeClass::Cancelled;
    }
    if exit_code == 127 {
        return OutcomeClass::Infrastructure;
    }
    // Without a log nothing was read: no refusal may be declared, and the
    // shape rule has no line to find. The exit code alone decides, and it
    // has already had its say above.
    let Some(head) = facts.log_head.as_deref().filter(|head| !head.is_empty()) else {
        return OutcomeClass::ArmResult;
    };
    if refusal_recognised(rules, exit_code, head) {
        return OutcomeClass::QuotaLimited;
    }
    if launcher_failed_before_the_agent_ran(facts.lines_added, exit_code, head) {
        return OutcomeClass::Infrastructure;
    }
    OutcomeClass::ArmResult
}

/// Rule 5: a refusal recognised by the limit signal.
///
/// A configured limit exit code recognises by itself — an exit code cannot be
/// quoted mid-sentence, so it needs no anchoring. A pattern recognises only
/// line-anchored; see [`line_starts_with_a_limit_pattern`].
fn refusal_recognised(rules: &SignalRules, exit_code: i32, head: &str) -> bool {
    if rules.limit_exit_codes.contains(&exit_code) {
        return true;
    }
    head.lines()
        .any(|line| line_starts_with_a_limit_pattern(rules, exit_code, line))
}

/// Whether one log-head line evidences a refusal.
///
/// `limit_signal::classify` is fed the line alone, and its `Limited` evidence
/// — the exact text that matched — must begin the line. Feeding the whole
/// head at once would let a pattern quoted inside a sentence count as a
/// refusal; the evidence names WHAT matched, so "does the line begin with
/// it" is the positional test with actual force. Casing is folded here
/// because the positional check happens outside `limit_signal`'s own
/// case-insensitive matcher, and the launchers capitalise.
fn line_starts_with_a_limit_pattern(rules: &SignalRules, exit_code: i32, line: &str) -> bool {
    let flat = normalise(line);
    let Classification::Limited { evidence, .. } = classify_signal(rules, exit_code, &flat, 0)
    else {
        return false;
    };
    let needle = evidence.to_lowercase();
    flat.trim_start().to_lowercase().starts_with(&needle)
}

/// Rule 6: the shape rule. All three conditions are required.
///
/// A launcher that refuses says so in its banner region, on a line of its own
/// beginning `error:`, and the agent never runs. An agent that is WORKING can
/// also print an error, so the line alone proves nothing; the non-zero exit
/// and the measured zero lines are what keep a genuinely failing arm from
/// being excused.
fn launcher_failed_before_the_agent_ran(
    lines_added: Option<u32>,
    exit_code: i32,
    head: &str,
) -> bool {
    if exit_code == 0 {
        return false;
    }
    // An unmeasured count is not a measured zero. Never measured is exactly
    // the case where the shape rule's third condition cannot be checked.
    if lines_added != Some(0) {
        return false;
    }
    head.lines().any(|line| {
        normalise(line)
            .trim_start()
            .to_lowercase()
            .starts_with("error:")
    })
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
        let records = [outcome(
            OutcomeClass::Unknown,
            Verdict::NoCompile,
            Rescue::None,
        )];
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
        let mut record = outcome(
            OutcomeClass::Infrastructure,
            Verdict::Pass,
            Rescue::Cosmetic,
        );
        record.tests_run = 0;
        record.lines = 0;
        assert_eq!(
            record.validate(),
            Err("Pass requires tests_run > 0".to_string())
        );
    }

    #[test]
    fn repaired_pass_above_max_rescue_increments_neither_alpha_nor_beta() {
        let records = [outcome(
            OutcomeClass::ArmResult,
            Verdict::Pass,
            Rescue::Repaired,
        )];
        assert_eq!(posterior_delta(&records, Rescue::Cosmetic), (0, 0));
    }

    #[test]
    fn quota_limited_run_is_absent_from_both_posterior_counts() {
        let records = [outcome(
            OutcomeClass::QuotaLimited,
            Verdict::NoCompile,
            Rescue::None,
        )];
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
        let record = outcome(
            OutcomeClass::Infrastructure,
            Verdict::NoOp,
            Rescue::Cosmetic,
        );
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
        assert!(
            outcome(OutcomeClass::ArmResult, Verdict::Pass, Rescue::None)
                .is_clean_success(Rescue::None)
        );
        assert!(
            !outcome(OutcomeClass::Unknown, Verdict::Pass, Rescue::None)
                .is_clean_success(Rescue::None)
        );
        assert!(
            !outcome(OutcomeClass::ArmResult, Verdict::NoTests, Rescue::None)
                .is_clean_success(Rescue::None)
        );
        assert!(
            !outcome(OutcomeClass::ArmResult, Verdict::Pass, Rescue::Repaired)
                .is_clean_success(Rescue::Cosmetic)
        );
    }

    #[test]
    fn census_omits_zero_counts() {
        let records = [outcome(
            OutcomeClass::Infrastructure,
            Verdict::NoOp,
            Rescue::None,
        )];
        assert_eq!(
            class_census(&records),
            vec![("arm".to_string(), vec![(OutcomeClass::Infrastructure, 1)])]
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
            arm: "a".into(),
            task: "t".into(),
            verdict: Verdict::Pass,
            class: OutcomeClass::ArmResult,
            rescue: Rescue::None,
            lines: 10,
            tests_run: 3,
            usd: Some(-0.0),
        };
        // -0.0 is a legitimate zero, not a negative cost: it must VALIDATE.
        assert!(
            o.validate().is_ok(),
            "-0.0 should be accepted as zero: {:?}",
            o.validate()
        );
        // and a genuinely negative cost must not be.
        let neg = Outcome {
            usd: Some(-1.0),
            ..o
        };
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

// Clause tests for `classify`, one per distinct behaviour the specification
// pins. Where the specification deliberately leaves a bucket open (the shape
// rule's hits), the tests assert only the promise it makes: not an arm
// result.
#[cfg(test)]
mod classification {
    use super::*;

    fn rules() -> SignalRules {
        SignalRules::new(60)
            .with_pattern("error: rate limit exceeded")
            .with_pattern("error: quota exceeded")
    }

    fn facts(exit_code: Option<i32>, lines_added: Option<u32>, log_head: &str) -> RunFacts {
        RunFacts {
            exit_code,
            lines_added,
            log_head: Some(log_head.to_string()),
            declared: None,
        }
    }

    #[test]
    fn declared_wins_over_every_other_fact() {
        for declared in [OutcomeClass::Infrastructure, OutcomeClass::ArmResult] {
            let f = RunFacts {
                declared: Some(declared),
                ..facts(Some(143), Some(0), "error: rate limit exceeded")
            };
            assert_eq!(classify(&f, &rules()), declared, "declared={declared:?}");
        }
    }

    #[test]
    fn declared_beats_a_missing_exit_code() {
        let f = RunFacts {
            declared: Some(OutcomeClass::ArmResult),
            ..facts(None, None, "")
        };
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    // A run never reaped has no exit status, and every later rule conditions
    // on one. Pinned to Unknown: it routes to triage instead of charging or
    // excusing the arm.
    #[test]
    fn a_run_never_reaped_is_unknown() {
        let f = facts(None, Some(0), "error: rate limit exceeded");
        assert_eq!(classify(&f, &rules()), OutcomeClass::Unknown);
    }

    #[test]
    fn orchestrator_kills_are_cancelled() {
        for code in [143, 137] {
            let f = facts(Some(code), Some(0), "error: rate limit exceeded");
            assert_eq!(classify(&f, &rules()), OutcomeClass::Cancelled, "rc={code}");
        }
    }

    #[test]
    fn command_not_found_is_infrastructure_even_with_lines() {
        let f = facts(Some(127), Some(50), "unknown source claude-sonnet");
        assert_eq!(classify(&f, &rules()), OutcomeClass::Infrastructure);
    }

    // Capitalised, because the launchers capitalise and the patterns are
    // lowercase: the missing lowercase was the second repair of the week.
    #[test]
    fn a_capitalised_refusal_on_its_own_line_is_quota_limited() {
        let f = facts(
            Some(1),
            Some(0),
            "Error: Rate limit exceeded: free-models-per-day.",
        );
        assert_eq!(classify(&f, &rules()), OutcomeClass::QuotaLimited);
    }

    #[test]
    fn a_refusal_on_a_later_line_still_anchors() {
        let f = facts(
            Some(1),
            Some(0),
            "Using the credential from the global store.\nError: Rate limit exceeded\n",
        );
        assert_eq!(classify(&f, &rules()), OutcomeClass::QuotaLimited);
    }

    // The caller was supposed to strip; stripping is idempotent, so a caller
    // that forgot still classifies correctly.
    #[test]
    fn an_unstripped_head_is_normalised_again_before_matching() {
        let f = facts(
            Some(1),
            Some(0),
            "\u{1b}[91mError: \u{1b}[0mRate limit exceeded",
        );
        assert_eq!(classify(&f, &rules()), OutcomeClass::QuotaLimited);
    }

    // Verbatim shape of the 2026-09-17 incident: an arm REPORTING that it
    // fixed refusal detection, quoting the pattern it fixed. Exit 0 and 396
    // lines, recorded as quota_limited once and lost from the arm results.
    const QUOTING_ARM: &str = "Done. Provider-refusal detection in `limit_signal.rs` now survives colour: the anchored pattern that missed the incident (`error: rate limit exceeded`) now matches.";

    #[test]
    fn an_arm_quoting_a_pattern_is_an_arm_result() {
        let f = facts(Some(0), Some(396), QUOTING_ARM);
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    // The anchoring clause with actual force: the same quotation on a failing
    // exit. Unanchored substring matching would read the quote as a refusal.
    #[test]
    fn a_quoted_pattern_does_not_fire_on_a_failing_exit_either() {
        let f = facts(Some(1), Some(396), QUOTING_ARM);
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    // The fifth string will arrive and no list will contain it. The shape
    // rule's promise is that it is not charged to the arm; the bucket beyond
    // that is deliberately not asserted.
    #[test]
    fn an_unenumerated_refusal_is_not_charged_to_the_arm() {
        let f = facts(Some(1), Some(0), "error: monthly ceiling reached");
        assert_ne!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    #[test]
    fn an_arm_that_wrote_lines_and_errored_is_an_arm_result() {
        let f = facts(
            Some(1),
            Some(240),
            "error: something went wrong while I was working",
        );
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    #[test]
    fn a_clean_no_op_counts_against_the_arm() {
        let f = facts(Some(0), Some(0), "Error: nothing came of it");
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    // The line must BEGIN a log-head line. Mid-line mentions are discussion.
    #[test]
    fn an_error_mention_that_does_not_begin_its_line_is_not_the_shape_rule() {
        let f = facts(
            Some(1),
            Some(0),
            "the launcher printed: error: no such thing",
        );
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    // An absent log is never a refusal -- not even for a configured limit
    // exit code, because nothing was read. An empty head is the same.
    #[test]
    fn an_absent_or_empty_log_is_never_a_refusal() {
        let r = rules().with_exit_code(429);
        let absent = RunFacts {
            log_head: None,
            ..facts(Some(429), Some(0), "")
        };
        assert_eq!(classify(&absent, &r), OutcomeClass::ArmResult);
        assert_eq!(
            classify(&facts(Some(429), Some(0), ""), &r),
            OutcomeClass::ArmResult
        );
    }

    // Never measured is not zero: the shape rule needs a measured zero, so
    // an unmeasured count leaves it silent.
    #[test]
    fn an_unmeasured_line_count_is_not_a_measured_zero() {
        let f = facts(Some(1), None, "error: something inscrutable went wrong");
        assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult);
    }

    #[test]
    fn extreme_exit_codes_fall_through_the_ordinary_rules() {
        for code in [i32::MIN, i32::MAX] {
            let f = facts(Some(code), Some(5), "error: something went wrong");
            assert_eq!(classify(&f, &rules()), OutcomeClass::ArmResult, "rc={code}");
        }
    }

    // With no patterns nothing is quota_limited by recognition, and the
    // shape rule still applies: neither an arm result nor a recognised
    // refusal, but never charged to the model.
    #[test]
    fn empty_rules_leave_the_shape_rule_standing() {
        let bare = SignalRules::new(60);
        let f = facts(Some(1), Some(0), "error: rate limit exceeded");
        let verdict = classify(&f, &bare);
        assert_ne!(
            verdict,
            OutcomeClass::QuotaLimited,
            "no patterns, so no rule-4 refusal"
        );
        assert_ne!(
            verdict,
            OutcomeClass::ArmResult,
            "the shape rule still applies"
        );
    }

    #[test]
    fn a_configured_limit_exit_code_recognises_without_a_matching_line() {
        let r = rules().with_exit_code(429);
        let f = facts(Some(429), Some(0), "boom");
        assert_eq!(classify(&f, &r), OutcomeClass::QuotaLimited);
    }

    // A new refusal is a new value, never a new match arm.
    #[test]
    fn a_new_refusal_string_is_data_not_code() {
        let r = SignalRules::new(60).with_pattern("error: cosmic rays detected");
        let f = facts(Some(1), Some(0), "error: cosmic rays detected");
        assert_eq!(classify(&f, &r), OutcomeClass::QuotaLimited);
    }
}
