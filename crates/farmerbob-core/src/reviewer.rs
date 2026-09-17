//! Pure review roles and the evidence they are allowed to emit.

/// The narrow role assigned to a reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Finds a falsifiable failure.
    Prosecutor,
    /// Finds the strongest specification-based defence.
    Defender,
    /// Finds questions left open by the specification.
    Cartographer,
}

/// What a reviewer emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artefact {
    /// A falsifiable failure: an input, the specification's expectation, and the alleged behaviour.
    Accusation {
        /// The input on which the failure is alleged.
        input: String,
        /// The behaviour required by the specification.
        expect: String,
        /// The behaviour alleged to occur.
        actual: String,
    },
    /// A specification clause said to license the behaviour under attack.
    Defence {
        /// The cited specification clause.
        clause: String,
        /// The reasoning connecting the clause to the behaviour.
        rationale: String,
    },
    /// A question the specification does not answer, with the readings it permits.
    Ambiguity {
        /// The unresolved question.
        question: String,
        /// The readings permitted by the specification.
        readings: Vec<String>,
    },
    /// An opinion, which is recorded but never scored or executed.
    Opinion {
        /// The opinion text.
        text: String,
    },
}

/// A reviewer's named submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    /// The reviewer's name.
    pub reviewer: String,
    /// The role under which the reviewer submitted the artefact.
    pub role: Role,
    /// The emitted artefact.
    pub artefact: Artefact,
}

/// Why a submission was rejected during triage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// The artefact does not match the role that emitted it.
    WrongArtefactForRole { role: Role },
    /// The accusation's expectation and alleged behaviour are equivalent.
    NotFalsifiable,
    /// A required field is empty or whitespace-only.
    Empty { field: String },
    /// The ambiguity offers fewer than two distinct readings.
    NotAmbiguous,
    /// The submission contains a rating.
    Rated,
}

/// Accept executable evidence and reject submissions that cannot serve as evidence.
pub fn triage(subs: &[Submission]) -> (Vec<&Submission>, Vec<(usize, Rejected)>) {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();

    for (index, submission) in subs.iter().enumerate() {
        let role_matches = matches!(
            (&submission.role, &submission.artefact),
            (
                Role::Prosecutor,
                Artefact::Accusation { .. } | Artefact::Opinion { .. }
            ) | (
                Role::Defender,
                Artefact::Defence { .. } | Artefact::Opinion { .. }
            ) | (
                Role::Cartographer,
                Artefact::Ambiguity { .. } | Artefact::Opinion { .. }
            )
        );
        if !role_matches {
            rejected.push((
                index,
                Rejected::WrongArtefactForRole {
                    role: submission.role,
                },
            ));
            continue;
        }

        if has_rating(&submission.artefact) {
            rejected.push((index, Rejected::Rated));
            continue;
        }

        let reason = match &submission.artefact {
            Artefact::Accusation {
                input,
                expect,
                actual,
            } => {
                if let Some(field) =
                    empty_field([("input", input), ("expect", expect), ("actual", actual)])
                {
                    Some(Rejected::Empty { field })
                } else if equivalent(expect, actual) {
                    Some(Rejected::NotFalsifiable)
                } else {
                    None
                }
            }
            Artefact::Defence { clause, rationale } => {
                empty_field([("clause", clause), ("rationale", rationale)])
                    .map(|field| Rejected::Empty { field })
            }
            Artefact::Ambiguity { question, readings } => {
                if let Some(field) = empty_field([("question", question)]) {
                    Some(Rejected::Empty { field })
                } else if readings.len() < 2 {
                    Some(Rejected::NotAmbiguous)
                } else if readings.iter().any(|reading| reading.trim().is_empty()) {
                    Some(Rejected::Empty {
                        field: "readings".to_owned(),
                    })
                } else if distinct_folded(readings).len() < 2 {
                    Some(Rejected::NotAmbiguous)
                } else {
                    None
                }
            }
            Artefact::Opinion { text } => {
                empty_field([("text", text)]).map(|field| Rejected::Empty { field })
            }
        };

        if let Some(reason) = reason {
            rejected.push((index, reason));
        } else if submission.reviewer.trim().is_empty() {
            rejected.push((
                index,
                Rejected::Empty {
                    field: "reviewer".to_owned(),
                },
            ));
        } else {
            accepted.push(submission);
        }
    }

    (accepted, rejected)
}

/// Return accusations that have not been rebutted by a cited defence.
pub fn unrebutted(subs: &[Submission]) -> Vec<&Submission> {
    let defences: Vec<(&str, &str)> = subs
        .iter()
        .filter_map(|submission| match &submission.artefact {
            Artefact::Defence { clause, rationale } if !clause.trim().is_empty() => {
                Some((clause.as_str(), rationale.as_str()))
            }
            _ => None,
        })
        .collect();

    subs.iter()
        .filter(|submission| match &submission.artefact {
            Artefact::Accusation { input, .. } => !defences.iter().any(|(_, rationale)| {
                !input.trim().is_empty() && contains_folded(rationale, input)
            }),
            _ => false,
        })
        .collect()
}

/// Return distinct open questions in case-insensitive sorted order.
pub fn open_questions(subs: &[Submission]) -> Vec<String> {
    let mut questions = Vec::new();
    for submission in subs {
        if submission.role == Role::Cartographer
            && let Artefact::Ambiguity { question, .. } = &submission.artefact
            && !questions
                .iter()
                .any(|known: &String| equivalent(known, question))
        {
            questions.push(question.clone());
        }
    }
    questions.sort_by_key(|question| folded(question));
    questions
}

fn folded(value: &str) -> String {
    value.trim().to_lowercase()
}

fn equivalent(left: &str, right: &str) -> bool {
    folded(left) == folded(right)
}

fn contains_folded(haystack: &str, needle: &str) -> bool {
    folded(haystack).contains(&folded(needle))
}

fn distinct_folded(values: &[String]) -> std::collections::HashSet<String> {
    values.iter().map(|value| folded(value)).collect()
}

fn empty_field<const N: usize>(fields: [(&str, &str); N]) -> Option<String> {
    fields
        .iter()
        .find(|(_, value)| value.trim().is_empty())
        .map(|(field, _)| (*field).to_owned())
}

fn has_rating(artefact: &Artefact) -> bool {
    let texts: Vec<&str> = match artefact {
        Artefact::Accusation {
            input,
            expect,
            actual,
        } => vec![input, expect, actual],
        Artefact::Defence { clause, rationale } => vec![clause, rationale],
        Artefact::Ambiguity { question, readings } => {
            let mut texts = vec![question.as_str()];
            texts.extend(readings.iter().map(String::as_str));
            texts
        }
        Artefact::Opinion { text } => vec![text],
    };

    texts.into_iter().any(text_has_rating)
}

fn text_has_rating(text: &str) -> bool {
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    bytes.windows(4).any(|window| {
        window[0].is_ascii_digit() && window[1] == b'/' && window[2] == b'1' && window[3] == b'0'
    }) || lower
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| ["score", "rating", "grade"].contains(&word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submission(role: Role, artefact: Artefact) -> Submission {
        Submission {
            reviewer: "reviewer".to_owned(),
            role,
            artefact,
        }
    }

    fn accusation(input: &str, expect: &str, actual: &str) -> Artefact {
        Artefact::Accusation {
            input: input.to_owned(),
            expect: expect.to_owned(),
            actual: actual.to_owned(),
        }
    }

    #[test]
    fn defender_accusation_is_wrong_role_before_falsifiability() {
        let sub = submission(Role::Defender, accusation("x", "same", " same "));
        let (_, rejected) = triage(&[sub]);
        assert_eq!(
            rejected,
            vec![(
                0,
                Rejected::WrongArtefactForRole {
                    role: Role::Defender
                }
            )]
        );
    }

    #[test]
    fn every_role_may_emit_an_opinion() {
        let subs = [
            submission(
                Role::Prosecutor,
                Artefact::Opinion {
                    text: "possible".to_owned(),
                },
            ),
            submission(
                Role::Defender,
                Artefact::Opinion {
                    text: "possible".to_owned(),
                },
            ),
            submission(
                Role::Cartographer,
                Artefact::Opinion {
                    text: "possible".to_owned(),
                },
            ),
        ];
        assert_eq!(triage(&subs).0.len(), 3);
    }

    #[test]
    fn eight_out_of_ten_is_rated() {
        let sub = submission(
            Role::Prosecutor,
            Artefact::Opinion {
                text: "8/10".to_owned(),
            },
        );
        assert_eq!(triage(&[sub]).1[0].1, Rejected::Rated);
    }

    #[test]
    fn score_word_is_rated_case_insensitively() {
        let sub = submission(
            Role::Defender,
            Artefact::Opinion {
                text: "SCORE: high".to_owned(),
            },
        );
        assert_eq!(triage(&[sub]).1[0].1, Rejected::Rated);
    }

    #[test]
    fn empty_required_field_is_rejected_by_name() {
        let sub = submission(Role::Prosecutor, accusation(" ", "one", "two"));
        assert_eq!(
            triage(&[sub]).1,
            vec![(
                0,
                Rejected::Empty {
                    field: "input".to_owned()
                }
            )]
        );
    }

    #[test]
    fn fewer_than_two_readings_is_not_ambiguous() {
        let sub = submission(
            Role::Cartographer,
            Artefact::Ambiguity {
                question: "which mode?".to_owned(),
                readings: vec!["strict".to_owned()],
            },
        );
        assert_eq!(triage(&[sub]).1[0].1, Rejected::NotAmbiguous);
    }

    #[test]
    fn each_role_accepts_only_its_evidence_artefact() {
        let subs = [
            submission(
                Role::Prosecutor,
                Artefact::Defence {
                    clause: "clause".to_owned(),
                    rationale: "reason".to_owned(),
                },
            ),
            submission(
                Role::Defender,
                Artefact::Ambiguity {
                    question: "question".to_owned(),
                    readings: vec!["one".to_owned(), "two".to_owned()],
                },
            ),
            submission(Role::Cartographer, accusation("input", "one", "two")),
        ];
        let (_, rejected) = triage(&subs);
        assert!(
            rejected
                .iter()
                .all(|(_, reason)| matches!(reason, Rejected::WrongArtefactForRole { .. }))
        );
    }

    #[test]
    fn identical_readings_are_not_ambiguous() {
        let sub = submission(
            Role::Cartographer,
            Artefact::Ambiguity {
                question: "which mode?".to_owned(),
                readings: vec!["strict".to_owned(), " STRICT ".to_owned()],
            },
        );
        assert_eq!(triage(&[sub]).1[0].1, Rejected::NotAmbiguous);
    }

    #[test]
    fn equal_expectation_and_actual_are_not_falsifiable() {
        let sub = submission(Role::Prosecutor, accusation("x", "Value", " value "));
        assert_eq!(triage(&[sub]).1[0].1, Rejected::NotFalsifiable);
    }

    #[test]
    fn defence_quoting_input_rebuts_accusation() {
        let accusation = submission(Role::Prosecutor, accusation("bad input", "one", "two"));
        let defence = submission(
            Role::Defender,
            Artefact::Defence {
                clause: "clause 1".to_owned(),
                rationale: "The bad input is permitted here.".to_owned(),
            },
        );
        assert!(unrebutted(&[accusation, defence]).is_empty());
    }

    #[test]
    fn open_questions_are_deduplicated_and_sorted() {
        let subs = [
            submission(
                Role::Cartographer,
                Artefact::Ambiguity {
                    question: "Z question".to_owned(),
                    readings: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            submission(
                Role::Cartographer,
                Artefact::Ambiguity {
                    question: "a question".to_owned(),
                    readings: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
            submission(
                Role::Cartographer,
                Artefact::Ambiguity {
                    question: " A QUESTION ".to_owned(),
                    readings: vec!["a".to_owned(), "b".to_owned()],
                },
            ),
        ];
        assert_eq!(open_questions(&subs), vec!["a question", "Z question"]);
    }

    #[test]
    fn one_bad_submission_does_not_discard_the_batch() {
        let subs = [
            submission(Role::Prosecutor, accusation("x", "one", "two")),
            submission(Role::Prosecutor, accusation("y", "same", "same")),
            submission(Role::Prosecutor, accusation("z", "one", "two")),
        ];
        let (accepted, rejected) = triage(&subs);
        assert_eq!(accepted.len(), 2);
        assert_eq!(rejected, vec![(1, Rejected::NotFalsifiable)]);
    }
}



