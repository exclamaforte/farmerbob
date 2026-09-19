//! The gate an escalated finding must pass to join the permanent suite.
//!
//! `fb-escalate.sh` promotes a confirmed finding into the task's permanent
//! conformance suite, so each round sharpens the gate that judges every future
//! candidate. Two disciplines decide whether a finding is allowed through:
//!
//! * The reference veto. An escalated test must PASS against the merged
//!   reference. A test that fails there encodes the critic's misreading of the
//!   specification rather than a defect in any candidate, and admitting it
//!   would fail every correct implementation forever.
//! * Provenance. Every escalated test records the critic that found it, the
//!   candidate it was found on, and the task, so a bad test is traceable and
//!   retirable and a good one is creditable.
//!
//! The decision is pure. The caller runs the tests and owns the file I/O
//! around the suite; the reference outcome arrives here as a value. Whether a
//! claim was confirmed in the first place is also the caller's rule (see
//! `crate::prove_veto`); this module never re-derives it.

use std::collections::BTreeMap;

/// A finding that has been confirmed against the candidate it was made about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The arm that made the claim. Non-empty is not enforced by the type.
    pub critic: String,
    /// The arm the claim was made about.
    pub subject: String,
    /// The task it was found on.
    pub task: String,
}

/// What happened when the finding's test was run against the merged reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// It ran and PASSED. The test does not indict correct code.
    Passed,
    /// It ran and FAILED. The test indicts the reference, so it encodes a
    /// misreading rather than a defect. Carries the reference's failure output.
    Failed(String),
    /// It could not be run. Carries a non-empty reason.
    NotRun(String),
}

/// Whether a finding may join the permanent suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Escalate it, with this provenance.
    Escalate(Provenance),
    /// Do not escalate: the reference fails it. Carries the reference output.
    Vetoed(String),
    /// Do not escalate: the reference was not run, so the veto never happened.
    /// An unrun veto is NOT a passed veto. Carries the reason.
    Unverified(String),
    /// Do not escalate: this finding is already in the suite.
    AlreadyPresent,
}

/// What an escalated test records about where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The arm that made the claim.
    pub critic: String,
    /// The arm the claim was made about.
    pub subject: String,
    /// The task it was found on.
    pub task: String,
}

/// Whether a recorded provenance and a finding name the same claim: all three
/// fields must match together.
fn same_finding(p: &Provenance, f: &Finding) -> bool {
    p.critic == f.critic && p.subject == f.subject && p.task == f.task
}

/// Decide one finding from the reference outcome alone. Presence has already
/// been ruled out by the caller.
fn from_reference(f: &Finding, r: &Reference) -> Admit {
    match r {
        Reference::Passed => Admit::Escalate(Provenance {
            critic: f.critic.clone(),
            subject: f.subject.clone(),
            task: f.task.clone(),
        }),
        Reference::Failed(output) => Admit::Vetoed(output.clone()),
        Reference::NotRun(reason) => Admit::Unverified(reason.clone()),
    }
}

/// Decide whether one confirmed finding joins the suite.
///
/// `present` is the provenance already recorded in the suite, in any order.
/// A finding already recorded is `AlreadyPresent`, whatever the reference says.
pub fn admit(f: &Finding, r: &Reference, present: &[Provenance]) -> Admit {
    if present.iter().any(|p| same_finding(p, f)) {
        Admit::AlreadyPresent
    } else {
        from_reference(f, r)
    }
}

/// The provenance a set of admitted findings adds to the suite.
///
/// Escalation is IDEMPOTENT: running it twice over the same findings adds
/// nothing the second time.
///
/// Returns the provenance of exactly the findings that were escalated, in
/// input order, and a `(index, Admit)` refusal for every finding that was not,
/// also in input order. The two parts partition the input.
pub fn escalate(
    findings: &[(Finding, Reference)],
    present: &[Provenance],
) -> (Vec<Provenance>, Vec<(usize, Admit)>) {
    let mut escalated = Vec::new();
    let mut rejected = Vec::new();

    for (index, (finding, reference)) in findings.iter().enumerate() {
        // Each occurrence sees the incoming suite plus whatever earlier
        // occurrences in this same call added, so a finding duplicated in the
        // input cannot be escalated twice.
        let in_suite = present
            .iter()
            .chain(escalated.iter())
            .any(|p| same_finding(p, finding));
        let outcome = if in_suite {
            Admit::AlreadyPresent
        } else {
            from_reference(finding, reference)
        };
        match outcome {
            Admit::Escalate(provenance) => escalated.push(provenance),
            other => rejected.push((index, other)),
        }
    }

    (escalated, rejected)
}

/// Credit, per critic, for findings that were escalated.
///
/// Keys are critic names; the value is how many of that critic's findings
/// were escalated. A critic with none does not appear.
pub fn credit(escalated: &[Provenance]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for provenance in escalated {
        *counts.entry(provenance.critic.clone()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(critic: &str, subject: &str, task: &str) -> Finding {
        Finding {
            critic: critic.to_string(),
            subject: subject.to_string(),
            task: task.to_string(),
        }
    }

    fn provenance(critic: &str, subject: &str, task: &str) -> Provenance {
        Provenance {
            critic: critic.to_string(),
            subject: subject.to_string(),
            task: task.to_string(),
        }
    }

    // Clauses 1, 3 and 4 together: from ONE finding, Passed escalates with the
    // finding's fields unchanged and NotRun refuses, and the two outcomes
    // differ. An implementation that escalates whenever the reference did not
    // explicitly fail returns Escalate for both and fails the match below.
    #[test]
    fn not_run_is_not_a_passed_veto() {
        let f = finding("critic-a", "candidate-x", "task-7");
        let why = "reference build broke";
        match (
            admit(&f, &Reference::Passed, &[]),
            admit(&f, &Reference::NotRun(why.to_string()), &[]),
        ) {
            (Admit::Escalate(p), Admit::Unverified(carried)) => {
                assert_eq!(p.critic, "critic-a");
                assert_eq!(p.subject, "candidate-x");
                assert_eq!(p.task, "task-7");
                assert_eq!(carried, why);
            }
            _ => panic!("clauses 1 and 3: Passed escalates, NotRun refuses, and they differ"),
        }
    }

    // Clause 2: the reference veto, carrying the output unchanged.
    #[test]
    fn failed_reference_vetoes_with_output_unchanged() {
        let f = finding("critic-a", "candidate-x", "task-7");
        let out = "test sensitivity_output ... FAILED";
        match admit(&f, &Reference::Failed(out.to_string()), &[]) {
            Admit::Vetoed(carried) => assert_eq!(carried, out),
            _ => panic!("clause 2: a failed reference vetoes the finding"),
        }
    }

    // Clause 5: presence wins whatever the reference says. Passed is the case
    // where a permissive implementation would escalate a duplicate; Failed and
    // NotRun pin the "whatever" qualifier.
    #[test]
    fn presence_wins_whatever_the_reference_says() {
        let f = finding("critic-a", "candidate-x", "task-7");
        let suite = vec![provenance("critic-a", "candidate-x", "task-7")];
        assert!(
            matches!(admit(&f, &Reference::Passed, &suite), Admit::AlreadyPresent),
            "clause 5: a present finding is AlreadyPresent even on Passed"
        );
        assert!(
            matches!(
                admit(&f, &Reference::Failed("out".to_string()), &suite),
                Admit::AlreadyPresent
            ),
            "clause 5: presence wins over a failed reference"
        );
        assert!(
            matches!(
                admit(&f, &Reference::NotRun("why".to_string()), &suite),
                Admit::AlreadyPresent
            ),
            "clause 5: presence wins over an unrun reference"
        );
    }

    // Clause 6: matching is on all THREE fields together. One case per field.
    #[test]
    fn matching_requires_all_three_fields() {
        let suite = vec![provenance("critic-a", "candidate-x", "task-7")];

        let f = finding("critic-a", "candidate-y", "task-7");
        assert!(
            matches!(admit(&f, &Reference::Passed, &suite), Admit::Escalate(_)),
            "clause 6: same critic and task with a different subject is a different finding"
        );

        let f = finding("critic-a", "candidate-x", "task-8");
        assert!(
            matches!(admit(&f, &Reference::Passed, &suite), Admit::Escalate(_)),
            "clause 6: same critic and subject with a different task is a different finding"
        );

        let f = finding("critic-b", "candidate-x", "task-7");
        assert!(
            matches!(admit(&f, &Reference::Passed, &suite), Admit::Escalate(_)),
            "clause 6: same subject and task with a different critic is a different finding"
        );
    }

    // Clause 7 and the partition: a mixed input producing one of each Admit
    // variant. The parts partition the input (lengths sum), escalated carries
    // the provenance unchanged, and the rejection entries carry indexes into
    // the input, in input order. The fourth element duplicates the first
    // within the call, which is clause 9's AlreadyPresent.
    #[test]
    fn mixed_input_partitions_with_rejection_indexes_in_input_order() {
        let input = vec![
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Passed,
            ),
            (
                finding("critic-b", "candidate-y", "task-7"),
                Reference::Failed("boom".to_string()),
            ),
            (
                finding("critic-c", "candidate-z", "task-7"),
                Reference::NotRun("reference build broke".to_string()),
            ),
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Passed,
            ),
        ];
        let (escalated, rejected) = escalate(&input, &[]);

        assert_eq!(
            escalated.len() + rejected.len(),
            input.len(),
            "partition: every finding is in exactly one part"
        );
        assert_eq!(escalated.len(), 1);
        assert_eq!(escalated[0].critic, "critic-a");
        assert_eq!(escalated[0].subject, "candidate-x");
        assert_eq!(escalated[0].task, "task-7");

        assert_eq!(rejected.len(), 3);
        assert_eq!(rejected[0].0, 1);
        assert_eq!(rejected[1].0, 2);
        assert_eq!(rejected[2].0, 3);
        match &rejected[0].1 {
            Admit::Vetoed(out) => assert_eq!(out, "boom"),
            _ => panic!("clause 2 via escalate: index 1 must be Vetoed"),
        }
        match &rejected[1].1 {
            Admit::Unverified(why) => assert_eq!(why, "reference build broke"),
            _ => panic!("clause 3 via escalate: index 2 must be Unverified"),
        }
        assert!(
            matches!(rejected[2].1, Admit::AlreadyPresent),
            "clause 9 via escalate: index 3 duplicates index 0 in the input"
        );
    }

    // Clause 8: idempotency. Feeding escalate's own output back as present
    // with the same findings adds nothing the second time.
    #[test]
    fn feeding_its_own_output_back_adds_nothing() {
        let input = vec![
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Passed,
            ),
            (
                finding("critic-b", "candidate-y", "task-7"),
                Reference::Failed("boom".to_string()),
            ),
            (
                finding("critic-c", "candidate-z", "task-7"),
                Reference::NotRun("reference build broke".to_string()),
            ),
        ];
        let (escalated, _) = escalate(&input, &[]);
        assert_eq!(escalated.len(), 1);

        let (added_again, refused) = escalate(&input, &escalated);
        assert!(
            added_again.is_empty(),
            "clause 8: re-escalating over its own output must add nothing"
        );
        assert_eq!(refused.len(), input.len());
    }

    // Clause 9: a finding duplicated in the input is escalated once, and the
    // second occurrence is AlreadyPresent. An implementation that only
    // consults the incoming present escalates it twice and fails the length.
    #[test]
    fn a_duplicate_in_one_call_is_escalated_once() {
        let input = vec![
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Passed,
            ),
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Passed,
            ),
        ];
        let (escalated, rejected) = escalate(&input, &[]);
        assert_eq!(escalated.len(), 1, "clause 9: escalated once, not twice");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, 1);
        assert!(
            matches!(rejected[0].1, Admit::AlreadyPresent),
            "clause 9: the second occurrence is AlreadyPresent"
        );
    }

    // Clause 7: both parts follow the input order, with two escalations
    // separated by a refusal.
    #[test]
    fn both_parts_follow_the_input_order() {
        let input = vec![
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Passed,
            ),
            (
                finding("critic-b", "candidate-y", "task-7"),
                Reference::Failed("boom".to_string()),
            ),
            (
                finding("critic-c", "candidate-z", "task-7"),
                Reference::Passed,
            ),
        ];
        let (escalated, rejected) = escalate(&input, &[]);
        assert_eq!(escalated.len(), 2);
        assert_eq!(escalated[0].subject, "candidate-x");
        assert_eq!(escalated[1].subject, "candidate-z");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, 1);
    }

    // Boundary at zero: no findings, no credit; and an empty suite is not a
    // reason to refuse.
    #[test]
    fn zero_findings_produce_zero_output() {
        let (escalated, rejected) = escalate(&[], &[]);
        assert!(escalated.is_empty());
        assert!(rejected.is_empty());
        assert!(credit(&[]).is_empty());
    }

    #[test]
    fn an_empty_suite_still_admits() {
        let input = vec![(
            finding("critic-a", "candidate-x", "task-7"),
            Reference::Passed,
        )];
        let (escalated, rejected) = escalate(&input, &[]);
        assert_eq!(escalated.len(), 1);
        assert!(rejected.is_empty());
        assert_eq!(escalated[0].critic, "critic-a");
        assert_eq!(escalated[0].subject, "candidate-x");
        assert_eq!(escalated[0].task, "task-7");
    }

    // Boundary: one vetoed finding, and all findings vetoed. The escalated
    // vector must be EMPTY, not merely short.
    #[test]
    fn one_vetoed_finding_leaves_one_rejection_at_index_zero() {
        let input = vec![(
            finding("critic-a", "candidate-x", "task-7"),
            Reference::Failed("boom".to_string()),
        )];
        let (escalated, rejected) = escalate(&input, &[]);
        assert!(escalated.is_empty());
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, 0);
        match &rejected[0].1 {
            Admit::Vetoed(out) => assert_eq!(out, "boom"),
            _ => panic!("a single vetoed finding is refused at index 0"),
        }
    }

    #[test]
    fn all_vetoed_escalates_nothing() {
        let input = vec![
            (
                finding("critic-a", "candidate-x", "task-7"),
                Reference::Failed("first".to_string()),
            ),
            (
                finding("critic-b", "candidate-y", "task-7"),
                Reference::Failed("second".to_string()),
            ),
        ];
        let (escalated, rejected) = escalate(&input, &[]);
        assert!(escalated.is_empty(), "must be EMPTY, not merely short");
        assert_eq!(rejected.len(), input.len());
    }

    // Clause 10: credit counts each provenance under its critic, omits
    // critics with none, and its values sum to the escalated vector's length.
    #[test]
    fn credit_counts_per_critic_and_omits_the_absent() {
        let escalated = vec![
            provenance("critic-a", "candidate-x", "task-7"),
            provenance("critic-a", "candidate-y", "task-7"),
            provenance("critic-b", "candidate-z", "task-7"),
        ];
        let counts = credit(&escalated);
        assert_eq!(counts.len(), 2);
        assert_eq!(counts.get("critic-a"), Some(&2));
        assert_eq!(counts.get("critic-b"), Some(&1));
        assert_eq!(counts.get("critic-nobody"), None);
        assert_eq!(counts.values().sum::<usize>(), escalated.len());
    }
}
