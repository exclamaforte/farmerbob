//! What the orchestrator should look at first, decided from facts alone.
//!
//! `fb-autopilot.sh` polls a log directory every two minutes, infers state
//! from which files exist, and decides what to do next. In its entire life it
//! has launched nothing, and its one autonomous behaviour was retrying a
//! broken pipeline 228 times because the failure it was checking for could
//! not be signalled. Both are failures of a decision that is small and pure:
//! given what was observed, what should a human or a loop look at FIRST, and
//! why?
//!
//! This module is that decision as a function. The caller gathers the facts —
//! `fb` will — and [`rank`] orders everything observed while [`next`] names
//! the one thing to act on first. No filesystem, no clock, no shell: the
//! caller observes, this module ranks.

use crate::measurement::{Absent, Measurement};

/// One thing the orchestrator could do, as gathered by the caller.
/// Exactly these variants and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A task whose runs are finished and which has not been adjudicated.
    Adjudicate {
        /// The task name.
        task: String,
        /// How many candidates passed the gate.
        passing: usize,
    },
    /// A task that has scores but whose subjective tier never ran.
    RunPipeline {
        /// The task name.
        task: String,
        /// The stage that is missing, as the caller names it.
        missing: String,
    },
    /// A pipeline that ran and failed. Never retried blindly.
    PipelineFailed {
        /// The task name.
        task: String,
        /// The stages that failed, in the order the pipeline runs them.
        stages: Vec<String>,
    },
    /// A matrix sitting in the queue that nothing has dispatched.
    LaunchWave {
        /// The queue file's basename.
        matrix: String,
        /// How many runs it would dispatch.
        runs: usize,
    },
    /// Nothing is queued and nothing is running.
    QueueEmpty,
}

/// Everything the caller observed. A field the caller could not read is
/// `Missing`, never a fabricated empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// Candidate items, in no particular order.
    pub items: Vec<Item>,
    /// How many agents are running right now.
    pub live_agents: Measurement<usize>,
}

/// What to look at first, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    /// The item itself.
    pub item: Item,
    /// One line a human can read. Non-empty; wording not pinned.
    pub why: String,
    /// True when acting on this is safe while agents are running.
    pub safe_while_busy: bool,
}

/// The variant's position in the pinned priority order, 0 = most urgent.
fn variant_order(item: &Item) -> u8 {
    match item {
        Item::PipelineFailed { .. } => 0,
        Item::Adjudicate { .. } => 1,
        Item::RunPipeline { .. } => 2,
        Item::LaunchWave { .. } => 3,
        Item::QueueEmpty => 4,
    }
}

/// The item's own name, the tie-break key within a variant. `QueueEmpty` has
/// none, and no tie to break: two `QueueEmpty` items carry the same fact.
fn own_name(item: &Item) -> Option<&str> {
    match item {
        Item::Adjudicate { task, .. }
        | Item::RunPipeline { task, .. }
        | Item::PipelineFailed { task, .. } => Some(task),
        Item::LaunchWave { matrix, .. } => Some(matrix),
        Item::QueueEmpty => None,
    }
}

/// The [`Attention`] for one item: one line of prose naming it, and the
/// pinned `safe_while_busy` value for its variant. Degenerate payloads — a
/// failed pipeline with no failed stages, a wave that would dispatch nothing,
/// an adjudication with no passing candidate — are facts like any other: they
/// rank normally and are described, never dropped.
fn attention_for(item: &Item) -> Attention {
    let (why, safe_while_busy) = match item {
        Item::PipelineFailed { task, stages } => {
            let why = if stages.is_empty() {
                format!(
                    "pipeline for task {task} failed with no failed stages recorded; \
                    look at the pipeline itself before trusting anything under it"
                )
            } else {
                let failed = stages.join(", ");
                format!(
                    "pipeline for task {task} failed at stage {failed}; \
                    nothing measured under it is trustworthy until this is fixed"
                )
            };
            (why, true)
        }
        Item::Adjudicate { task, passing } => (
            format!(
                "task {task} finished its runs with {passing} passing candidate(s) \
                and has not been adjudicated"
            ),
            false,
        ),
        Item::RunPipeline { task, missing } => (
            format!(
                "task {task} has scores but stage {missing} never ran; \
                the pipeline still needs to be run"
            ),
            false,
        ),
        Item::LaunchWave { matrix, runs } => (
            format!(
                "queue file {matrix} would dispatch {runs} run(s) \
                and nothing has dispatched it yet"
            ),
            false,
        ),
        Item::QueueEmpty => (
            "nothing is queued and nothing is running; \
            the empty queue is itself the observation, and the moment to write something new"
                .to_string(),
            true,
        ),
    };
    Attention {
        item: item.clone(),
        why,
        safe_while_busy,
    }
}

/// Rank what the caller observed. Highest priority first.
///
/// Every item given is returned exactly once, reordered and never filtered:
/// `rank(f).len() == f.items.len()` always. Variants order as pinned —
/// failed pipeline, adjudication, missing pipeline stage, undispatched wave,
/// empty queue — and within a variant, ties break by the item's own name,
/// ascending. Items equal in both variant and name keep their input order,
/// so two runs over the same `Facts` produce identical output.
pub fn rank(f: &Facts) -> Vec<Attention> {
    let mut ranked: Vec<Attention> = f.items.iter().map(attention_for).collect();
    ranked.sort_by(|a, b| {
        variant_order(&a.item)
            .cmp(&variant_order(&b.item))
            .then_with(|| own_name(&a.item).cmp(&own_name(&b.item)))
    });
    ranked
}

/// The single next thing, or `Missing` when there is nothing to do.
///
/// An empty `items` means the caller observed nothing, so `Missing` — never a
/// fabricated `QueueEmpty`, which is a thing the caller would have observed.
/// When agents are observed running — or when whether they run is itself
/// `Missing` — the machine is BUSY, and the first item of [`rank`] whose
/// `safe_while_busy` is true is chosen; if none is, `Missing`. The result is
/// always an element of [`rank`], carrying the same `why` that [`rank`] gave
/// it, so the two can never disagree about the same item.
pub fn next(f: &Facts) -> Measurement<Attention> {
    if f.items.is_empty() {
        return Measurement::Missing(Absent::NothingToMeasure {
            reason: "the caller observed nothing: items is empty, so there is nothing to rank"
                .to_string(),
        });
    }
    let busy = match f.live_agents.value() {
        Some(0) => false,
        Some(_) => true,
        None => true,
    };
    let chosen = if busy {
        rank(f).into_iter().find(|a| a.safe_while_busy)
    } else {
        rank(f).into_iter().next()
    };
    match chosen {
        Some(a) => Measurement::Observed(a),
        None => Measurement::Missing(Absent::NothingToMeasure {
            reason: "every ranked item starts work or merges code while agents hold worktrees"
                .to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    //! `rank` and `next` are tested together, not separately: `next` is
    //! defined as a filter over `rank`'s order, and a matched pair of bugs —
    //! a `rank` that sorted backwards and a `next` that took the last
    //! element — would pass two independent tests while the orchestrator was
    //! told to do the least urgent thing available. Every `next` assertion
    //! below therefore compares against a position computed from `rank`'s own
    //! output, never from a hand-written order.

    use super::{Attention, Facts, Item, next, rank};
    use crate::measurement::{Absent, Measurement};

    /// A task whose runs are finished, unadjudicated.
    fn adj(task: &str, passing: usize) -> Item {
        Item::Adjudicate {
            task: task.to_string(),
            passing,
        }
    }

    /// A task whose subjective tier never ran.
    fn rp(task: &str, missing: &str) -> Item {
        Item::RunPipeline {
            task: task.to_string(),
            missing: missing.to_string(),
        }
    }

    /// A pipeline that ran and failed.
    fn pf(task: &str, stages: &[&str]) -> Item {
        Item::PipelineFailed {
            task: task.to_string(),
            stages: stages.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// A queued matrix nothing has dispatched.
    fn lw(matrix: &str, runs: usize) -> Item {
        Item::LaunchWave {
            matrix: matrix.to_string(),
            runs,
        }
    }

    /// Facts with the machine observed idle.
    fn idle(items: Vec<Item>) -> Facts {
        Facts {
            items,
            live_agents: Measurement::Observed(0),
        }
    }

    /// Facts with `n` agents observed running.
    fn busy(items: Vec<Item>, n: usize) -> Facts {
        Facts {
            items,
            live_agents: Measurement::Observed(n),
        }
    }

    /// The item's own name, as the spec's tie-break defines it.
    fn own_name(item: &Item) -> Option<&str> {
        match item {
            Item::Adjudicate { task, .. }
            | Item::RunPipeline { task, .. }
            | Item::PipelineFailed { task, .. } => Some(task),
            Item::LaunchWave { matrix, .. } => Some(matrix),
            Item::QueueEmpty => None,
        }
    }

    /// Asserts the measurement is exactly `Missing(NothingToMeasure)`, without
    /// quoting the reason text, which the spec leaves as prose.
    fn assert_nothing_to_measure(m: &Measurement<Attention>) {
        assert!(
            matches!(m.absent(), Some(Absent::NothingToMeasure { .. })),
            "expected Missing(NothingToMeasure), got {m:?}"
        );
    }

    /// Unwraps an observed measurement, failing with the absence if missing.
    fn observed(m: Measurement<Attention>) -> Attention {
        match m {
            Measurement::Observed(a) => a,
            Measurement::Missing(absent) => {
                panic!("expected Observed, got Missing({absent:?})")
            }
        }
    }

    #[test]
    fn variant_priority_is_exactly_pinned() {
        let f = idle(vec![
            Item::QueueEmpty,
            lw("m", 3),
            rp("t-run", "subjective"),
            adj("t-adj", 2),
            pf("t-fail", &["gate"]),
        ]);
        let got: Vec<Item> = rank(&f).into_iter().map(|a| a.item).collect();
        assert_eq!(
            got,
            vec![
                pf("t-fail", &["gate"]),
                adj("t-adj", 2),
                rp("t-run", "subjective"),
                lw("m", 3),
                Item::QueueEmpty,
            ]
        );
    }

    #[test]
    fn rank_returns_every_item_exactly_once() {
        let items = vec![
            adj("a", 1),
            adj("a", 1),
            adj("b", 0),
            Item::QueueEmpty,
            Item::QueueEmpty,
            lw("m", 0),
        ];
        let f = idle(items);
        let ranked = rank(&f);
        assert_eq!(ranked.len(), f.items.len(), "rank must never filter");
        for item in &f.items {
            let given = f.items.iter().filter(|j| j == &item).count();
            let got = ranked.iter().filter(|a| a.item == *item).count();
            assert_eq!(
                got, given,
                "item {item:?} appears the wrong number of times"
            );
        }
    }

    #[test]
    fn within_a_variant_ties_break_by_name_ascending() {
        let f = idle(vec![
            adj("beta", 1),
            adj("alpha", 2),
            pf("zeta", &["s"]),
            pf("kappa", &["s"]),
            lw("m2", 1),
            lw("m1", 1),
        ]);
        let ranked = rank(&f);
        let names: Vec<&str> = ranked.iter().filter_map(|a| own_name(&a.item)).collect();
        assert_eq!(names, vec!["kappa", "zeta", "alpha", "beta", "m1", "m2"]);
    }

    #[test]
    fn rank_is_deterministic() {
        let f = idle(vec![adj("b", 1), pf("a", &[]), lw("m", 1), adj("c", 2)]);
        assert_eq!(rank(&f), rank(&f));
    }

    #[test]
    fn safe_while_busy_is_pinned_per_variant() {
        let f = idle(vec![
            pf("t", &["s"]),
            adj("t", 1),
            rp("t", "s"),
            lw("m", 1),
            Item::QueueEmpty,
        ]);
        for a in rank(&f) {
            let expected = matches!(a.item, Item::PipelineFailed { .. } | Item::QueueEmpty);
            assert_eq!(
                a.safe_while_busy, expected,
                "safe_while_busy wrong for {:?}",
                a.item
            );
        }
    }

    #[test]
    fn why_is_non_empty_and_names_the_item() {
        let f = idle(vec![
            pf("broken-task", &["s1"]),
            adj("adj-task", 1),
            rp("pipe-task", "subjective"),
            lw("queue-file.tsv", 4),
            Item::QueueEmpty,
        ]);
        for a in rank(&f) {
            assert!(!a.why.is_empty(), "empty why for {:?}", a.item);
            match &a.item {
                Item::PipelineFailed { task, .. }
                | Item::Adjudicate { task, .. }
                | Item::RunPipeline { task, .. } => assert!(
                    a.why.contains(task.as_str()),
                    "why {:?} does not name task {task:?}",
                    a.why
                ),
                Item::LaunchWave { matrix, .. } => assert!(
                    a.why.contains(matrix.as_str()),
                    "why {:?} does not name matrix {matrix:?}",
                    a.why
                ),
                // QueueEmpty has no task or matrix to name; non-empty is the
                // whole pinned requirement.
                Item::QueueEmpty => {}
            }
        }
    }

    #[test]
    fn next_idle_is_the_first_of_rank() {
        let f = idle(vec![adj("b", 1), pf("a", &["s"]), lw("c", 2)]);
        let first = rank(&f)[0].clone();
        assert_eq!(observed(next(&f)), first);
    }

    #[test]
    fn empty_items_is_missing_never_queue_empty() {
        let cases = [
            idle(vec![]),
            busy(vec![], 3),
            Facts {
                items: vec![],
                live_agents: Measurement::not_attempted(),
            },
        ];
        for f in &cases {
            assert_nothing_to_measure(&next(f));
        }
    }

    #[test]
    fn next_busy_takes_the_first_safe_item_of_rank() {
        let f = busy(
            vec![adj("a", 1), lw("m", 2), Item::QueueEmpty, pf("p", &["s"])],
            1,
        );
        let first_safe = rank(&f)
            .iter()
            .find(|a| a.safe_while_busy)
            .cloned()
            .unwrap();
        assert_eq!(observed(next(&f)), first_safe);
    }

    #[test]
    fn next_busy_skips_unsafe_items() {
        let f = busy(vec![adj("a", 1), lw("m", 2), Item::QueueEmpty], 1);
        let first_safe = rank(&f)
            .iter()
            .find(|a| a.safe_while_busy)
            .cloned()
            .unwrap();
        let got = observed(next(&f));
        assert_eq!(got, first_safe);
        assert!(
            matches!(got.item, Item::QueueEmpty),
            "busy next must skip every unsafe item, chose {:?}",
            got.item
        );
    }

    #[test]
    fn next_busy_prefers_the_highest_ranked_safe_item() {
        // Both safe items present: the failed pipeline outranks the empty
        // queue, so a `next` that took the LAST safe element fails here.
        let f = busy(vec![Item::QueueEmpty, pf("p", &["s"])], 1);
        assert!(matches!(
            observed(next(&f)).item,
            Item::PipelineFailed { .. }
        ));
    }

    #[test]
    fn next_busy_without_safe_items_is_missing() {
        let f = busy(vec![adj("a", 1), rp("b", "subjective"), lw("m", 1)], 2);
        assert_nothing_to_measure(&next(&f));
    }

    #[test]
    fn missing_agent_count_acts_busy() {
        let skip = Facts {
            items: vec![adj("a", 1), Item::QueueEmpty],
            live_agents: Measurement::not_attempted(),
        };
        assert!(matches!(observed(next(&skip)).item, Item::QueueEmpty));
        let none_safe = Facts {
            items: vec![adj("a", 1)],
            live_agents: Measurement::instrument_failed("the grep died"),
        };
        assert_nothing_to_measure(&next(&none_safe));
        let safe_top = Facts {
            items: vec![pf("p", &["s"])],
            live_agents: Measurement::untrusted("the counter is stale"),
        };
        assert!(matches!(
            observed(next(&safe_top)).item,
            Item::PipelineFailed { .. }
        ));
    }

    #[test]
    fn zero_agents_is_idle_and_one_is_busy() {
        let items = vec![adj("a", 1), Item::QueueEmpty];
        let at_zero = Facts {
            items: items.clone(),
            live_agents: Measurement::Observed(0),
        };
        assert!(matches!(
            observed(next(&at_zero)).item,
            Item::Adjudicate { .. }
        ));
        let at_one = Facts {
            items,
            live_agents: Measurement::Observed(1),
        };
        assert!(matches!(observed(next(&at_one)).item, Item::QueueEmpty));
    }

    #[test]
    fn degenerate_items_still_rank_and_name_their_item() {
        let f = idle(vec![
            pf("dead", &[]),
            lw("empty-matrix", 0),
            adj("zero-pass", 0),
        ]);
        let ranked = rank(&f);
        assert_eq!(ranked.len(), 3, "degenerate items must not be dropped");
        assert!(
            matches!(ranked[0].item, Item::PipelineFailed { .. }),
            "an empty-stages failure still ranks first"
        );
        assert!(ranked[0].why.contains("dead"));
        assert!(ranked[1].why.contains("zero-pass"));
        assert!(ranked[2].why.contains("empty-matrix"));
    }

    #[test]
    fn one_task_in_two_variants_ranks_both_in_variant_order() {
        let f = idle(vec![adj("same", 1), pf("same", &["gate"])]);
        let ranked = rank(&f);
        assert_eq!(ranked.len(), 2, "rank deduplicates nothing");
        assert!(matches!(ranked[0].item, Item::PipelineFailed { .. }));
        assert!(matches!(ranked[1].item, Item::Adjudicate { .. }));
    }

    #[test]
    fn rank_ignores_live_agents() {
        let items = vec![adj("a", 1), Item::QueueEmpty, pf("p", &[])];
        let idle_board = rank(&Facts {
            items: items.clone(),
            live_agents: Measurement::Observed(0),
        });
        let deep_board = rank(&Facts {
            items: items.clone(),
            live_agents: Measurement::Observed(9),
        });
        let dark_board = rank(&Facts {
            items,
            live_agents: Measurement::not_attempted(),
        });
        assert_eq!(idle_board, deep_board);
        assert_eq!(deep_board, dark_board);
    }

    #[test]
    fn a_single_observed_queue_empty_is_a_real_result() {
        // The contrast with `empty_items_is_missing_never_queue_empty`: an
        // observed QueueEmpty is a fact, an empty list is not.
        let f = idle(vec![Item::QueueEmpty]);
        let got = observed(next(&f));
        assert!(matches!(got.item, Item::QueueEmpty));
        assert!(got.safe_while_busy);
        assert!(!got.why.is_empty());
    }
}
