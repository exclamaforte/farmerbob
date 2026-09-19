//! Where a task stands between dispatch and scoring, as the board sees it.
//!
//! [`crate::board::observe`] yields a tier item for a task exactly when its
//! passing count was actually observed. That rule keeps a task with an
//! unreadable score file from being reported as scoring zero, but the disk
//! facts it consumes say nothing about whether the RUNS have finished, so
//! two opposite situations are identical to it: dispatched and still live
//! (wait), and finished but never scored (score it now). The second is the
//! most common thing the orchestrator needs to be told. (bead farmerbob-ervw)
//!
//! This module supplies the missing distinction as a pure function over a
//! [`Sighting`]: what the caller saw, never what this module looks up. It
//! performs no I/O and owns no policy — [`crate::attention::rank`] owns
//! priority and `board::observe` owns which items a task yields.

use crate::measurement::{Absent, Measurement};

/// What the caller saw on disk and in the process table for one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// The task name.
    pub task: String,
    /// How many worktrees exist for it, of any arm.
    pub worktrees: usize,
    /// How many agents for THIS task are running right now.
    /// `Missing` when the caller could not ask.
    pub live: Measurement<usize>,
    /// Whether a readable score file exists.
    pub scored: bool,
}

/// Where a task is in its run.
///
/// Exactly five variants, closed, and exhaustive over the inputs: scored or
/// not, worktrees or none, live known or not.
///
/// Deliberately not [`crate::runstate::RunState`], which is one dispatch's
/// position in the run lifecycle (queued, quota-blocked, verifying, ...);
/// this classifies a task's whole footprint as the board sees it. Where the
/// Exact API and an existing type conflict, the Exact API wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// No worktrees. It has never been dispatched.
    NeverDispatched,
    /// Worktrees exist and at least one agent is live.
    Running {
        /// How many are live.
        live: usize,
    },
    /// Worktrees exist, nothing is live, and nothing has been scored.
    /// This is the case the board is blind to.
    FinishedUnscored {
        /// How many worktrees are waiting to be measured.
        worktrees: usize,
    },
    /// Scored. What happens next is the tier rules' business, not this
    /// module's.
    Scored,
    /// The caller could not ask whether anything is live, so `Running` and
    /// `FinishedUnscored` cannot be told apart. Carries the reason.
    Unknown(String),
}

/// Classify one sighting.
///
/// Precedence is fixed by the clauses: already scored is [`RunState::Scored`]
/// whatever the worktree and live counts say — a task can be scored while a
/// straggler is still up; then zero worktrees is [`RunState::NeverDispatched`]
/// whatever the live count says, so an unobserved live count cannot turn a
/// never-dispatched task into an [`RunState::Unknown`]; only then does the
/// live count separate [`RunState::Running`] from
/// [`RunState::FinishedUnscored`], and a live count that could not be
/// observed is [`RunState::Unknown`] — an unasked question is not a negative
/// answer.
pub fn state(s: &Sighting) -> RunState {
    if s.scored {
        return RunState::Scored;
    }
    if s.worktrees == 0 {
        return RunState::NeverDispatched;
    }
    match &s.live {
        Measurement::Observed(n) if *n > 0 => RunState::Running { live: *n },
        Measurement::Observed(_) => RunState::FinishedUnscored {
            worktrees: s.worktrees,
        },
        Measurement::Missing(absent) => RunState::Unknown(unknown_reason(absent)),
    }
}

/// The sightings that need scoring, in the order given.
///
/// Exactly the sightings whose [`state`] is [`RunState::FinishedUnscored`] —
/// not `Unknown`, not `Running`, not `Scored` — in input order, with no
/// deduplication.
pub fn needs_scoring(sightings: &[Sighting]) -> Vec<&Sighting> {
    sightings
        .iter()
        .filter(|s| matches!(state(s), RunState::FinishedUnscored { .. }))
        .collect()
}

/// The reason a [`RunState::Unknown`] carries: the [`Absent`]'s own stated
/// reason when it has one, a fixed non-empty text when it does not. Never
/// empty, whatever the `Absent` carries — `Absent::NotAttempted` has no
/// reason field at all, and the other variants' fields are public, so a
/// blank one can be constructed by hand.
fn unknown_reason(absent: &Absent) -> String {
    match absent {
        Absent::NotAttempted => NOT_OBSERVED.to_string(),
        Absent::InstrumentFailed { reason }
        | Absent::NothingToMeasure { reason }
        | Absent::Untrusted { reason } => {
            if reason.trim().is_empty() {
                NOT_OBSERVED.to_string()
            } else {
                reason.clone()
            }
        }
    }
}

/// Fallback reason for an [`Absent`] that carries no text of its own.
const NOT_OBSERVED: &str = "whether any agent is live was not observed";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    /// Clauses 2 and 3, pinned in one test as required: the two states the
    /// board confuses, against sightings differing ONLY in the live count.
    #[test]
    fn running_versus_finished_turns_only_on_the_live_count() {
        let running = Sighting {
            task: "t".to_string(),
            worktrees: 3,
            live: Measurement::observed(2),
            scored: false,
        };
        let finished = Sighting {
            task: "t".to_string(),
            worktrees: 3,
            live: Measurement::observed(0),
            scored: false,
        };
        assert_eq!(state(&running), RunState::Running { live: 2 });
        assert_eq!(
            state(&finished),
            RunState::FinishedUnscored { worktrees: 3 }
        );
    }

    /// Clauses 1 and 7: zero worktrees is `NeverDispatched` whatever the
    /// live count says. An implementation that checks `live` first returns
    /// `Unknown` here and is wrong.
    #[test]
    fn never_dispatched_does_not_need_the_live_count() {
        let missing = Sighting {
            task: "t".to_string(),
            worktrees: 0,
            live: Measurement::not_attempted(),
            scored: false,
        };
        let observed_zero = Sighting {
            task: "t".to_string(),
            worktrees: 0,
            live: Measurement::observed(0),
            scored: false,
        };
        assert_eq!(state(&missing), RunState::NeverDispatched);
        assert_eq!(state(&observed_zero), RunState::NeverDispatched);
    }

    /// Clause 5: already scored is `Scored`, whatever the worktree and live
    /// counts say — pinned against a sighting that would otherwise be
    /// `Running`, since a straggler may still be up.
    #[test]
    fn scored_dominates_the_worktree_and_live_counts() {
        let straggler = Sighting {
            task: "t".to_string(),
            worktrees: 3,
            live: Measurement::observed(2),
            scored: true,
        };
        let no_worktrees = Sighting {
            task: "t".to_string(),
            worktrees: 0,
            live: Measurement::not_attempted(),
            scored: true,
        };
        assert_eq!(state(&straggler), RunState::Scored);
        assert_eq!(state(&no_worktrees), RunState::Scored);
    }

    /// Clause 6 and the empty-reason boundary: a live count that could not
    /// be observed is `Unknown`, never `FinishedUnscored`, and the reason is
    /// non-empty even when the `Absent` carries nothing, or a blank reason
    /// built by hand. The wording is not pinned; only non-emptiness is.
    #[test]
    fn missing_live_is_unknown_with_a_non_empty_reason() {
        let not_attempted = Sighting {
            task: "t".to_string(),
            worktrees: 2,
            live: Measurement::not_attempted(),
            scored: false,
        };
        let classified = state(&not_attempted);
        assert!(matches!(classified, RunState::Unknown(ref reason) if !reason.is_empty()));
        assert!(!matches!(classified, RunState::FinishedUnscored { .. }));

        let blank = Sighting {
            task: "t".to_string(),
            worktrees: 2,
            live: Measurement::Missing(Absent::Untrusted {
                reason: String::new(),
            }),
            scored: false,
        };
        assert!(matches!(state(&blank), RunState::Unknown(ref reason) if !reason.is_empty()));
    }

    /// Boundary: `Observed(0)` with one worktree is still a field worth
    /// scoring.
    #[test]
    fn one_worktree_with_nothing_live_is_finished_unscored() {
        let one = Sighting {
            task: "t".to_string(),
            worktrees: 1,
            live: Measurement::observed(0),
            scored: false,
        };
        assert_eq!(state(&one), RunState::FinishedUnscored { worktrees: 1 });
    }

    /// Boundary: no sightings, nothing to score.
    #[test]
    fn needs_scoring_of_no_sightings_is_empty() {
        assert!(needs_scoring(&[]).is_empty());
    }

    /// Clause 8 and the partition pin: on one of each of the five states,
    /// exactly the `FinishedUnscored` sighting comes back, in input order —
    /// not `Unknown`, not `Running`, not `Scored` — and its length plus the
    /// count of every other state equals the input length.
    #[test]
    fn needs_scoring_takes_exactly_the_finished_unscored_in_order() {
        let all = vec![
            Sighting {
                task: "never".to_string(),
                worktrees: 0,
                live: Measurement::observed(0),
                scored: false,
            },
            Sighting {
                task: "running".to_string(),
                worktrees: 2,
                live: Measurement::observed(1),
                scored: false,
            },
            Sighting {
                task: "finished".to_string(),
                worktrees: 2,
                live: Measurement::observed(0),
                scored: false,
            },
            Sighting {
                task: "scored".to_string(),
                worktrees: 3,
                live: Measurement::observed(0),
                scored: true,
            },
            Sighting {
                task: "unknown".to_string(),
                worktrees: 1,
                live: Measurement::not_attempted(),
                scored: false,
            },
        ];
        let need = needs_scoring(&all);
        assert_eq!(need, vec![&all[2]]);
        let others = all
            .iter()
            .filter(|s| !matches!(state(s), RunState::FinishedUnscored { .. }))
            .count();
        assert_eq!(need.len() + others, all.len());
    }

    /// No deduplication: two sightings of the same task name both appear if
    /// both are `FinishedUnscored`, in input order, with the running one
    /// between them left out.
    #[test]
    fn needs_scoring_does_not_deduplicate_same_task_sightings() {
        let all = vec![
            Sighting {
                task: "t".to_string(),
                worktrees: 3,
                live: Measurement::observed(0),
                scored: false,
            },
            Sighting {
                task: "t".to_string(),
                worktrees: 3,
                live: Measurement::observed(2),
                scored: false,
            },
            Sighting {
                task: "t".to_string(),
                worktrees: 4,
                live: Measurement::observed(0),
                scored: false,
            },
        ];
        assert_eq!(needs_scoring(&all), vec![&all[0], &all[2]]);
    }
}
