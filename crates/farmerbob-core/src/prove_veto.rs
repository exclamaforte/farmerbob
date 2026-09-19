//! Decide whether a reproduced claim survives comparison with the merged reference.
//!
//! A subject failure is only a confirmed claim when the same test passes against
//! the reference. If no reference exists, the result remains provisional so a
//! summary cannot present it as an independently confirmed claim.

/// What happened when the claim's test was run against the implementation it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// The test failed there: the claimed defect reproduces.
    Failed,
    /// The test passed there: the claim does not reproduce.
    Passed,
    /// The test could not be run at all.
    NotRun(String),
}

/// What happened when the same test was run against the merged reference at HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// It ran and the test passed: the reference is clean, so a subject failure is real.
    Clean,
    /// It ran and the test also failed: the test indicts the reference too.
    AlsoFailed,
    /// There is no reference to run against.
    Unavailable(String),
}

/// The fate of one claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// Subject failed and the reference was clean. This is the strongest verdict available.
    Confirmed,
    /// Subject failed and the reference was unavailable.
    Provisional(String),
    /// Subject failed and the reference failed too. The test is not about this candidate.
    Vetoed,
    /// Subject passed. The claim does not reproduce.
    Refuted,
    /// The subject run did not happen.
    Unproved(String),
}

/// Decide one claim's fate from the two runs.
pub fn fate(subject: &Subject, reference: &Reference) -> Fate {
    match subject {
        Subject::Failed => match reference {
            Reference::Clean => Fate::Confirmed,
            Reference::AlsoFailed => Fate::Vetoed,
            Reference::Unavailable(reason) => Fate::Provisional(reason.clone()),
        },
        Subject::Passed => Fate::Refuted,
        Subject::NotRun(reason) => Fate::Unproved(reason.clone()),
    }
}

/// The counts a run of many claims produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Number of claims confirmed against a clean reference.
    pub confirmed: usize,
    /// Number of claims whose subject failed without an available reference.
    pub provisional: usize,
    /// Number of claims vetoed because the reference also failed.
    pub vetoed: usize,
    /// Number of claims whose subject passed.
    pub refuted: usize,
    /// Number of claims whose subject run did not happen.
    pub unproved: usize,
}

/// Tally a set of fates, counting each fate in exactly one bucket.
pub fn tally(fates: &[Fate]) -> Tally {
    let mut tally = Tally {
        confirmed: 0,
        provisional: 0,
        vetoed: 0,
        refuted: 0,
        unproved: 0,
    };

    for fate in fates {
        match fate {
            Fate::Confirmed => tally.confirmed += 1,
            Fate::Provisional(_) => tally.provisional += 1,
            Fate::Vetoed => tally.vetoed += 1,
            Fate::Refuted => tally.refuted += 1,
            Fate::Unproved(_) => tally.unproved += 1,
        }
    }

    tally
}

/// Whether any claim in the tally rests on a reference that was never run.
pub fn any_provisional(tally: &Tally) -> bool {
    tally.provisional > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_subject_uses_each_reference_outcome() {
        assert_eq!(fate(&Subject::Failed, &Reference::Clean), Fate::Confirmed);
        assert_eq!(fate(&Subject::Failed, &Reference::AlsoFailed), Fate::Vetoed);

        let reason = "HEAD has no reference".to_string();
        assert_eq!(
            fate(&Subject::Failed, &Reference::Unavailable(reason.clone()),),
            Fate::Provisional(reason),
        );
    }

    #[test]
    fn confirmed_and_provisional_are_distinct_for_the_same_failed_subject() {
        let confirmed = fate(&Subject::Failed, &Reference::Clean);
        let provisional = fate(
            &Subject::Failed,
            &Reference::Unavailable("no HEAD reference".to_string()),
        );

        assert_ne!(confirmed, provisional);
    }

    #[test]
    fn passed_subject_is_refuted_for_every_reference_outcome() {
        for reference in [
            Reference::Clean,
            Reference::AlsoFailed,
            Reference::Unavailable("not available".to_string()),
        ] {
            assert_eq!(fate(&Subject::Passed, &reference), Fate::Refuted);
        }
    }

    #[test]
    fn not_run_subject_is_unproved_for_every_reference_outcome() {
        let reason = "subject build failed".to_string();
        for reference in [
            Reference::Clean,
            Reference::AlsoFailed,
            Reference::Unavailable("not available".to_string()),
        ] {
            assert_eq!(
                fate(&Subject::NotRun(reason.clone()), &reference),
                Fate::Unproved(reason.clone()),
            );
        }
    }

    #[test]
    fn empty_reasons_are_preserved_without_extra_validation() {
        assert_eq!(
            fate(&Subject::Failed, &Reference::Unavailable(String::new()),),
            Fate::Provisional(String::new()),
        );
        assert_eq!(
            fate(&Subject::NotRun(String::new()), &Reference::Clean),
            Fate::Unproved(String::new()),
        );
    }

    #[test]
    fn tally_partitions_a_mixed_run() {
        let fates = [
            Fate::Confirmed,
            Fate::Provisional("no reference".to_string()),
            Fate::Vetoed,
            Fate::Refuted,
            Fate::Unproved("did not run".to_string()),
        ];
        let result = tally(&fates);

        assert_eq!(result.confirmed, 1);
        assert_eq!(result.provisional, 1);
        assert_eq!(result.vetoed, 1);
        assert_eq!(result.refuted, 1);
        assert_eq!(result.unproved, 1);
        assert_eq!(
            result.confirmed
                + result.provisional
                + result.vetoed
                + result.refuted
                + result.unproved,
            fates.len()
        );
    }

    #[test]
    fn zero_claims_have_no_provisional_reference_gap() {
        let result = tally(&[]);

        assert_eq!(
            result,
            Tally {
                confirmed: 0,
                provisional: 0,
                vetoed: 0,
                refuted: 0,
                unproved: 0,
            }
        );
        assert!(!any_provisional(&result));
    }

    #[test]
    fn one_provisional_claim_is_not_a_confirmation() {
        let result = tally(&[Fate::Provisional("no reference".to_string())]);

        assert_eq!(result.provisional, 1);
        assert_eq!(result.confirmed, 0);
        assert_eq!(result.vetoed, 0);
        assert_eq!(result.refuted, 0);
        assert_eq!(result.unproved, 0);
        assert_eq!(
            result.confirmed
                + result.provisional
                + result.vetoed
                + result.refuted
                + result.unproved,
            1
        );
        assert!(any_provisional(&result));
    }

    #[test]
    fn any_provisional_depends_only_on_the_provisional_count() {
        let with_confirmations = Tally {
            confirmed: 2,
            provisional: 0,
            vetoed: 1,
            refuted: 3,
            unproved: 4,
        };
        let only_provisional = Tally {
            confirmed: 0,
            provisional: 1,
            vetoed: 0,
            refuted: 0,
            unproved: 0,
        };

        assert!(!any_provisional(&with_confirmations));
        assert!(any_provisional(&only_provisional));
    }
}
