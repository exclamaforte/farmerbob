//! The autopilot's directory listing, turned into facts.
//!
//! `fb-autopilot.sh` infers what state a task is in from which files exist:
//! a score file means the gate ran, a claims file means the subjective tier
//! ran, a `.tsv` under `.fb/queue/` means a wave is waiting. The inference is
//! right; the medium is not. In shell it cannot be tested, it reads an
//! unreadable directory as an empty one, and it cannot tell "failed" from
//! "not yet run" — which is how one broken pipeline was retried 228 times.
//!
//! This module is that inference as a pure function. The caller lists the
//! directory — no I/O here and no clock — and [`observe`] maps the listing
//! onto the [`Item`]s [`crate::attention::rank`] orders. Every field of
//! [`TaskFiles`] is an observation; each inference the shell made lives in
//! exactly one clause of [`observe`], and nothing is dropped on the way.

use crate::attention::{Facts, Item};
use crate::measurement::Measurement;
use crate::run_state_view::{self, RunState};

/// One task, as the caller found it on disk. Every field is what the caller
/// could actually observe; nothing here is inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFiles {
    /// The task name.
    pub name: String,
    /// A spec exists at `.fb/prompts/<name>.md`.
    pub has_spec: bool,
    /// How many candidates passed the gate, from `<name>.score.json`.
    /// `Missing` when the file is absent or could not be read.
    pub passing: Measurement<usize>,
    /// The subjective tier left a non-empty `<name>.claims.json`.
    pub has_claims: bool,
    /// An adjudication record exists at `.fb/adjudicated/<name>`.
    pub adjudicated: bool,
    /// Stages the last pipeline run reported as failed, in pipeline order.
    /// EMPTY means the last run failed nothing; it does not mean no run.
    pub failed_stages: Vec<String>,
    /// How many worktrees exist for this task, of any arm.
    pub worktrees: usize,
    /// How many agents for THIS task are running right now. `Missing` when
    /// the caller could not ask.
    pub live: Measurement<usize>,
}

/// One queued matrix the caller found in `.fb/queue/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMatrix {
    /// The file's basename, e.g. `wave94.tsv`.
    pub matrix: String,
    /// How many runs its lines would dispatch.
    pub runs: usize,
}

/// Turn a listing into the facts `attention::rank` consumes.
///
/// `live_agents` is passed through unchanged: this module observes nothing
/// itself and never substitutes a count for an absent one.
///
/// Each entry of `tasks` yields its items independently — `PipelineFailed`
/// first when the last run failed stages, then at most one tier item: a task
/// not yet adjudicated yields `Adjudicate` or `RunPipeline` exactly when its
/// passing count was actually observed, while a finished-but-unscored task is
/// withheld from the tier rules because its item variant is not available on
/// this API surface. A task already adjudicated yields no tier item at all.
/// Nothing is sorted and nothing is filtered:
/// `Facts.items` holds exactly what those clauses produce, tasks in the order
/// given, then queued waves in the order given, then `QueueEmpty` when the
/// listing left nothing to do. A name appearing twice in `tasks` is
/// degenerate, not invalid, and both entries yield their items.
pub fn observe(
    tasks: &[TaskFiles],
    queued: &[QueuedMatrix],
    live_agents: Measurement<usize>,
) -> Facts {
    let mut items = Vec::new();
    let mut has_finished_unscored = false;

    for task in tasks {
        // The failed run is a fact about the instrument, not a request to
        // retry: it is carried unchanged, and it does not suppress the tier
        // items below — a finished decision does not repair a broken
        // instrument, and an unobserved passing count does not douse a
        // failure that was observed.
        if !task.failed_stages.is_empty() {
            items.push(Item::PipelineFailed {
                task: task.name.clone(),
                stages: task.failed_stages.clone(),
            });
        }

        let state = run_state_view::state(&run_state_view::Sighting {
            task: task.name.clone(),
            worktrees: task.worktrees,
            live: task.live.clone(),
            scored: task.passing.is_observed(),
        });
        if !task.adjudicated && matches!(state, RunState::FinishedUnscored { .. }) {
            has_finished_unscored = true;
        }

        // A recorded decision ends the tier items: there is nothing left for
        // the pipeline or the adjudicator to do about a finished task.
        if task.adjudicated {
            continue;
        }

        // The shared run-state classifier distinguishes a finished, unscored
        // task from a live or unknown one, and `Item::ScoreIt` is the word for
        // it. This is the item the board could compute and could not say:
        // until the variant existed, a finished task yielded nothing and the
        // orchestrator was told the board was empty.
        //
        // It REPLACES the tier items rather than joining them. Both of those
        // hang off an observed passing count, and there is none yet -- that is
        // the whole point. Scoring it is what produces one.
        if let RunState::FinishedUnscored { worktrees } = state {
            items.push(Item::ScoreIt {
                task: task.name.clone(),
                worktrees,
            });
            continue;
        }

        // Both tier items hang off an observed passing count. A count that
        // was never observed is not evidence that any candidate passed, and
        // it is not evidence that none did — so no item, rather than a
        // fabricated one.
        if let Measurement::Observed(n) = &task.passing {
            if task.has_claims {
                items.push(Item::Adjudicate {
                    task: task.name.clone(),
                    passing: *n,
                });
            } else {
                items.push(Item::RunPipeline {
                    task: task.name.clone(),
                    // The stage whose absence a score file with no claims is
                    // evidence of. Pinned: a caller matching on it needs it
                    // fixed.
                    missing: "critique".to_string(),
                });
            }
        }
    }

    for wave in queued {
        items.push(Item::LaunchWave {
            matrix: wave.matrix.clone(),
            runs: wave.runs,
        });
    }

    // The empty queue is itself an observation, and it may only be made when
    // nothing else was: not beside a wave to dispatch, not beside a failed
    // pipeline, and not beside a task with an open tier. (The wave check is
    // redundant over `queued.is_empty()` and is kept as the clause states it,
    // so the two can never drift apart.)
    let something_to_do = items.iter().any(|item| {
        matches!(
            item,
            Item::Adjudicate { .. } | Item::RunPipeline { .. } | Item::PipelineFailed { .. }
        )
    });
    if queued.is_empty() && !something_to_do && !has_finished_unscored {
        items.push(Item::QueueEmpty);
    }

    Facts { items, live_agents }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    // Clauses 3, 5 and 7 are all rules about when an item is ABSENT, and an
    // implementation that emits everything always passes a test that only
    // checks a wanted item is present. So every test here asserts the exact
    // sequence `observe` returns — presence and absence pinned together —
    // instead of checking that a wanted item appears somewhere in the list.

    /// The neutral listing: a task the caller could see but knows nothing
    /// else about. Every field is the observation of an absent instrument.
    fn task(name: &str) -> TaskFiles {
        TaskFiles {
            name: name.to_string(),
            has_spec: true,
            passing: Measurement::Missing(Absent::NotAttempted),
            has_claims: false,
            adjudicated: false,
            failed_stages: Vec::new(),
            worktrees: 0,
            live: Measurement::Missing(Absent::NotAttempted),
        }
    }

    fn wave(matrix: &str, runs: usize) -> QueuedMatrix {
        QueuedMatrix {
            matrix: matrix.to_string(),
            runs,
        }
    }

    /// The `live_agents` argument most tests pass: irrelevant to the items
    /// under test, and asserted to survive untouched only where it matters.
    fn no_agents() -> Measurement<usize> {
        Measurement::Missing(Absent::NotAttempted)
    }

    #[test]
    fn failed_stages_carry_through_unchanged_and_unsorted() {
        let t = TaskFiles {
            failed_stages: vec!["grade".into(), "build".into(), "merge".into()],
            ..task("repair-lease")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::PipelineFailed {
                task: "repair-lease".to_string(),
                stages: vec!["grade".into(), "build".into(), "merge".into()],
            }]
        );
    }

    #[test]
    fn scores_with_no_claims_ever_ran_ask_for_the_critique_stage() {
        let t = TaskFiles {
            passing: Measurement::Observed(2),
            ..task("wave-plan")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::RunPipeline {
                task: "wave-plan".to_string(),
                missing: "critique".to_string(),
            }]
        );
    }

    #[test]
    fn scores_with_claims_and_no_decision_ask_for_adjudication() {
        let t = TaskFiles {
            passing: Measurement::Observed(3),
            has_claims: true,
            ..task("cell-record")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::Adjudicate {
                task: "cell-record".to_string(),
                passing: 3,
            }]
        );
    }

    #[test]
    fn an_adjudicated_task_yields_no_tier_item() {
        let t = TaskFiles {
            passing: Measurement::Observed(1),
            has_claims: true,
            adjudicated: true,
            ..task("stage-outcome")
        };
        let facts = observe(&[t], &[wave("wave90.tsv", 4)], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::LaunchWave {
                matrix: "wave90.tsv".to_string(),
                runs: 4,
            }]
        );
    }

    #[test]
    fn a_missing_passing_count_yields_no_tier_item() {
        let t = TaskFiles {
            has_claims: true,
            ..task("suite-verdict")
        };
        let facts = observe(&[t], &[wave("wave89.tsv", 6)], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::LaunchWave {
                matrix: "wave89.tsv".to_string(),
                runs: 6,
            }]
        );
    }

    #[test]
    fn a_missing_passing_count_does_not_douse_a_pipeline_failure() {
        let t = TaskFiles {
            has_claims: true,
            failed_stages: vec!["gate".to_string()],
            ..task("known-defect")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::PipelineFailed {
                task: "known-defect".to_string(),
                stages: vec!["gate".to_string()],
            }]
        );
    }

    #[test]
    fn every_queued_wave_launches_in_the_order_given() {
        let facts = observe(
            &[],
            &[wave("wave91.tsv", 12), wave("wave92.tsv", 7)],
            no_agents(),
        );
        assert_eq!(
            facts.items,
            vec![
                Item::LaunchWave {
                    matrix: "wave91.tsv".to_string(),
                    runs: 12,
                },
                Item::LaunchWave {
                    matrix: "wave92.tsv".to_string(),
                    runs: 7,
                },
            ]
        );
    }

    #[test]
    fn an_empty_listing_is_queue_empty() {
        let facts = observe(&[], &[], Measurement::Observed(0));
        assert_eq!(facts.items, vec![Item::QueueEmpty]);
    }

    #[test]
    fn a_task_that_yields_nothing_still_leaves_the_queue_empty() {
        let t = TaskFiles {
            adjudicated: true,
            ..task("lib-diff")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(facts.items, vec![Item::QueueEmpty]);
    }

    #[test]
    fn a_deleted_spec_hides_nothing() {
        let t = TaskFiles {
            has_spec: false,
            passing: Measurement::Observed(1),
            has_claims: true,
            ..task("prior")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::Adjudicate {
                task: "prior".to_string(),
                passing: 1,
            }]
        );
    }

    #[test]
    fn zero_passing_candidates_still_need_a_decision() {
        let t = TaskFiles {
            passing: Measurement::Observed(0),
            has_claims: true,
            ..task("reap-plan")
        };
        let facts = observe(&[t], &[], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::Adjudicate {
                task: "reap-plan".to_string(),
                passing: 0,
            }]
        );
    }

    #[test]
    fn a_wave_dispatching_nothing_is_still_a_wave() {
        let facts = observe(&[], &[wave("wave94.tsv", 0)], no_agents());
        assert_eq!(
            facts.items,
            vec![Item::LaunchWave {
                matrix: "wave94.tsv".to_string(),
                runs: 0,
            }]
        );
    }

    #[test]
    fn items_keep_the_order_given_with_each_failure_first_for_its_task() {
        let failed_and_unclaimed = TaskFiles {
            passing: Measurement::Observed(5),
            failed_stages: vec!["gate".to_string()],
            ..task("alpha")
        };
        let claimed = TaskFiles {
            passing: Measurement::Observed(7),
            has_claims: true,
            ..task("beta")
        };
        let facts = observe(
            &[failed_and_unclaimed, claimed],
            &[wave("wave93.tsv", 2)],
            no_agents(),
        );
        assert_eq!(
            facts.items,
            vec![
                Item::PipelineFailed {
                    task: "alpha".to_string(),
                    stages: vec!["gate".to_string()],
                },
                Item::RunPipeline {
                    task: "alpha".to_string(),
                    missing: "critique".to_string(),
                },
                Item::Adjudicate {
                    task: "beta".to_string(),
                    passing: 7,
                },
                Item::LaunchWave {
                    matrix: "wave93.tsv".to_string(),
                    runs: 2,
                },
            ]
        );
    }

    #[test]
    fn live_agents_pass_through_unchanged_when_observed() {
        let facts = observe(&[], &[], Measurement::Observed(3));
        assert_eq!(facts.live_agents, Measurement::Observed(3));
    }

    #[test]
    fn live_agents_pass_through_unchanged_when_missing() {
        let absent = Measurement::Missing(Absent::InstrumentFailed {
            reason: "ps could not be read".to_string(),
        });
        let facts = observe(&[], &[wave("wave88.tsv", 1)], absent.clone());
        assert_eq!(facts.live_agents, absent);
    }
}
