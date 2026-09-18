//! What a pipeline stage actually did, as distinct from what its exit status
//! said.
//!
//! The pipeline recorded a stage as done whenever its command exited 0, and
//! that was the only question it knew how to ask: `promote` exited 0 for
//! sixty-six tasks while writing no artefact at all, and every one of those
//! tasks carried a signature saying the stage had completed. The pipeline was
//! not lying; it had asked the only question it knew. This module is the
//! missing question, in the crate where it can be tested. The artefact's size
//! arrives as a [`Measurement`], so the three answers stay distinct: ran
//! ([`StageOutcome::Ran`]), looked at and found to have produced nothing
//! ([`StageOutcome::ProducedNothing`]), and never inspected at all
//! ([`StageOutcome::Unknown`]). Collapsing the last two into the first two is
//! the coercion this module exists to close.
//!
//! The module is pure: no I/O. The caller stats the file and says what
//! happened.

use crate::measurement::{Absent, Measurement};

/// What a stage actually did, as distinct from what its exit status said.
///
/// Exactly these variants and no others: a closed set of five. Every one is
/// derivable from an exit status and an artefact size except
/// [`StageOutcome::Skipped`], which no classifier can produce because a stage
/// that was never attempted has neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// Exited 0 and left a non-empty artefact.
    Ran,
    /// Exited 0 and left no artefact, or an empty one. The stage is not
    /// trustworthy and its signature must not be written.
    ProducedNothing,
    /// Exited non-zero.
    Failed {
        /// The exit status, as reported. A negative `rc` — what a
        /// signal-killed process reports through some launchers — is
        /// preserved verbatim, never normalised.
        rc: i32,
    },
    /// Exited 0, and the artefact could not be inspected at all, so neither
    /// [`StageOutcome::Ran`] nor [`StageOutcome::ProducedNothing`] is known
    /// to be true.
    Unknown {
        /// Why the artefact could not be inspected, in prose. Never empty.
        why: String,
    },
    /// Never attempted.
    Skipped {
        /// Why it was not attempted, in prose. Never empty.
        why: String,
    },
}

/// One stage of a pipeline, named, with what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// The stage's name, as the pipeline prints it. The empty string is
    /// degenerate, not invalid: it classifies, reports and blocks like any
    /// other name.
    pub name: String,
    /// What it did.
    pub outcome: StageOutcome,
}

/// Classify one stage from its exit status and the size of the artefact it
/// was supposed to leave.
///
/// `bytes` is `Observed(n)` when the artefact's size was read — `n` may be
/// zero — and `Missing(..)` when it could not be. This module does no I/O:
/// the caller stats the file and says what happened.
///
/// The rules, in the order they are applied:
///
/// - Every non-zero `rc` — negative included, preserved verbatim — is
///   [`StageOutcome::Failed`], whatever `bytes` says, even when a file is
///   sitting there: the exit status is the fact known first, and a stage that
///   failed has not produced a trustworthy artefact.
/// - `rc == 0` with `Observed(n > 0)` is [`StageOutcome::Ran`]; with
///   `Observed(0)` it is [`StageOutcome::ProducedNothing`]. The boundary is
///   pinned on both sides: zero bytes is nothing, one byte is ran.
/// - `rc == 0` with `Missing(..)` is [`StageOutcome::Unknown`], and never
///   [`StageOutcome::ProducedNothing`] — "I could not look" and "I looked and
///   it was empty" are different facts. All four [`Absent`] variants give the
///   same answer, because the distinction between them is about the
///   INSTRUMENT and this function's question is about the STAGE.
///
/// [`StageOutcome::Skipped`] is never produced here: a stage that was not
/// attempted has no `rc` and no `bytes` to classify, and this signature has
/// no argument that could express a decision not to run. A caller that
/// declines to run a stage constructs `Skipped` itself.
pub fn classify(rc: i32, bytes: &Measurement<u64>) -> StageOutcome {
    if rc != 0 {
        return StageOutcome::Failed { rc };
    }
    match bytes {
        Measurement::Observed(0) => StageOutcome::ProducedNothing,
        Measurement::Observed(_) => StageOutcome::Ran,
        Measurement::Missing(absent) => StageOutcome::Unknown {
            why: unknown_why(absent),
        },
    }
}

/// The `why` prose for an [`Unknown`](StageOutcome::Unknown) outcome: the
/// stated reason where the [`Absent`] carries one, and the plain fact of
/// never having inspected where it does not.
///
/// [`Absent::NotAttempted`] is the one variant with no stated reason to
/// derive from, so its `why` says directly that the artefact was never
/// inspected. Blank reasons cannot reach this function: `measurement`
/// normalises them at construction.
fn unknown_why(absent: &Absent) -> String {
    let stated = match absent {
        Absent::NotAttempted => {
            return "the artefact was never inspected: no size measurement was attempted"
                .to_string();
        }
        Absent::InstrumentFailed { reason }
        | Absent::NothingToMeasure { reason }
        | Absent::Untrusted { reason } => reason,
    };
    format!("the artefact could not be inspected: {stated}")
}

/// Whether this outcome permits the pipeline to record the stage as done.
///
/// True for exactly one variant, [`StageOutcome::Ran`]: a signature says the
/// stage ran and left something to show for it. `ProducedNothing` is the
/// original defect and must not sign; `Failed` did not run to completion;
/// `Unknown` is precisely the case that is not known; `Skipped` never ran.
pub fn may_sign(o: &StageOutcome) -> bool {
    matches!(o, StageOutcome::Ran)
}

/// The first stage that stops the pipeline, if any: the earliest stage in
/// `stages` whose outcome does not permit a signature.
///
/// The EARLIEST, not the last and not the worst: a later failure does not
/// outrank an earlier one, because the later stage ran against whatever the
/// earlier one left. An empty slice is not blocked — it is empty — and
/// neither is a slice whose every stage may sign. The return is a borrow of
/// an element of the input, so the caller can name the stage; not a copy, an
/// index or a bool.
pub fn first_blocking(stages: &[Stage]) -> Option<&Stage> {
    stages.iter().find(|stage| !may_sign(&stage.outcome))
}

/// A one-line report for a human reading the pipeline log, for each stage in
/// the order given.
///
/// Exactly one line per stage, in input order, never reordered and never
/// filtered: a stage that is fine still gets a line, because a report that
/// lists only problems cannot be read as a list of what ran. Every line
/// contains the stage's name; what else a line says is prose and is not part
/// of the contract. An empty slice yields an empty vector, not a line saying
/// the pipeline was empty. A stage whose name is the empty string is
/// degenerate, not invalid, and is reported like any other.
pub fn report(stages: &[Stage]) -> Vec<String> {
    stages.iter().map(outcome_line).collect()
}

/// The one report line for one stage: the name, then what the outcome was.
/// The wording after the colon is prose for the log reader; only the
/// containment of the name is pinned.
fn outcome_line(stage: &Stage) -> String {
    let what = match &stage.outcome {
        StageOutcome::Ran => "ran".to_string(),
        StageOutcome::ProducedNothing => "produced nothing".to_string(),
        StageOutcome::Failed { rc } => format!("failed with rc {rc}"),
        StageOutcome::Unknown { why } => format!("could not be checked: {why}"),
        StageOutcome::Skipped { why } => format!("skipped: {why}"),
    };
    let name = &stage.name;
    format!("{name}: {what}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Measurement;

    #[test]
    fn any_non_zero_rc_is_failed_whatever_the_bytes_say() {
        let sizes: [Measurement<u64>; 4] = [
            Measurement::observed(0),
            Measurement::observed(4096),
            Measurement::not_attempted(),
            Measurement::instrument_failed("stat: no such file"),
        ];
        for rc in [1, 2, 127, -1, -9, i32::MIN] {
            for bytes in &sizes {
                assert_eq!(classify(rc, bytes), StageOutcome::Failed { rc });
            }
        }
    }

    #[test]
    fn exit_zero_pinned_on_both_sides_of_the_byte_boundary() {
        assert_eq!(
            classify(0, &Measurement::observed(0)),
            StageOutcome::ProducedNothing
        );
        assert_eq!(classify(0, &Measurement::observed(1)), StageOutcome::Ran);
        assert_eq!(classify(0, &Measurement::observed(4096)), StageOutcome::Ran);
    }

    #[test]
    fn every_missing_reason_is_unknown_with_a_stated_why() {
        let missings: [Measurement<u64>; 4] = [
            Measurement::not_attempted(),
            Measurement::instrument_failed("suite did not compile"),
            Measurement::nothing_to_measure("stage writes no artefact by design"),
            Measurement::untrusted("artefact from a known-broken stage"),
        ];
        for bytes in missings {
            match classify(0, &bytes) {
                StageOutcome::Unknown { why } => assert!(!why.is_empty()),
                other => panic!("expected Unknown for {bytes:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn classify_never_says_skipped() {
        let sizes: [Measurement<u64>; 3] = [
            Measurement::observed(0),
            Measurement::observed(64),
            Measurement::not_attempted(),
        ];
        for rc in [0, 3, -15] {
            for bytes in &sizes {
                assert!(
                    !matches!(classify(rc, bytes), StageOutcome::Skipped { .. }),
                    "classify({rc}, {bytes:?}) returned Skipped"
                );
            }
        }
    }

    /// `may_sign` is only meaningful against the outcomes `classify` can
    /// produce, so the pair is tested against one table: a `classify` that
    /// never returned `ProducedNothing` and a `may_sign` that wrongly signed
    /// it would each pass an independent test and together reproduce the
    /// original defect.
    #[test]
    fn may_sign_agrees_with_classify_across_the_whole_table() {
        let cases: [(i32, Measurement<u64>, bool); 5] = [
            (0, Measurement::observed(1), true),
            (0, Measurement::observed(0), false),
            (1, Measurement::observed(4096), false),
            (0, Measurement::not_attempted(), false),
            (-9, Measurement::not_attempted(), false),
        ];
        for (rc, bytes, signs) in cases {
            let outcome = classify(rc, &bytes);
            assert_eq!(
                may_sign(&outcome),
                signs,
                "classify({rc}, {bytes:?}) = {outcome:?}"
            );
        }
    }

    #[test]
    fn may_sign_is_false_for_skipped() {
        let skipped = StageOutcome::Skipped {
            why: "disabled in config".to_string(),
        };
        assert!(!may_sign(&skipped));
    }

    #[test]
    fn the_earliest_non_signable_stage_wins_not_the_last_or_the_worst() {
        let stages = vec![
            Stage {
                name: "prepare".to_string(),
                outcome: StageOutcome::Ran,
            },
            Stage {
                name: "score".to_string(),
                outcome: StageOutcome::ProducedNothing,
            },
            Stage {
                name: "promote".to_string(),
                outcome: StageOutcome::Failed { rc: 1 },
            },
        ];
        assert_eq!(
            first_blocking(&stages),
            Some(&Stage {
                name: "score".to_string(),
                outcome: StageOutcome::ProducedNothing,
            })
        );
    }

    #[test]
    fn skipped_and_unknown_block_when_they_are_first() {
        let with_skipped = vec![
            Stage {
                name: "lint".to_string(),
                outcome: StageOutcome::Ran,
            },
            Stage {
                name: "crossx".to_string(),
                outcome: StageOutcome::Skipped {
                    why: "no rivals".to_string(),
                },
            },
            Stage {
                name: "promote".to_string(),
                outcome: StageOutcome::Failed { rc: 2 },
            },
        ];
        assert_eq!(
            first_blocking(&with_skipped),
            Some(&Stage {
                name: "crossx".to_string(),
                outcome: StageOutcome::Skipped {
                    why: "no rivals".to_string(),
                },
            })
        );

        let with_unknown = vec![Stage {
            name: "score".to_string(),
            outcome: StageOutcome::Unknown {
                why: "stat failed".to_string(),
            },
        }];
        assert_eq!(first_blocking(&with_unknown), Some(&with_unknown[0]));
    }

    #[test]
    fn an_all_signable_pipeline_is_not_blocked() {
        let stages = vec![
            Stage {
                name: "prepare".to_string(),
                outcome: StageOutcome::Ran,
            },
            Stage {
                name: "score".to_string(),
                outcome: StageOutcome::Ran,
            },
        ];
        assert_eq!(first_blocking(&stages), None);
    }

    #[test]
    fn an_empty_pipeline_is_not_blocked() {
        let stages: Vec<Stage> = Vec::new();
        assert_eq!(first_blocking(&stages), None);
    }

    #[test]
    fn the_return_is_a_borrow_of_the_input_element_itself() {
        let stages = vec![
            Stage {
                name: "score".to_string(),
                outcome: StageOutcome::ProducedNothing,
            },
            Stage {
                name: "promote".to_string(),
                outcome: StageOutcome::Failed { rc: 1 },
            },
        ];
        let blocking = first_blocking(&stages).expect("a blocking stage exists");
        assert!(std::ptr::eq(blocking, &stages[0]));
    }

    #[test]
    fn report_gives_one_line_per_stage_in_order_and_names_each() {
        let stages = vec![
            Stage {
                name: "prepare".to_string(),
                outcome: StageOutcome::Ran,
            },
            Stage {
                name: "score".to_string(),
                outcome: StageOutcome::ProducedNothing,
            },
            Stage {
                name: "promote".to_string(),
                outcome: StageOutcome::Failed { rc: 1 },
            },
        ];
        let lines = report(&stages);
        assert_eq!(lines.len(), stages.len());
        for (line, stage) in lines.iter().zip(&stages) {
            let name = &stage.name;
            assert!(
                line.contains(name.as_str()),
                "line {line:?} does not name {name}"
            );
        }
    }

    #[test]
    fn an_empty_pipeline_reports_no_lines() {
        let stages: Vec<Stage> = Vec::new();
        assert!(report(&stages).is_empty());
    }

    #[test]
    fn an_empty_stage_name_is_degenerate_not_invalid() {
        let stages = vec![Stage {
            name: String::new(),
            outcome: StageOutcome::ProducedNothing,
        }];
        assert_eq!(first_blocking(&stages), Some(&stages[0]));
        assert_eq!(report(&stages).len(), 1);
    }
}
