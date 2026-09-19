//! Stage prerequisites and arm credit assignments.

/// A stage of the task pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Critique of the specification, before anyone implements.
    Speccheck,
    /// Objective measurement of each candidate.
    Score,
    /// Every candidate's test suite run against every other candidate's code.
    Crossx,
    /// A model reads one candidate's deliverable and reports findings.
    Critique,
    /// Findings become claims. Pure computation over critiques.
    Promote,
    /// A promoted claim becomes an executable discriminating test.
    Prove,
}

/// What a stage needs before it can run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Nothing but the specification. Runs before any candidate exists.
    SpecOnly,
    /// At least this many candidates. No second opinion is required because
    /// the stage runs no model.
    Candidates(u32),
    /// At least this many candidates and one independent roster arm.
    CandidatesAndIndependent(u32),
    /// At least this many candidates because the stage compares them against
    /// one another.
    ComparedCandidates(u32),
}

/// What the named stage needs.
pub fn needs(stage: Stage) -> Needs {
    match stage {
        Stage::Speccheck => Needs::SpecOnly,
        Stage::Score | Stage::Promote => Needs::Candidates(1),
        Stage::Crossx => Needs::ComparedCandidates(2),
        Stage::Critique | Stage::Prove => Needs::CandidatesAndIndependent(1),
    }
}

/// The seat an arm is cast in. This is the unit of credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    /// Reviewed the specification before anyone implemented.
    SpecCritic,
    /// Reviewed an implementation.
    Critic,
    /// Turned a promoted claim into an executable test.
    Prover,
}

/// One arm, cast in one seat, for one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Casting {
    /// The stage this casting is for.
    pub stage: Stage,
    /// The seat being filled.
    pub seat: Seat,
    /// The arm that does the work and receives the credit.
    pub arm: String,
    /// Whose work is under examination, if any.
    pub subject: Option<String>,
}

/// Why a stage could not be cast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uncast {
    /// A comparing or examining stage has too few candidates.
    TooFewCandidates {
        /// How many candidates the stage needs.
        needed: u32,
        /// How many candidates the field has.
        have: u32,
    },
    /// No roster arm is independent of the work being examined.
    NoIndependentArm,
}

/// Cast the seats a stage needs for one field.
pub fn cast(
    stage: Stage,
    candidates: &[String],
    roster: &[String],
) -> Result<Vec<Casting>, Uncast> {
    match stage {
        Stage::Speccheck => roster
            .first()
            .cloned()
            .map(|arm| {
                vec![Casting {
                    stage,
                    seat: Seat::SpecCritic,
                    arm,
                    subject: None,
                }]
            })
            .ok_or(Uncast::NoIndependentArm),
        Stage::Score | Stage::Promote => {
            require_candidates(candidates, 1)?;
            Ok(Vec::new())
        }
        Stage::Crossx => {
            require_candidates(candidates, 2)?;
            Ok(Vec::new())
        }
        Stage::Critique | Stage::Prove => {
            let seat = if stage == Stage::Critique {
                Seat::Critic
            } else {
                Seat::Prover
            };
            require_candidates(candidates, 1)?;
            examining_castings(stage, seat, candidates, roster)
        }
    }
}

fn require_candidates(candidates: &[String], needed: u32) -> Result<(), Uncast> {
    if candidates.len() < needed as usize {
        Err(Uncast::TooFewCandidates {
            needed,
            have: candidates.len() as u32,
        })
    } else {
        Ok(())
    }
}

fn examining_castings(
    stage: Stage,
    seat: Seat,
    candidates: &[String],
    roster: &[String],
) -> Result<Vec<Casting>, Uncast> {
    let mut castings = Vec::with_capacity(candidates.len());
    for subject in candidates {
        let Some(arm) = roster.iter().find(|arm| *arm != subject) else {
            return Err(Uncast::NoIndependentArm);
        };
        castings.push(Casting {
            stage,
            seat,
            arm: arm.clone(),
            subject: Some(subject.clone()),
        });
    }
    Ok(castings)
}

#[cfg(test)]
mod tests {
    use super::{Casting, Needs, Seat, Stage, Uncast, cast, needs};

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn needs_distinguishes_comparison_from_independent_review() {
        assert_eq!(needs(Stage::Crossx), Needs::ComparedCandidates(2));
        assert_eq!(needs(Stage::Critique), Needs::CandidatesAndIndependent(1));
        assert_eq!(needs(Stage::Speccheck), Needs::SpecOnly);
        assert_eq!(needs(Stage::Score), Needs::Candidates(1));
        assert_eq!(needs(Stage::Promote), Needs::Candidates(1));
        assert_eq!(needs(Stage::Prove), Needs::CandidatesAndIndependent(1));
    }

    #[test]
    fn one_candidate_can_be_critiqued_by_an_independent_roster_arm() {
        assert_eq!(
            cast(Stage::Critique, &names(&["a"]), &names(&["a", "b", "c"])),
            Ok(vec![Casting {
                stage: Stage::Critique,
                seat: Seat::Critic,
                arm: String::from("b"),
                subject: Some(String::from("a")),
            }])
        );
    }

    #[test]
    fn comparison_requires_two_candidates_even_with_a_roster() {
        assert_eq!(
            cast(Stage::Crossx, &names(&["a"]), &names(&["a", "b", "c"])),
            Err(Uncast::TooFewCandidates { needed: 2, have: 1 })
        );
        assert_eq!(
            cast(Stage::Crossx, &names(&["a", "b"]), &[]),
            Ok(Vec::new())
        );
    }

    #[test]
    fn examining_stage_distinguishes_no_candidate_from_no_examiner() {
        assert_eq!(
            cast(Stage::Critique, &[], &names(&["a"])),
            Err(Uncast::TooFewCandidates { needed: 1, have: 0 })
        );
        assert_eq!(
            cast(Stage::Critique, &names(&["a"]), &names(&["a"])),
            Err(Uncast::NoIndependentArm)
        );
    }

    #[test]
    fn examiner_and_subject_are_recorded_in_their_distinct_fields() {
        assert_eq!(
            cast(Stage::Critique, &names(&["a", "b"]), &names(&["a", "b"])),
            Ok(vec![
                Casting {
                    stage: Stage::Critique,
                    seat: Seat::Critic,
                    arm: String::from("b"),
                    subject: Some(String::from("a")),
                },
                Casting {
                    stage: Stage::Critique,
                    seat: Seat::Critic,
                    arm: String::from("a"),
                    subject: Some(String::from("b")),
                },
            ])
        );
    }

    #[test]
    fn speccheck_uses_the_first_roster_arm_and_needs_no_candidate() {
        assert_eq!(
            cast(Stage::Speccheck, &[], &names(&["a", "b"])),
            Ok(vec![Casting {
                stage: Stage::Speccheck,
                seat: Seat::SpecCritic,
                arm: String::from("a"),
                subject: None,
            }])
        );
        assert_eq!(
            cast(Stage::Speccheck, &[], &[]),
            Err(Uncast::NoIndependentArm)
        );
    }

    #[test]
    fn score_and_promote_cast_no_model_and_need_no_roster() {
        assert_eq!(
            cast(Stage::Score, &names(&["a"]), &names(&["a"])),
            Ok(Vec::new())
        );
        assert_eq!(cast(Stage::Promote, &names(&["a"]), &[]), Ok(Vec::new()));
    }

    #[test]
    fn examiner_selection_preserves_roster_order_and_candidate_occurrences() {
        assert_eq!(
            cast(Stage::Critique, &names(&["a", "a"]), &names(&["c", "c"])),
            Ok(vec![
                Casting {
                    stage: Stage::Critique,
                    seat: Seat::Critic,
                    arm: String::from("c"),
                    subject: Some(String::from("a")),
                },
                Casting {
                    stage: Stage::Critique,
                    seat: Seat::Critic,
                    arm: String::from("c"),
                    subject: Some(String::from("a")),
                },
            ])
        );
        assert_eq!(
            cast(Stage::Critique, &names(&["a"]), &names(&["c", "a"])),
            Ok(vec![Casting {
                stage: Stage::Critique,
                seat: Seat::Critic,
                arm: String::from("c"),
                subject: Some(String::from("a")),
            }])
        );
    }

    #[test]
    fn prove_uses_the_prover_seat() {
        assert_eq!(
            cast(Stage::Prove, &names(&["a"]), &names(&["a", "b"])),
            Ok(vec![Casting {
                stage: Stage::Prove,
                seat: Seat::Prover,
                arm: String::from("b"),
                subject: Some(String::from("a")),
            }])
        );
    }
}
