



// ESCALATED from cross-examination: codex-luna's suite discriminated on reviewer.
// Not a CLAIM -- cross-examination found it directly. Kept only because it passes
// against the merged winner, which is what separates a discovery from an
// over-fitted suite.
#[cfg(test)]
mod cx_reviewer_codex_luna {
    use super::*;

    fn accusation(input: &str, expect: &str, actual: &str) -> Submission {
        Submission {
            reviewer: "r".into(),
            role: Role::Prosecutor,
            artefact: Artefact::Accusation {
                input: input.into(),
                expect: expect.into(),
                actual: actual.into(),
            },
        }
    }

    #[test]
    fn defender_accusation_is_wrong_artefact_before_falsifiability() {
        let mut submission = accusation("x", "same", " SAME ");
        submission.role = Role::Defender;
        let (_, rejected) = triage(&[submission]);
        assert_eq!(
            rejected[0].1,
            Rejected::WrongArtefactForRole {
                role: Role::Defender
            }
        );
    }

    #[test]
    fn every_role_may_emit_an_opinion() {
        let submissions =
            [Role::Prosecutor, Role::Defender, Role::Cartographer].map(|role| Submission {
                reviewer: "r".into(),
                role,
                artefact: Artefact::Opinion {
                    text: "thought".into(),
                },
            });
        assert_eq!(triage(&submissions).0.len(), 3);
    }

    #[test]
    fn ratings_are_rejected_in_exact_forms() {
        let submissions = ["8/10", "SCORE: high"].map(|text| Submission {
            reviewer: "r".into(),
            role: Role::Prosecutor,
            artefact: Artefact::Opinion { text: text.into() },
        });
        assert!(triage(&submissions).0.is_empty());
        assert!(
            submissions
                .iter()
                .enumerate()
                .all(|(index, _)| triage(&submissions).1[index].1 == Rejected::Rated)
        );
    }

    #[test]
    fn identical_readings_are_not_ambiguous() {
        let submission = Submission {
            reviewer: "r".into(),
            role: Role::Cartographer,
            artefact: Artefact::Ambiguity {
                question: "q".into(),
                readings: vec!["One".into(), " one ".into()],
            },
        };
        assert_eq!(triage(&[submission]).1[0].1, Rejected::NotAmbiguous);
    }

    #[test]
    fn equal_expectation_and_actual_is_not_falsifiable() {
        assert_eq!(
            triage(&[accusation("x", " YES ", "yes")]).1[0].1,
            Rejected::NotFalsifiable
        );
    }

    #[test]
    fn defence_quoting_input_rebuts_accusation() {
        let submissions = [
            accusation("input A", "yes", "no"),
            Submission {
                reviewer: "d".into(),
                role: Role::Defender,
                artefact: Artefact::Defence {
                    clause: "C1".into(),
                    rationale: "The rule covers INPUT A here".into(),
                },
            },
        ];
        assert!(unrebutted(&submissions).is_empty());
    }

    #[test]
    fn open_questions_are_deduplicated_and_sorted() {
        let make = |question: &str| Submission {
            reviewer: "c".into(),
            role: Role::Cartographer,
            artefact: Artefact::Ambiguity {
                question: question.into(),
                readings: vec!["a".into(), "b".into()],
            },
        };
        let questions = open_questions(&[make("Zoo"), make("alpha"), make(" zoo ")]);
        assert_eq!(questions, vec!["alpha", "Zoo"]);
    }

    #[test]
    fn one_bad_submission_does_not_discard_the_batch() {
        let submissions = [
            accusation("x", "yes", "no"),
            accusation("y", "same", "same"),
        ];
        let (accepted, rejected) = triage(&submissions);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].artefact, submissions[0].artefact);
        assert_eq!(rejected[0].0, 1);
    }
}
