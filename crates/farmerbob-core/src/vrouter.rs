//! Verification router: which check to run next, and when to stop.
//!
//! farmerbob runs many agents on the same task and then has to decide whether
//! each candidate is acceptable. Every check costs time and money, so the
//! router spends a fixed review budget on the checks most likely to change the
//! verdict, and stops as soon as no further check could.
//!
//! The asymmetry this module exists to protect: a bad *implementer* wastes one
//! run, but a bad *verifier* mislabels every candidate it judges, and those
//! labels are what the router learns from. So a verifier is never described by
//! a single accuracy number. [`VerifierStats`] keeps two separate error rates
//! — how often it rejects things that really are defective ([`VerifierStats::sensitivity`])
//! and how often it approves things that really are acceptable
//! ([`VerifierStats::specificity`]). A verifier that approves everything scores
//! a handsome accuracy on a workload that is mostly fine while detecting
//! nothing at all; its sensitivity exposes it.
//!
//! Verdicts from a verifier that is not yet [`VerifierStats::is_calibrated`]
//! are recorded but do not label a candidate; see [`is_shadow`].
//!
//! This module is pure: no I/O, no clock, no randomness. Everything it needs is
//! passed in as [`Evidence`] and [`RouterConfig`].

/// Identity of an agent acting as a verifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct VerifierId(pub String);

/// Class of task a verifier's statistics are tracked against, e.g. `"refactor"`.
///
/// A verifier that is sharp on concurrency bugs is not necessarily sharp on
/// UI behaviour, so rates are never pooled across families.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TaskFamily(pub String);

/// What a check concluded.
///
/// [`Finding::SpecAmbiguous`] is a statement about the *task*: it does not
/// determine the answer, so nothing can be scored against it.
/// [`Finding::UnableToAssess`] is a statement about the *checker*: it lacked the
/// means to judge. The two are kept apart because they call for different
/// responses — respecify the task versus send a different checker.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Finding {
    /// A defect was observed happening, not merely inferred.
    DemonstratedDefect {
        /// What was observed.
        detail: String,
    },
    /// Something looks wrong but has not been shown to be wrong.
    SuspectedDefect {
        /// What looks wrong.
        detail: String,
    },
    /// The check ran to completion and found nothing.
    NoDefectFound,
    /// The check could not form a judgement; a different check is needed.
    UnableToAssess {
        /// Why the checker could not judge.
        reason: String,
    },
    /// The task itself does not determine whether this behaviour is correct.
    SpecAmbiguous {
        /// What is underspecified.
        detail: String,
    },
}

impl Finding {
    /// True when this finding establishes that a defect exists.
    pub fn is_demonstrated(&self) -> bool {
        matches!(self, Finding::DemonstratedDefect { .. })
    }

    /// True when this finding reports a defect it could not demonstrate.
    pub fn is_suspected(&self) -> bool {
        matches!(self, Finding::SuspectedDefect { .. })
    }

    /// True when the check completed and reported no defect.
    pub fn is_clean(&self) -> bool {
        matches!(self, Finding::NoDefectFound)
    }

    /// True when the task itself is underspecified, so nothing can be scored.
    pub fn is_spec_ambiguous(&self) -> bool {
        matches!(self, Finding::SpecAmbiguous { .. })
    }

    /// True when the checker could not form a judgement at all.
    pub fn is_unable_to_assess(&self) -> bool {
        matches!(self, Finding::UnableToAssess { .. })
    }
}

/// A check that can be run against a candidate.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Check {
    /// Did the artifact actually run its own gate (build, tests, lints)?
    ExecutedGate,
    /// Question the implementer about its own work.
    CrossExamination,
    /// Exercise the artifact and observe its behaviour.
    BehaviouralVerifier(VerifierId),
    /// Read the diff and judge it.
    CodeReview(VerifierId),
    /// Try to turn one specific suspicion into a demonstration.
    TargetedReproduction {
        /// The suspicion to reproduce, carried over from a
        /// [`Finding::SuspectedDefect`].
        claim: String,
    },
    /// A test written by another arm that discriminates between candidates.
    DeputyDiscriminatingTest,
    /// The orchestrator re-reads the task and the artifact itself.
    OrchestratorAudit,
}

impl Check {
    /// The verifier carrying out this check, if any.
    ///
    /// [`Check::ExecutedGate`], [`Check::CrossExamination`],
    /// [`Check::DeputyDiscriminatingTest`] and [`Check::OrchestratorAudit`] are
    /// not attributed to an arm on the roster.
    pub fn verifier(&self) -> Option<&VerifierId> {
        match self {
            Check::BehaviouralVerifier(v) | Check::CodeReview(v) => Some(v),
            Check::ExecutedGate
            | Check::CrossExamination
            | Check::TargetedReproduction { .. }
            | Check::DeputyDiscriminatingTest
            | Check::OrchestratorAudit => None,
        }
    }

    /// True when `other` is the same kind of check as `self`, ignoring which
    /// verifier (or which claim) it carries.
    pub fn same_kind(&self, other: &Check) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// The verdict the evidence supports so far.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Decision {
    /// Good enough to take.
    Accept,
    /// A defect was demonstrated; no amount of further review can help.
    Reject,
    /// Not rejected, but a suspicion is unresolved: send it back.
    Repair,
    /// The evidence does not determine a verdict yet.
    Undetermined,
}

/// Per (verifier, task family) outcome counts.
///
/// Only counts are stored, never ratios, so the whole struct can be recomputed
/// from an append-only log of adjudicated observations. Every ratio is derived
/// on demand and returns `None` rather than guessing when its denominator is
/// zero.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifierStats {
    /// Correctly rejected a candidate that really was defective.
    pub rejected_defective: u32,
    /// Approved a candidate that really was defective.
    pub missed_defective: u32,
    /// Correctly approved a candidate that really was acceptable.
    pub approved_acceptable: u32,
    /// Rejected a candidate that really was acceptable.
    pub rejected_acceptable: u32,
    /// Declined to judge at all.
    pub abstained: u32,
    /// Findings this verifier reported.
    pub findings_reported: u32,
    /// Of those, findings later confirmed to be real.
    pub findings_confirmed: u32,
}

impl VerifierStats {
    /// P(reject | defective): of the defective candidates this verifier saw,
    /// the share it caught. `None` when it has not yet seen a defective case,
    /// because the rate is then simply unmeasured rather than zero.
    pub fn sensitivity(&self) -> Option<f64> {
        ratio(
            self.rejected_defective,
            self.rejected_defective
                .saturating_add(self.missed_defective),
        )
    }

    /// P(approve | acceptable): of the acceptable candidates this verifier saw,
    /// the share it let through. `None` when it has not yet seen an acceptable
    /// case.
    ///
    /// Read together with [`VerifierStats::sensitivity`]: a verifier that
    /// approves everything scores a perfect specificity and a useless
    /// sensitivity.
    pub fn specificity(&self) -> Option<f64> {
        ratio(
            self.approved_acceptable,
            self.approved_acceptable
                .saturating_add(self.rejected_acceptable),
        )
    }

    /// Share of the findings this verifier reported that turned out to be real.
    /// `None` when it has reported nothing.
    pub fn precision(&self) -> Option<f64> {
        ratio(self.findings_confirmed, self.findings_reported)
    }

    /// Total adjudicated observations behind these counts.
    pub fn n(&self) -> u32 {
        let total = u64::from(self.rejected_defective)
            + u64::from(self.missed_defective)
            + u64::from(self.approved_acceptable)
            + u64::from(self.rejected_acceptable)
            + u64::from(self.abstained);
        u32::try_from(total).unwrap_or(u32::MAX)
    }

    /// Whether this verifier's verdicts may be trusted to label a candidate in
    /// this task family.
    ///
    /// Two conditions, both required: at least `min_n` adjudicated
    /// observations, and at least one defective case among them. The second
    /// matters most — without a defective case, sensitivity has never been
    /// measured, and a verifier that has only ever seen good work has not shown
    /// it can tell good from bad.
    pub fn is_calibrated(&self, min_n: u32) -> bool {
        let saw_defective = self
            .rejected_defective
            .saturating_add(self.missed_defective)
            > 0;
        self.n() >= min_n && saw_defective
    }
}

/// `numerator / denominator` as a rate, or `None` when there is nothing to
/// divide by. Saturating on the way in, so no input can overflow or divide by
/// zero.
fn ratio(numerator: u32, denominator: u32) -> Option<f64> {
    if denominator == 0 {
        return None;
    }
    Some(f64::from(numerator) / f64::from(denominator))
}

/// Everything the router knows about one candidate.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Evidence {
    /// Which class of task this candidate is being judged against.
    pub family: TaskFamily,
    /// Checks already run, in the order they ran, with what each found.
    pub completed: Vec<(Check, Finding)>,
    /// Arms that must not verify this candidate: the implementer, plus anyone
    /// already used on it.
    pub excluded: Vec<VerifierId>,
    /// Review budget already spent, in whatever unit the caller uses.
    pub spent: u32,
}

impl Evidence {
    /// True when some completed check matches `pred`, whatever it found and
    /// whichever verifier ran it.
    fn has(&self, pred: impl Fn(&Check) -> bool) -> bool {
        self.completed.iter().any(|(c, _)| pred(c))
    }

    /// True when any check found the task itself underspecified.
    fn spec_is_ambiguous(&self) -> bool {
        self.completed.iter().any(|(_, f)| f.is_spec_ambiguous())
    }

    /// Detail of the first suspicion recorded, if any. The earliest suspicion
    /// is the one worth reproducing first.
    fn first_suspicion(&self) -> Option<&str> {
        self.completed.iter().find_map(|(_, f)| match f {
            Finding::SuspectedDefect { detail } => Some(detail.as_str()),
            _ => None,
        })
    }
}

/// Router policy: how much review may be spent, and who is available.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RouterConfig {
    /// Total review budget for one candidate.
    pub review_budget: u32,
    /// What one more check costs, in the same unit as `review_budget`.
    pub cost_per_check: u32,
    /// Observations a verifier needs before its verdicts can label a candidate.
    pub min_n_for_calibration: u32,
    /// Verifiers available, with their model family and their stats for this
    /// task family.
    pub roster: Vec<(VerifierId, String, VerifierStats)>,
}

impl RouterConfig {
    /// The roster entry for `v`, if it is on the roster at all. The first entry
    /// wins if the id appears more than once.
    fn entry<'a>(&'a self, v: &VerifierId) -> Option<&'a (VerifierId, String, VerifierStats)> {
        self.roster.iter().find(|(id, _, _)| id == v)
    }

    /// Stats for `v`, if it is on the roster.
    fn stats(&self, v: &VerifierId) -> Option<&VerifierStats> {
        self.entry(v).map(|(_, _, stats)| stats)
    }
}

/// Pick the next check, or `None` to stop.
///
/// Stopping happens when the budget is exhausted, when a defect has already
/// been demonstrated, or when the task itself is underspecified. Otherwise the
/// ladder is walked in order, skipping kinds already run and verifiers already
/// excluded:
/// [`Check::ExecutedGate`] → [`Check::CrossExamination`] →
/// [`Check::BehaviouralVerifier`] → [`Check::CodeReview`] →
/// [`Check::TargetedReproduction`] (only when a suspicion exists) →
/// [`Check::DeputyDiscriminatingTest`] → [`Check::OrchestratorAudit`].
///
/// A step that needs a verifier is passed over when no verifier is eligible —
/// an empty or fully excluded roster does not stop the router, it just moves on
/// to the checks that need no verifier.
pub fn next_check(ev: &Evidence, cfg: &RouterConfig) -> Option<Check> {
    // Budget: another check must fit inside what is left.
    if ev.spent.saturating_add(cfg.cost_per_check) > cfg.review_budget {
        return None;
    }

    match decide(ev) {
        // Further review cannot undo a demonstrated defect.
        Decision::Reject => return None,
        // Escalating to more reviewers cannot fix an underspecified task. Note
        // that `Undetermined` for any other reason means keep going.
        Decision::Undetermined if ev.spec_is_ambiguous() => return None,
        Decision::Accept | Decision::Repair | Decision::Undetermined => {}
    }

    if !ev.has(|c| matches!(c, Check::ExecutedGate)) {
        return Some(Check::ExecutedGate);
    }
    if !ev.has(|c| matches!(c, Check::CrossExamination)) {
        return Some(Check::CrossExamination);
    }

    // Verifier-backed steps. Both draw on the same roster and the same
    // already-used families: a reviewer from the same model family as a
    // verifier already consulted is a second opinion in name only.
    let used = used_families(ev, cfg);

    if !ev.has(|c| matches!(c, Check::BehaviouralVerifier(_)))
        && let Some(v) = pick_verifier(ev, cfg, &used)
    {
        return Some(Check::BehaviouralVerifier(v));
    }
    if !ev.has(|c| matches!(c, Check::CodeReview(_)))
        && let Some(v) = pick_verifier(ev, cfg, &used)
    {
        return Some(Check::CodeReview(v));
    }

    // Reproducing a suspicion is only meaningful once there is one.
    if let Some(claim) = ev.first_suspicion()
        && !ev.has(|c| matches!(c, Check::TargetedReproduction { .. }))
    {
        return Some(Check::TargetedReproduction {
            claim: claim.to_owned(),
        });
    }

    if !ev.has(|c| matches!(c, Check::DeputyDiscriminatingTest)) {
        return Some(Check::DeputyDiscriminatingTest);
    }
    if !ev.has(|c| matches!(c, Check::OrchestratorAudit)) {
        return Some(Check::OrchestratorAudit);
    }

    None
}

/// The decision implied by the evidence so far.
///
/// Precedence, highest first: an underspecified task outranks everything, since
/// a "defect" measured against a spec that does not determine the answer is not
/// established; then a demonstrated defect; then a clean gate corroborated by a
/// second clean check; then an unconfirmed suspicion.
pub fn decide(ev: &Evidence) -> Decision {
    if ev.spec_is_ambiguous() {
        return Decision::Undetermined;
    }
    if ev.completed.iter().any(|(_, f)| f.is_demonstrated()) {
        return Decision::Reject;
    }

    let gate_clean = ev
        .completed
        .iter()
        .any(|(c, f)| matches!(c, Check::ExecutedGate) && f.is_clean());
    let clean_checks = ev.completed.iter().filter(|(_, f)| f.is_clean()).count();
    if gate_clean && clean_checks >= 2 {
        return Decision::Accept;
    }

    if ev.completed.iter().any(|(_, f)| f.is_suspected()) {
        return Decision::Repair;
    }

    Decision::Undetermined
}

/// Whether verdicts from `v` are to be recorded but not trusted.
///
/// True when `v` is absent from the roster, or its stats for this task family
/// are not yet [`VerifierStats::is_calibrated`]. Shadow verdicts still count
/// towards calibration — that is how a verifier earns its way out of shadow —
/// but they must not be allowed to label a candidate.
pub fn is_shadow(v: &VerifierId, cfg: &RouterConfig) -> bool {
    match cfg.stats(v) {
        None => true,
        Some(stats) => !stats.is_calibrated(cfg.min_n_for_calibration),
    }
}

/// Model families already consulted on this candidate.
///
/// A verifier counts as consulted if it has run a check on this candidate or is
/// excluded as an arm already used on it — which includes the implementer, whose
/// family is exactly the one a reviewer should not share.
fn used_families<'a>(ev: &'a Evidence, cfg: &'a RouterConfig) -> Vec<&'a str> {
    let mut families: Vec<&'a str> = Vec::new();
    for v in ev
        .completed
        .iter()
        .filter_map(|(c, _)| c.verifier())
        .chain(ev.excluded.iter())
    {
        if let Some((_, family, _)) = cfg.entry(v)
            && !families.contains(&family.as_str())
        {
            families.push(family.as_str());
        }
    }
    families
}

/// Best available verifier, or `None` if none is eligible.
///
/// Ranked by: family differing from every family already used, then higher
/// measured sensitivity, with an unmeasured (`None`) sensitivity ranking below
/// any measured value. Ties are broken by roster order.
fn pick_verifier(ev: &Evidence, cfg: &RouterConfig, used: &[&str]) -> Option<VerifierId> {
    let mut best: Option<(VerifierId, bool, Option<f64>)> = None;
    for (v, family, stats) in &cfg.roster {
        if ev.excluded.contains(v) {
            continue;
        }
        let differs = !used.contains(&family.as_str());
        let cand = (v.clone(), differs, stats.sensitivity());
        let take = match &best {
            None => true,
            Some((_, best_differs, best_sens)) => {
                differs != *best_differs || (differs == *best_differs && beats(cand.2, *best_sens))
            }
        };
        if take {
            best = Some(cand);
        }
    }
    best.map(|(v, _, _)| v)
}

/// True when `a` should outrank `b` as a sensitivity: any measured value beats
/// an unmeasured one, and ties do not displace the incumbent.
fn beats(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x > y,
        (Some(_), None) => true,
        (None, Some(_)) | (None, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid(s: &str) -> VerifierId {
        VerifierId(String::from(s))
    }

    fn family(s: &str) -> TaskFamily {
        TaskFamily(String::from(s))
    }

    fn clean() -> Finding {
        Finding::NoDefectFound
    }

    fn defect(detail: &str) -> Finding {
        Finding::DemonstratedDefect {
            detail: String::from(detail),
        }
    }

    fn suspect(detail: &str) -> Finding {
        Finding::SuspectedDefect {
            detail: String::from(detail),
        }
    }

    fn ambiguous(detail: &str) -> Finding {
        Finding::SpecAmbiguous {
            detail: String::from(detail),
        }
    }

    fn unable(reason: &str) -> Finding {
        Finding::UnableToAssess {
            reason: String::from(reason),
        }
    }

    fn stats(sens_num: u32, sens_den: u32) -> VerifierStats {
        VerifierStats {
            rejected_defective: sens_num,
            missed_defective: sens_den.saturating_sub(sens_num),
            approved_acceptable: 10,
            rejected_acceptable: 0,
            abstained: 0,
            findings_reported: 0,
            findings_confirmed: 0,
        }
    }

    fn cfg_with(roster: Vec<(VerifierId, String, VerifierStats)>) -> RouterConfig {
        RouterConfig {
            review_budget: 100,
            cost_per_check: 10,
            min_n_for_calibration: 8,
            roster,
        }
    }

    fn ev_with(completed: Vec<(Check, Finding)>, excluded: Vec<VerifierId>) -> Evidence {
        Evidence {
            family: family("refactor"),
            completed,
            excluded,
            spent: 0,
        }
    }

    // ---- decide -----------------------------------------------------------

    #[test]
    fn decide_rejects_on_any_demonstrated_defect() {
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (
                    Check::BehaviouralVerifier(vid("b")),
                    defect("panics on empty input"),
                ),
            ],
            vec![],
        );
        assert_eq!(decide(&ev), Decision::Reject);
    }

    #[test]
    fn decide_spec_ambiguous_outranks_demonstrated_defect() {
        let ev = ev_with(
            vec![
                (
                    Check::BehaviouralVerifier(vid("b")),
                    defect("sorts descending"),
                ),
                (
                    Check::CodeReview(vid("c")),
                    ambiguous("\"sorted\" order unspecified"),
                ),
            ],
            vec![],
        );
        assert_eq!(decide(&ev), Decision::Undetermined);
    }

    #[test]
    fn decide_spec_ambiguous_alone_is_undetermined() {
        let ev = ev_with(
            vec![(
                Check::CrossExamination,
                ambiguous("no stated rounding rule"),
            )],
            vec![],
        );
        assert_eq!(decide(&ev), Decision::Undetermined);
    }

    #[test]
    fn decide_accepts_clean_gate_plus_second_clean_check() {
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::BehaviouralVerifier(vid("b")), clean()),
            ],
            vec![],
        );
        assert_eq!(decide(&ev), Decision::Accept);
    }

    #[test]
    fn decide_does_not_accept_on_a_clean_gate_alone() {
        let ev = ev_with(vec![(Check::ExecutedGate, clean())], vec![]);
        assert_eq!(decide(&ev), Decision::Undetermined);
    }

    #[test]
    fn decide_does_not_accept_two_clean_checks_without_the_gate() {
        let ev = ev_with(
            vec![
                (Check::BehaviouralVerifier(vid("b")), clean()),
                (Check::CodeReview(vid("c")), clean()),
            ],
            vec![],
        );
        assert_ne!(decide(&ev), Decision::Accept);
    }

    #[test]
    fn decide_repairs_on_suspicion_without_demonstration() {
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (
                    Check::CodeReview(vid("c")),
                    suspect("off-by-one at the edge"),
                ),
            ],
            vec![],
        );
        assert_eq!(decide(&ev), Decision::Repair);
    }

    #[test]
    fn decide_does_not_count_inability_to_assess_as_a_clean_check() {
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (
                    Check::BehaviouralVerifier(vid("b")),
                    unable("sandbox refused to run it"),
                ),
            ],
            vec![],
        );
        assert!(ev.completed[1].1.is_unable_to_assess());
        assert_eq!(decide(&ev), Decision::Undetermined);
    }

    #[test]
    fn decide_undetermined_when_nothing_conclusive() {
        let ev = ev_with(
            vec![(Check::CrossExamination, unable("no toolchain"))],
            vec![],
        );
        assert_eq!(decide(&ev), Decision::Undetermined);
    }

    #[test]
    fn decide_undetermined_on_empty_evidence() {
        assert_eq!(decide(&ev_with(vec![], vec![])), Decision::Undetermined);
    }

    // ---- next_check: stopping --------------------------------------------

    #[test]
    fn next_check_stops_when_the_budget_is_exhausted() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let mut ev = ev_with(vec![], vec![]);
        ev.spent = 95;
        assert_eq!(next_check(&ev, &cfg), None);
    }

    #[test]
    fn next_check_runs_when_one_more_check_still_fits() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let mut ev = ev_with(vec![], vec![]);
        ev.spent = 90;
        assert_eq!(next_check(&ev, &cfg), Some(Check::ExecutedGate));
    }

    #[test]
    fn next_check_stops_once_a_defect_is_demonstrated() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let ev = ev_with(vec![(Check::ExecutedGate, defect("gate fails"))], vec![]);
        assert_eq!(decide(&ev), Decision::Reject);
        assert_eq!(next_check(&ev, &cfg), None);
    }

    #[test]
    fn next_check_stops_when_the_spec_is_ambiguous() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let ev = ev_with(
            vec![(Check::ExecutedGate, ambiguous("no oracle given"))],
            vec![],
        );
        assert_eq!(next_check(&ev, &cfg), None);
    }

    #[test]
    fn next_check_continues_when_undetermined_for_any_other_reason() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let ev = ev_with(vec![(Check::ExecutedGate, unable("no toolchain"))], vec![]);
        assert_eq!(decide(&ev), Decision::Undetermined);
        assert_eq!(next_check(&ev, &cfg), Some(Check::CrossExamination));
    }

    // ---- next_check: the ladder ------------------------------------------

    #[test]
    fn next_check_walks_the_ladder_in_order() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let mut ev = ev_with(vec![], vec![]);
        let mut seen: Vec<Check> = Vec::new();
        for _ in 0..10 {
            match next_check(&ev, &cfg) {
                Some(next) => {
                    ev.completed.push((next.clone(), clean()));
                    seen.push(next);
                }
                None => break,
            }
        }
        let kinds: Vec<bool> = seen
            .iter()
            .map(|c| {
                matches!(c, Check::BehaviouralVerifier(_)) || matches!(c, Check::CodeReview(_))
            })
            .collect();
        assert_eq!(seen.first(), Some(&Check::ExecutedGate));
        assert_eq!(seen.get(1), Some(&Check::CrossExamination));
        assert!(
            kinds.get(2) == Some(&true),
            "third check is verifier-backed"
        );
        assert!(
            kinds.get(3) == Some(&true),
            "fourth check is verifier-backed"
        );
        assert!(
            !seen
                .iter()
                .any(|c| matches!(c, Check::TargetedReproduction { .. })),
            "no suspicion means no reproduction"
        );
        assert_eq!(seen.get(4), Some(&Check::DeputyDiscriminatingTest));
        assert_eq!(seen.get(5), Some(&Check::OrchestratorAudit));
        assert_eq!(seen.len(), 6);
        assert_eq!(next_check(&ev, &cfg), None, "ladder exhausted");
    }

    #[test]
    fn next_check_skips_checks_already_completed() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
            ],
            vec![],
        );
        assert_eq!(
            next_check(&ev, &cfg),
            Some(Check::BehaviouralVerifier(vid("b")))
        );
    }

    #[test]
    fn next_check_never_picks_a_verifier_in_the_excluded_list() {
        let cfg = cfg_with(vec![
            (vid("implementer"), String::from("fam-a"), stats(9, 10)),
            (vid("used-already"), String::from("fam-b"), stats(8, 10)),
            (vid("fresh"), String::from("fam-c"), stats(7, 10)),
        ]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
            ],
            vec![vid("implementer"), vid("used-already")],
        );
        let picked = next_check(&ev, &cfg);
        assert_eq!(picked, Some(Check::BehaviouralVerifier(vid("fresh"))));
        // Still true after the behavioural step is spent: only "fresh" may review.
        let mut ev2 = ev;
        ev2.completed
            .push((Check::BehaviouralVerifier(vid("fresh")), clean()));
        assert_eq!(
            next_check(&ev2, &cfg),
            Some(Check::CodeReview(vid("fresh")))
        );
    }

    #[test]
    fn next_check_prefers_a_verifier_from_a_different_model_family() {
        let cfg = cfg_with(vec![
            (vid("used"), String::from("fam-a"), stats(9, 10)),
            (vid("same-family"), String::from("fam-a"), stats(5, 10)),
            (vid("other-family"), String::from("fam-b"), stats(5, 10)),
        ]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
                (Check::BehaviouralVerifier(vid("used")), clean()),
            ],
            vec![vid("used")],
        );
        assert_eq!(
            next_check(&ev, &cfg),
            Some(Check::CodeReview(vid("other-family")))
        );
    }

    #[test]
    fn next_check_falls_back_to_a_same_family_verifier_when_none_differs() {
        let cfg = cfg_with(vec![
            (vid("used"), String::from("fam-a"), stats(9, 10)),
            (vid("only-other"), String::from("fam-a"), stats(4, 10)),
        ]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
                (Check::BehaviouralVerifier(vid("used")), clean()),
            ],
            vec![vid("used")],
        );
        assert_eq!(
            next_check(&ev, &cfg),
            Some(Check::CodeReview(vid("only-other")))
        );
    }

    #[test]
    fn next_check_ranks_unmeasured_sensitivity_below_any_measured_value() {
        let sharp = VerifierStats {
            rejected_defective: 1,
            missed_defective: 9,
            ..VerifierStats::default()
        };
        let never_saw_a_defect = VerifierStats {
            approved_acceptable: 40,
            ..VerifierStats::default()
        };
        assert_eq!(sharp.sensitivity(), Some(0.1));
        assert_eq!(never_saw_a_defect.sensitivity(), None);

        let cfg = cfg_with(vec![
            (vid("unmeasured"), String::from("fam-z"), never_saw_a_defect),
            (vid("measured"), String::from("fam-z"), sharp),
        ]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
            ],
            vec![],
        );
        assert_eq!(
            next_check(&ev, &cfg),
            Some(Check::BehaviouralVerifier(vid("measured")))
        );
    }

    #[test]
    fn next_check_prefers_higher_sensitivity_among_otherwise_equal_verifiers() {
        let cfg = cfg_with(vec![
            (vid("weak"), String::from("fam-a"), stats(2, 10)),
            (vid("strong"), String::from("fam-a"), stats(9, 10)),
        ]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
            ],
            vec![],
        );
        assert_eq!(
            next_check(&ev, &cfg),
            Some(Check::BehaviouralVerifier(vid("strong")))
        );
    }

    #[test]
    fn next_check_targets_reproduction_at_the_first_suspicion() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
                (
                    Check::BehaviouralVerifier(vid("b")),
                    suspect("drops the last row"),
                ),
                (Check::CodeReview(vid("b")), suspect("leaks a handle")),
            ],
            vec![],
        );
        assert_eq!(
            next_check(&ev, &cfg),
            Some(Check::TargetedReproduction {
                claim: String::from("drops the last row"),
            })
        );
    }

    #[test]
    fn next_check_does_not_reproduce_without_a_suspicion() {
        let cfg = cfg_with(vec![(vid("b"), String::from("fam-a"), stats(9, 10))]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, unable("no toolchain")),
                (Check::CrossExamination, clean()),
                (Check::BehaviouralVerifier(vid("b")), clean()),
                (Check::CodeReview(vid("b")), clean()),
            ],
            vec![],
        );
        assert_eq!(next_check(&ev, &cfg), Some(Check::DeputyDiscriminatingTest));
    }

    #[test]
    fn next_check_with_an_empty_roster_falls_through_to_verifier_free_checks() {
        let cfg = cfg_with(vec![]);
        let ev = ev_with(
            vec![
                (Check::ExecutedGate, clean()),
                (Check::CrossExamination, clean()),
            ],
            vec![],
        );
        assert_eq!(next_check(&ev, &cfg), Some(Check::DeputyDiscriminatingTest));
    }

    #[test]
    fn next_check_survives_an_empty_roster_and_zero_budget_without_stopping_early_or_panicking() {
        let cfg = RouterConfig {
            review_budget: 0,
            cost_per_check: 0,
            min_n_for_calibration: 0,
            roster: vec![],
        };
        let ev = ev_with(vec![], vec![]);
        assert_eq!(next_check(&ev, &cfg), Some(Check::ExecutedGate));
        assert!(is_shadow(&vid("nobody"), &cfg));
    }

    // ---- VerifierStats ----------------------------------------------------

    #[test]
    fn default_stats_return_none_rather_than_dividing_by_zero() {
        let s = VerifierStats::default();
        assert_eq!(s.sensitivity(), None);
        assert_eq!(s.specificity(), None);
        assert_eq!(s.precision(), None);
        assert_eq!(s.n(), 0);
        assert!(!s.is_calibrated(0));
    }

    #[test]
    fn every_accessor_is_none_on_stats_with_no_adjudicated_cases() {
        let s = VerifierStats {
            findings_reported: 5,
            ..VerifierStats::default()
        };
        assert_eq!(s.sensitivity(), None, "no defective cases seen");
        assert_eq!(s.specificity(), None, "no acceptable cases seen");
        assert_eq!(s.precision(), Some(0.0), "five reported, none confirmed");
        assert!(!s.is_calibrated(1));
    }

    #[test]
    fn rates_are_computed_over_the_cases_actually_seen() {
        let s = VerifierStats {
            rejected_defective: 3,
            missed_defective: 1,
            approved_acceptable: 6,
            rejected_acceptable: 2,
            abstained: 4,
            findings_reported: 8,
            findings_confirmed: 6,
        };
        assert_eq!(s.sensitivity(), Some(0.75), "3 of 4 defective caught");
        assert_eq!(s.specificity(), Some(0.75), "6 of 8 acceptable let through");
        assert_eq!(s.precision(), Some(0.75), "6 of 8 findings confirmed");
        assert_eq!(s.n(), 16);
    }

    #[test]
    fn a_verifier_that_approves_everything_is_exposed_by_its_sensitivity() {
        let s = VerifierStats {
            rejected_defective: 0,
            missed_defective: 5,
            approved_acceptable: 95,
            rejected_acceptable: 0,
            abstained: 0,
            findings_reported: 0,
            findings_confirmed: 0,
        };
        assert_eq!(s.specificity(), Some(1.0), "looks perfect on its own");
        assert_eq!(s.sensitivity(), Some(0.0), "but catches nothing");
    }

    #[test]
    fn calibration_needs_both_enough_observations_and_a_defective_case() {
        let saw_defects = VerifierStats {
            rejected_defective: 2,
            missed_defective: 2,
            approved_acceptable: 2,
            rejected_acceptable: 2,
            abstained: 2,
            findings_reported: 0,
            findings_confirmed: 0,
        };
        assert!(saw_defects.is_calibrated(10));
        assert!(!saw_defects.is_calibrated(11), "too few observations");

        let only_good_work = VerifierStats {
            approved_acceptable: 50,
            ..VerifierStats::default()
        };
        assert!(!only_good_work.is_calibrated(10), "sensitivity unmeasured");
    }

    #[test]
    fn counters_at_the_maximum_do_not_overflow() {
        let s = VerifierStats {
            rejected_defective: u32::MAX,
            missed_defective: u32::MAX,
            approved_acceptable: u32::MAX,
            rejected_acceptable: u32::MAX,
            abstained: u32::MAX,
            findings_reported: u32::MAX,
            findings_confirmed: u32::MAX,
        };
        assert_eq!(s.n(), u32::MAX, "saturates rather than wrapping");
        assert!(s.sensitivity().is_some());
        assert!(s.is_calibrated(u32::MAX));
    }

    // ---- is_shadow --------------------------------------------------------

    #[test]
    fn a_verifier_absent_from_the_roster_is_shadow() {
        let cfg = cfg_with(vec![(vid("known"), String::from("fam-a"), stats(9, 10))]);
        assert!(is_shadow(&vid("stranger"), &cfg));
    }

    #[test]
    fn an_uncalibrated_verifier_on_the_roster_is_shadow() {
        let uncalibrated = VerifierStats {
            rejected_defective: 1,
            missed_defective: 1,
            approved_acceptable: 1,
            ..VerifierStats::default()
        };
        let cfg = cfg_with(vec![(vid("rookie"), String::from("fam-a"), uncalibrated)]);
        assert!(is_shadow(&vid("rookie"), &cfg), "n=3 < min_n=8");
    }

    #[test]
    fn a_calibrated_verifier_is_not_shadow() {
        let cfg = cfg_with(vec![(vid("veteran"), String::from("fam-a"), stats(9, 10))]);
        assert!(!is_shadow(&vid("veteran"), &cfg));
    }

    #[test]
    fn a_verifier_with_perfect_accuracy_but_no_defective_cases_is_shadow() {
        let cfg = cfg_with(vec![(
            vid("yes-man"),
            String::from("fam-a"),
            VerifierStats {
                approved_acceptable: 100,
                ..VerifierStats::default()
            },
        )]);
        assert!(is_shadow(&vid("yes-man"), &cfg));
    }

    // ---- plumbing ---------------------------------------------------------

    #[test]
    fn check_kinds_ignore_which_verifier_or_claim_they_carry() {
        let a = Check::BehaviouralVerifier(vid("one"));
        let b = Check::BehaviouralVerifier(vid("two"));
        assert!(a.same_kind(&b));
        assert!(!a.same_kind(&Check::CodeReview(vid("one"))));
        assert!(
            Check::TargetedReproduction {
                claim: String::from("x")
            }
            .same_kind(&Check::TargetedReproduction {
                claim: String::from("y")
            })
        );
        assert_eq!(a.verifier(), Some(&vid("one")));
        assert_eq!(Check::ExecutedGate.verifier(), None);
    }

    #[test]
    fn evidence_and_config_round_trip_through_json() {
        let ev = ev_with(
            vec![(Check::CodeReview(vid("c")), suspect("unbounded retry"))],
            vec![vid("implementer")],
        );
        let cfg = cfg_with(vec![(vid("c"), String::from("fam-a"), stats(7, 10))]);

        let ev_back: Evidence = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        let cfg_back: RouterConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(ev, ev_back);
        assert_eq!(cfg, cfg_back);
        assert_eq!(next_check(&ev_back, &cfg_back), next_check(&ev, &cfg));
    }
}
