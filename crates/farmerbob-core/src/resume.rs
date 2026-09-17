//! Ordered, rationed resumption of runs parked on quota buckets.
//!
//! Resuming everything the instant a bucket resets fires the largest burst of
//! the day at the provider that just rate-limited the system. This module
//! plans releases per bucket instead: oldest-waiting first, at most `batch`
//! per bucket per call, spread by `stagger_ms`. It is pure logic: the caller
//! supplies `now_ms` and acts on the returned plan.

use std::collections::{HashMap, HashSet};

/// A run waiting on a bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    /// Identifier of the parked run.
    pub run_id: String,
    /// Name of the bucket the run is parked against.
    pub bucket: String,
    /// Instant the run was parked, in milliseconds.
    pub since_ms: u64,
    /// Absolute instant the provider stated, if it stated one.
    pub resume_at_ms: Option<u64>,
    /// How many times this run has already been resumed and blocked again.
    pub attempts: u32,
}

/// Policy controlling how parked runs are released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Most runs released per bucket per call. Must be at least 1; 0 is treated as 1.
    pub batch: usize,
    /// Minimum gap between releases within one bucket.
    pub stagger_ms: u64,
    /// Fallback wait when the provider stated no reset instant.
    pub default_window_ms: u64,
    /// A run blocked this many times is not resumed again; it is `Exhausted`.
    pub max_attempts: u32,
}

/// What to do with one parked run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Release at this instant, at or after `now_ms`.
    Resume { run_id: String, at_ms: u64 },
    /// Not yet due. Carries the instant it becomes due.
    Wait { run_id: String, until_ms: u64 },
    /// Blocked too many times; stop trying and surface it.
    Exhausted { run_id: String },
}

/// Returns `true` when the run may be released at `now_ms`.
///
/// A stated reset is due when `now_ms >= resume_at_ms`. Otherwise the run is
/// due once `now_ms - since_ms >= default_window_ms`. A `since_ms` in the
/// future is never due and never underflows.
fn is_due(run: &Parked, default_window_ms: u64, now_ms: u64) -> bool {
    match run.resume_at_ms {
        Some(at) => now_ms >= at,
        None => match now_ms.checked_sub(run.since_ms) {
            Some(elapsed) => elapsed >= default_window_ms,
            None => false,
        },
    }
}

/// Returns the instant at which the run becomes due.
///
/// For a stated reset this is `resume_at_ms`, otherwise
/// `since_ms + default_window_ms` saturating.
fn due_at(run: &Parked, default_window_ms: u64) -> u64 {
    match run.resume_at_ms {
        Some(at) => at,
        None => run.since_ms.saturating_add(default_window_ms),
    }
}

/// Returns `true` when the run has blocked too many times to be resumed.
fn is_exhausted(run: &Parked, max_attempts: u32) -> bool {
    run.attempts >= max_attempts
}

/// Returns the effective per-bucket batch, treating 0 as 1.
fn effective_batch(batch: usize) -> usize {
    batch.max(1)
}

/// Plan the next releases. Returns one Decision per input run, in the input order.
///
/// Within a bucket, runs are released oldest `since_ms` first -- a run that has waited
/// longest goes first, so resumption cannot starve anyone.
pub fn plan(parked: &[Parked], p: Policy, now_ms: u64) -> Vec<Decision> {
    let batch = effective_batch(p.batch);

    // Group candidate (due, non-exhausted) indices by bucket, then keep the
    // oldest `batch` per bucket in (since_ms, run_id) order.
    let mut by_bucket: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, run) in parked.iter().enumerate() {
        if is_exhausted(run, p.max_attempts) {
            continue;
        }
        if !is_due(run, p.default_window_ms, now_ms) {
            continue;
        }
        by_bucket.entry(run.bucket.as_str()).or_default().push(index);
    }

    // Rank within the bucket determines the staggered release instant.
    let mut release_at: HashMap<usize, u64> = HashMap::new();
    for indices in by_bucket.values() {
        let mut ordered: Vec<(u64, &str, usize)> = indices
            .iter()
            .filter_map(|index| {
                parked
                    .get(*index)
                    .map(|run| (run.since_ms, run.run_id.as_str(), *index))
            })
            .collect();
        ordered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        for (rank, (_, _, index)) in ordered.iter().take(batch).enumerate() {
            let offset = (rank as u64).saturating_mul(p.stagger_ms);
            release_at.insert(*index, now_ms.saturating_add(offset));
        }
    }

    parked
        .iter()
        .enumerate()
        .map(|(index, run)| {
            if is_exhausted(run, p.max_attempts) {
                Decision::Exhausted {
                    run_id: run.run_id.clone(),
                }
            } else if let Some(at) = release_at.get(&index).copied() {
                Decision::Resume {
                    run_id: run.run_id.clone(),
                    at_ms: at,
                }
            } else {
                Decision::Wait {
                    run_id: run.run_id.clone(),
                    until_ms: due_at(run, p.default_window_ms),
                }
            }
        })
        .collect()
}

/// The run ids `plan` would release, in release order. A convenience over `plan`, and it
/// must agree with it exactly.
pub fn due_now(parked: &[Parked], p: Policy, now_ms: u64) -> Vec<String> {
    let decisions = plan(parked, p, now_ms);
    // Release order is chronological: earliest `at_ms` first, with ties broken
    // oldest-waiting first and then by id so the order is deterministic and
    // independent of input order.
    let mut releases: Vec<(u64, u64, String)> = Vec::new();
    for (run, decision) in parked.iter().zip(decisions.iter()) {
        if let Decision::Resume { run_id, at_ms } = decision {
            releases.push((*at_ms, run.since_ms, run_id.clone()));
        }
    }
    releases.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    releases.into_iter().map(|(_, _, id)| id).collect()
}

/// Remove runs already handed out, by id. Resumption is at-most-once per plan: calling
/// `plan` twice without recording the releases must not produce two live agents.
pub fn without(parked: &[Parked], released: &[String]) -> Vec<Parked> {
    let handed_out: HashSet<&str> = released.iter().map(String::as_str).collect();
    parked
        .iter()
        .filter(|run| !handed_out.contains(run.run_id.as_str()))
        .cloned()
        .collect()
}

/// The earliest instant at which `plan` would return any `Resume`, or `None` when every
/// run is exhausted. This is what a scheduler sleeps until.
pub fn next_wakeup(parked: &[Parked], p: Policy, now_ms: u64) -> Option<u64> {
    if parked.is_empty() {
        return None;
    }
    let decisions = plan(parked, p, now_ms);
    if decisions
        .iter()
        .any(|d| matches!(d, Decision::Resume { .. }))
    {
        return Some(now_ms);
    }
    decisions
        .iter()
        .filter_map(|d| match d {
            Decision::Wait { until_ms, .. } => Some(*until_ms),
            _ => None,
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            batch: 2,
            stagger_ms: 100,
            default_window_ms: 1_000,
            max_attempts: 3,
        }
    }

    fn parked(id: &str, bucket: &str, since_ms: u64, resume_at_ms: Option<u64>) -> Parked {
        Parked {
            run_id: id.into(),
            bucket: bucket.into(),
            since_ms,
            resume_at_ms,
            attempts: 0,
        }
    }

    #[test]
    fn two_buckets_each_release_their_own_batch() {
        let p = policy();
        let runs = vec![
            parked("a1", "a", 0, Some(0)),
            parked("a2", "a", 1, Some(0)),
            parked("a3", "a", 2, Some(0)),
            parked("b1", "b", 0, Some(0)),
            parked("b2", "b", 1, Some(0)),
            parked("b3", "b", 2, Some(0)),
        ];
        let decisions = plan(&runs, p, 10);
        let resumed: Vec<&str> = decisions
            .iter()
            .filter_map(|d| match d {
                Decision::Resume { run_id, .. } => Some(run_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(resumed.len(), 4);
        assert!(resumed.contains(&"a1"));
        assert!(resumed.contains(&"a2"));
        assert!(resumed.contains(&"b1"));
        assert!(resumed.contains(&"b2"));
        // Input order is preserved by plan.
        let ids: Vec<&str> = decisions
            .iter()
            .map(|d| match d {
                Decision::Resume { run_id, .. } => run_id.as_str(),
                Decision::Wait { run_id, .. } => run_id.as_str(),
                Decision::Exhausted { run_id } => run_id.as_str(),
            })
            .collect();
        assert_eq!(ids, vec!["a1", "a2", "a3", "b1", "b2", "b3"]);
    }

    #[test]
    fn the_k_th_release_is_staggered() {
        let p = policy();
        let runs = vec![
            parked("a", "x", 0, Some(0)),
            parked("b", "x", 1, Some(0)),
        ];
        let decisions = plan(&runs, p, 500);
        assert_eq!(
            decisions[0],
            Decision::Resume {
                run_id: "a".into(),
                at_ms: 500,
            }
        );
        assert_eq!(
            decisions[1],
            Decision::Resume {
                run_id: "b".into(),
                at_ms: 600,
            }
        );
    }

    #[test]
    fn oldest_first_ordering_with_a_deterministic_tie_break() {
        let p = Policy { batch: 1, ..policy() };
        // Youngest first in the input; oldest must win the single slot.
        let runs = vec![
            parked("young", "x", 200, Some(0)),
            parked("old", "x", 100, Some(0)),
        ];
        let decisions = plan(&runs, p, 999);
        assert!(matches!(&decisions[0], Decision::Wait { .. }));
        assert!(matches!(
            &decisions[1],
            Decision::Resume { run_id, .. } if run_id == "old"
        ));
        // Equal age breaks by run_id ascending.
        let tied = vec![
            parked("b", "x", 50, Some(0)),
            parked("a", "x", 50, Some(0)),
        ];
        let decisions = plan(&tied, p, 999);
        assert!(matches!(
            &decisions[1],
            Decision::Resume { run_id, .. } if run_id == "a"
        ));
        assert!(matches!(&decisions[0], Decision::Wait { .. }));
    }

    #[test]
    fn an_exhausted_run_does_not_consume_a_slot() {
        let p = Policy {
            batch: 1,
            ..policy()
        };
        let mut old = parked("old", "x", 0, Some(0));
        old.attempts = 3;
        let runs = vec![old, parked("next", "x", 1, Some(0))];
        let decisions = plan(&runs, p, 50);
        assert_eq!(
            decisions[0],
            Decision::Exhausted {
                run_id: "old".into()
            }
        );
        assert!(matches!(
            &decisions[1],
            Decision::Resume { run_id, at_ms: 50 } if run_id == "next"
        ));
    }

    #[test]
    fn a_since_ms_in_the_future_does_not_underflow_and_is_not_due() {
        let p = policy();
        let runs = vec![parked("f", "x", 10_000, None)];
        let decisions = plan(&runs, p, 100);
        assert_eq!(
            decisions[0],
            Decision::Wait {
                run_id: "f".into(),
                until_ms: 11_000,
            }
        );
        // A stated reset in the future is also not due.
        let runs = vec![parked("g", "x", 0, Some(5_000))];
        let decisions = plan(&runs, p, 100);
        assert_eq!(
            decisions[0],
            Decision::Wait {
                run_id: "g".into(),
                until_ms: 5_000,
            }
        );
    }

    #[test]
    fn due_now_agrees_with_plan() {
        let p = policy();
        let runs = vec![
            parked("c", "x", 30, Some(0)),
            parked("a", "x", 10, Some(0)),
            parked("b", "y", 5, Some(0)),
            parked("d", "x", 40, None),
        ];
        let now = 5_000;
        let decisions = plan(&runs, p, now);
        let due = due_now(&runs, p, now);
        // Same membership as the Resume decisions in the plan.
        let mut from_plan: Vec<String> = decisions
            .iter()
            .filter_map(|d| match d {
                Decision::Resume { run_id, .. } => Some(run_id.clone()),
                _ => None,
            })
            .collect();
        let mut from_due = due.clone();
        from_plan.sort();
        from_due.sort();
        assert_eq!(from_plan, from_due);
        // Release order: non-decreasing release instant, and oldest-first
        // within each bucket.
        let at_of = |id: &str| match decisions.iter().find(|d| match d {
            Decision::Resume { run_id, .. } => run_id == id,
            _ => false,
        }) {
            Some(Decision::Resume { at_ms, .. }) => *at_ms,
            _ => now,
        };
        let mut last_at = now;
        for id in &due {
            let at = at_of(id);
            assert!(at >= last_at, "due_now not in release order");
            last_at = at;
        }
        for bucket in ["x", "y"] {
            let in_bucket: Vec<&String> =
                due.iter().filter(|id| runs.iter().any(|r| &r.run_id == *id && r.bucket == bucket)).collect();
            let sinces: Vec<u64> = in_bucket
                .iter()
                .map(|id| runs.iter().find(|r| &r.run_id == *id).map_or(0, |r| r.since_ms))
                .collect();
            let mut sorted = sinces.clone();
            sorted.sort();
            assert_eq!(sinces, sorted, "bucket {bucket} not oldest-first");
        }
    }

    #[test]
    fn next_wakeup_is_none_when_all_are_exhausted() {
        let p = policy();
        let mut a = parked("a", "x", 0, Some(0));
        a.attempts = 3;
        let mut b = parked("b", "y", 0, None);
        b.attempts = 99;
        assert_eq!(next_wakeup(&[a, b], p, 10), None);
        assert_eq!(next_wakeup(&[], p, 10), None);
    }

    #[test]
    fn batch_0_behaves_as_1() {
        let p = Policy {
            batch: 0,
            ..policy()
        };
        let runs = vec![
            parked("a", "x", 0, Some(0)),
            parked("b", "x", 1, Some(0)),
        ];
        let decisions = plan(&runs, p, 70);
        assert!(matches!(
            &decisions[0],
            Decision::Resume { run_id, at_ms: 70 } if run_id == "a"
        ));
        assert!(matches!(&decisions[1], Decision::Wait { .. }));
    }

    #[test]
    fn stagger_0_releases_the_whole_batch_at_now() {
        let p = Policy {
            batch: 3,
            stagger_ms: 0,
            ..policy()
        };
        let runs = vec![
            parked("a", "x", 2, Some(0)),
            parked("b", "x", 1, Some(0)),
        ];
        let decisions = plan(&runs, p, 77);
        assert!(decisions.iter().all(|d| matches!(
            d,
            Decision::Resume { at_ms: 77, .. }
        )));
    }

    #[test]
    fn fallback_window_and_saturating_wait_instant() {
        let p = policy();
        // Exactly at the boundary counts as due.
        let runs = vec![parked("a", "x", 0, None)];
        assert!(matches!(plan(&runs, p, 1_000)[0], Decision::Resume { .. }));
        assert!(matches!(plan(&runs, p, 999)[0], Decision::Wait { .. }));
        // Saturates instead of wrapping.
        let runs = vec![parked("s", "x", u64::MAX - 10, None)];
        assert_eq!(
            plan(&runs, p, 0)[0],
            Decision::Wait {
                run_id: "s".into(),
                until_ms: u64::MAX,
            }
        );
    }

    #[test]
    fn next_wakeup_covers_due_and_future_waits() {
        let p = policy();
        let runs = vec![parked("a", "x", 0, Some(0))];
        assert_eq!(next_wakeup(&runs, p, 10), Some(10));
        let runs = vec![parked("a", "x", 0, Some(500))];
        assert_eq!(next_wakeup(&runs, p, 10), Some(500));
        let runs = vec![parked("a", "x", 100, None)];
        assert_eq!(next_wakeup(&runs, p, 10), Some(1_100));
    }

    #[test]
    fn without_removes_handed_out_runs() {
        let runs = vec![
            parked("a", "x", 0, Some(0)),
            parked("b", "x", 1, Some(0)),
        ];
        let rest = without(&runs, &["a".to_string()]);
        assert_eq!(rest, vec![runs[1].clone()]);
        // Planning twice without recording must not hand out the same run again.
        let p = policy();
        let first = due_now(&runs, p, 100);
        let rest = without(&runs, &first);
        assert!(due_now(&rest, p, 100).is_empty());
    }
}


// ESCALATED from cross-examination: codex-luna's suite discriminated on resume.
// Not a CLAIM -- cross-examination found it directly. Kept only because it passes
// against the merged winner, which is what separates a discovery from an
// over-fitted suite.
#[cfg(test)]
mod cx_resume_codex_luna {
    use super::*;

    fn run(id: &str, bucket: &str, since_ms: u64) -> Parked {
        Parked {
            run_id: id.into(),
            bucket: bucket.into(),
            since_ms,
            resume_at_ms: Some(10),
            attempts: 0,
        }
    }

    fn policy(batch: usize) -> Policy {
        Policy {
            batch,
            stagger_ms: 5,
            default_window_ms: 10,
            max_attempts: 3,
        }
    }

    #[test]
    fn two_buckets_each_release_their_own_batch() {
        let parked = [
            run("a1", "a", 1),
            run("b1", "b", 2),
            run("a2", "a", 3),
            run("b2", "b", 4),
        ];
        assert_eq!(due_now(&parked, policy(1), 10), vec!["a1", "b1"]);
    }

    #[test]
    fn kth_release_is_staggered() {
        let parked = [run("a", "x", 1), run("b", "x", 2)];
        assert_eq!(
            plan(&parked, policy(2), 10),
            vec![
                Decision::Resume {
                    run_id: "a".into(),
                    at_ms: 10
                },
                Decision::Resume {
                    run_id: "b".into(),
                    at_ms: 15
                },
            ]
        );
    }

    #[test]
    fn oldest_first_ordering_has_a_deterministic_tie_break() {
        let parked = [run("z", "x", 2), run("b", "x", 1), run("a", "x", 1)];
        assert_eq!(due_now(&parked, policy(3), 10), vec!["a", "b", "z"]);
    }

    #[test]
    fn exhausted_run_does_not_consume_a_slot() {
        let mut exhausted = run("old", "x", 1);
        exhausted.attempts = 3;
        let parked = [exhausted, run("live", "x", 2)];
        assert_eq!(due_now(&parked, policy(1), 10), vec!["live"]);
    }

    #[test]
    fn future_since_does_not_underflow_and_is_not_due() {
        let parked = [Parked {
            resume_at_ms: None,
            ..run("future", "x", u64::MAX)
        }];
        assert_eq!(
            plan(&parked, policy(1), 0),
            vec![Decision::Wait {
                run_id: "future".into(),
                until_ms: u64::MAX
            }]
        );
    }

    #[test]
    fn due_now_agrees_with_plan() {
        let parked = [run("a", "x", 1), run("b", "x", 2), run("c", "y", 3)];
        let mut expected: Vec<(u64, String)> = plan(&parked, policy(2), 10)
            .into_iter()
            .filter_map(|d| match d {
                Decision::Resume { run_id, at_ms } => Some((at_ms, run_id)),
                _ => None,
            })
            .collect();
        expected.sort_by_key(|release| release.0);
        assert_eq!(
            due_now(&parked, policy(2), 10),
            expected
                .into_iter()
                .map(|(_, run_id)| run_id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn next_wakeup_is_none_when_all_are_exhausted() {
        let mut parked_run = run("done", "x", 1);
        parked_run.attempts = 3;
        assert_eq!(next_wakeup(&[parked_run], policy(1), 0), None);
    }

    #[test]
    fn batch_zero_behaves_as_one() {
        let parked = [run("a", "x", 1), run("b", "x", 2)];
        assert_eq!(due_now(&parked, policy(0), 10), vec!["a"]);
    }
}
