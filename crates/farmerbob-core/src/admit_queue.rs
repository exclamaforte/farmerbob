//! Admit queue: flatten a wave manifest into a dispatch queue with
//! concurrency caps per upstream vendor bucket.
//!
//! Three rules govern the queue:
//! 1. An arm absent from the registry aborts the wave (typo detection).
//! 2. Concurrency is capped per vendor bucket, not per machine.
//! 3. Manifest order is preserved.

use std::collections::BTreeMap;

/// What the registry knows about one arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// Registered and dispatchable. Carries its upstream vendor bucket.
    Enabled { bucket: String },
    /// Registered, but not to be dispatched. Carries a non-empty reason.
    Disabled(String),
    /// Not in the registry at all.
    Unregistered,
}

/// One row of the wave manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub task: String,
    pub crate_name: String,
    /// Arms, in the order the manifest lists them.
    pub arms: Vec<String>,
}

/// One dispatchable run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub arm: String,
    pub task: String,
    pub crate_name: String,
    /// The vendor bucket this run counts against.
    pub bucket: String,
}

/// An arm that was dropped, and why. Reported so a skipped arm is visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub arm: String,
    pub task: String,
    pub reason: String,
}

/// Why no queue could be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Abort {
    /// An arm is not registered. Carries its name and its task.
    Unregistered { arm: String, task: String },
    /// Every arm in every row was skipped.
    NoEligibleRuns,
}

/// Flatten a manifest into a dispatch queue.
///
/// `lookup` answers for any arm name. The first unregistered arm aborts, and nothing
/// after it is examined.
pub fn queue(
    rows: &[Row],
    lookup: &dyn Fn(&str) -> Registration,
) -> Result<(Vec<Run>, Vec<Skipped>), Abort> {
    let mut runs = Vec::new();
    let mut skipped = Vec::new();
    let mut any_enabled = false;

    for row in rows {
        for arm in &row.arms {
            match lookup(arm) {
                Registration::Enabled { bucket } => {
                    any_enabled = true;
                    runs.push(Run {
                        arm: arm.clone(),
                        task: row.task.clone(),
                        crate_name: row.crate_name.clone(),
                        bucket,
                    });
                }
                Registration::Disabled(reason) => {
                    skipped.push(Skipped {
                        arm: arm.clone(),
                        task: row.task.clone(),
                        reason,
                    });
                }
                Registration::Unregistered => {
                    return Err(Abort::Unregistered {
                        arm: arm.clone(),
                        task: row.task.clone(),
                    });
                }
            }
        }
    }

    if !any_enabled {
        return Err(Abort::NoEligibleRuns);
    }

    Ok((runs, skipped))
}

/// Whether one more run against `bucket` may start now.
///
/// `in_flight` counts runs currently dispatched, by bucket.
pub fn may_start(bucket: &str, in_flight: &BTreeMap<String, usize>, cap: usize) -> bool {
    let count = in_flight.get(bucket).copied().unwrap_or(0);
    count < cap
}

/// The next run in `queue` that may start, given what is in flight, or `None`.
///
/// Does not reorder the queue: it returns the index of the FIRST run whose bucket
/// has room, so a blocked vendor does not block the wave behind it.
pub fn next_dispatchable(
    queue: &[Run],
    in_flight: &BTreeMap<String, usize>,
    cap: usize,
) -> Option<usize> {
    for (idx, run) in queue.iter().enumerate() {
        if may_start(&run.bucket, in_flight, cap) {
            return Some(idx);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(task: &str, crate_name: &str, arms: &[&str]) -> Row {
        Row {
            task: task.to_string(),
            crate_name: crate_name.to_string(),
            arms: arms.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn run(arm: &str, task: &str, crate_name: &str, bucket: &str) -> Run {
        Run {
            arm: arm.to_string(),
            task: task.to_string(),
            crate_name: crate_name.to_string(),
            bucket: bucket.to_string(),
        }
    }

    fn skipped(arm: &str, task: &str, reason: &str) -> Skipped {
        Skipped {
            arm: arm.to_string(),
            task: task.to_string(),
            reason: reason.to_string(),
        }
    }

    // Clause 1: A row of three Enabled arms yields three Runs, in manifest order,
    // each carrying the row's task and crate and its own bucket.
    #[test]
    fn clause1_three_enabled_arms_yield_three_runs_in_order() {
        let rows = vec![row("t1", "c1", &["a1", "a2", "a3"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Enabled {
                bucket: "b2".into(),
            },
            "a3" => Registration::Enabled {
                bucket: "b3".into(),
            },
            _ => Registration::Unregistered,
        };
        let (runs, skipped) = queue(&rows, &lookup).unwrap();
        assert_eq!(skipped.len(), 0);
        assert_eq!(
            runs,
            vec![
                run("a1", "t1", "c1", "b1"),
                run("a2", "t1", "c1", "b2"),
                run("a3", "t1", "c1", "b3"),
            ]
        );
    }

    // Clause 2: A Disabled arm is omitted from the queue and appears in Skipped
    // with the registry's reason carried unchanged. The wave is NOT aborted.
    #[test]
    fn clause2_disabled_arm_is_skipped_with_reason_wave_continues() {
        let rows = vec![row("t1", "c1", &["a1", "a2", "a3"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Disabled("quota exhausted".into()),
            "a3" => Registration::Enabled {
                bucket: "b3".into(),
            },
            _ => Registration::Unregistered,
        };
        let (runs, skips) = queue(&rows, &lookup).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], run("a1", "t1", "c1", "b1"));
        assert_eq!(runs[1], run("a3", "t1", "c1", "b3"));
        assert_eq!(skips, vec![skipped("a2", "t1", "quota exhausted")]);
    }

    // Clause 3: An Unregistered arm yields Err(Abort::Unregistered) naming that
    // arm and its task.
    #[test]
    fn clause3_unregistered_arm_aborts_with_name_and_task() {
        let rows = vec![row("t1", "c1", &["a1", "a2", "a3"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Unregistered,
            "a3" => Registration::Enabled {
                bucket: "b3".into(),
            },
            _ => Registration::Unregistered,
        };
        let err = queue(&rows, &lookup).unwrap_err();
        assert_eq!(
            err,
            Abort::Unregistered {
                arm: "a2".into(),
                task: "t1".into(),
            }
        );
    }

    // Clause 4: Disabled vs Unregistered distinction - same manifest, different
    // lookup answers. Disabled skips; Unregistered aborts.
    #[test]
    fn clause4_disabled_versus_unregistered_same_manifest_different_lookup() {
        let rows = vec![row("t1", "c1", &["a1", "a2"])];

        // Disabled: wave continues
        let lookup_disabled = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Disabled("reason".into()),
            _ => Registration::Unregistered,
        };
        let (runs_d, skipped_d) = queue(&rows, &lookup_disabled).unwrap();
        assert_eq!(runs_d.len(), 1);
        assert_eq!(skipped_d.len(), 1);

        // Unregistered: wave aborts
        let lookup_unreg = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Unregistered,
            _ => Registration::Unregistered,
        };
        let err = queue(&rows, &lookup_unreg).unwrap_err();
        assert!(matches!(err, Abort::Unregistered { arm, task } if arm == "a2" && task == "t1"));
    }

    // Clause 5: may_start is true when in-flight < cap, false when == cap, false
    // when > cap. Pin all three including greater.
    #[test]
    fn clause5_may_start_true_when_under_cap() {
        let in_flight: BTreeMap<String, usize> = [("b1".into(), 0)].into_iter().collect();
        assert!(may_start("b1", &in_flight, 1));
    }

    #[test]
    fn clause5_may_start_false_when_at_cap() {
        let in_flight: BTreeMap<String, usize> = [("b1".into(), 1)].into_iter().collect();
        assert!(!may_start("b1", &in_flight, 1));
    }

    #[test]
    fn clause5_may_start_false_when_over_cap() {
        let in_flight: BTreeMap<String, usize> = [("b1".into(), 2)].into_iter().collect();
        assert!(!may_start("b1", &in_flight, 1));
    }

    // Clause 6: A bucket absent from in_flight counts as zero.
    #[test]
    fn clause6_absent_bucket_counts_as_zero() {
        let in_flight: BTreeMap<String, usize> = BTreeMap::new();
        assert!(may_start("b1", &in_flight, 1));
        assert!(!may_start("b1", &in_flight, 0));
    }

    // Clause 7: Two arms with the SAME bucket count against one another.
    // Pin a queue of two arms sharing a bucket under cap:1: next_dispatchable
    // with one of them in flight must not return the other.
    #[test]
    fn clause7_shared_bucket_blocks_both_under_cap_one() {
        let queue = vec![
            run("a1", "t1", "c1", "shared"),
            run("a2", "t1", "c1", "shared"),
        ];
        let in_flight: BTreeMap<String, usize> = [("shared".into(), 1)].into_iter().collect();
        assert_eq!(next_dispatchable(&queue, &in_flight, 1), None);
    }

    // Clause 8: next_dispatchable skips a blocked run and returns a LATER one
    // whose bucket has room. Pin queue where index 0 blocked, index 1 not:
    // answer is 1, not None.
    #[test]
    fn clause8_next_dispatchable_skips_blocked_returns_later() {
        let queue = vec![run("a1", "t1", "c1", "b1"), run("a2", "t1", "c1", "b2")];
        let in_flight: BTreeMap<String, usize> = [("b1".into(), 1)].into_iter().collect();
        assert_eq!(next_dispatchable(&queue, &in_flight, 1), Some(1));
    }

    // Clause 9: next_dispatchable returns None when every remaining run's bucket is full.
    #[test]
    fn clause9_next_dispatchable_none_when_all_full() {
        let queue = vec![run("a1", "t1", "c1", "b1"), run("a2", "t1", "c1", "b2")];
        let in_flight: BTreeMap<String, usize> =
            [("b1".into(), 1), ("b2".into(), 1)].into_iter().collect();
        assert_eq!(next_dispatchable(&queue, &in_flight, 1), None);
    }

    // Clause 10: Rows processed in order, arms within row in order; queue is concatenation.
    #[test]
    fn clause10_rows_and_arms_order_preserved_concatenation() {
        let rows = vec![
            row("t1", "c1", &["a1", "a2"]),
            row("t2", "c2", &["a3", "a4"]),
        ];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Enabled {
                bucket: "b2".into(),
            },
            "a3" => Registration::Enabled {
                bucket: "b3".into(),
            },
            "a4" => Registration::Enabled {
                bucket: "b4".into(),
            },
            _ => Registration::Unregistered,
        };
        let (runs, _) = queue(&rows, &lookup).unwrap();
        assert_eq!(
            runs,
            vec![
                run("a1", "t1", "c1", "b1"),
                run("a2", "t1", "c1", "b2"),
                run("a3", "t2", "c2", "b3"),
                run("a4", "t2", "c2", "b4"),
            ]
        );
    }

    // Boundary: ZERO rows -> Err(NoEligibleRuns)
    #[test]
    fn boundary_zero_rows_is_no_eligible_runs() {
        let rows: Vec<Row> = vec![];
        let lookup = |_: &str| Registration::Unregistered;
        let err = queue(&rows, &lookup).unwrap_err();
        assert_eq!(err, Abort::NoEligibleRuns);
    }

    // Boundary: Empty manifest (all disabled) -> Err(NoEligibleRuns)
    #[test]
    fn boundary_all_disabled_is_no_eligible_runs() {
        let rows = vec![row("t1", "c1", &["a1", "a2"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Disabled("r1".into()),
            "a2" => Registration::Disabled("r2".into()),
            _ => Registration::Unregistered,
        };
        let err = queue(&rows, &lookup).unwrap_err();
        assert_eq!(err, Abort::NoEligibleRuns);
    }

    // Boundary: A row with ZERO arms contributes nothing and is not an error by itself.
    #[test]
    fn boundary_row_with_zero_arms_contributes_nothing() {
        let rows = vec![row("t1", "c1", &[]), row("t2", "c2", &["a1"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            _ => Registration::Unregistered,
        };
        let (runs, skipped) = queue(&rows, &lookup).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0], run("a1", "t2", "c2", "b1"));
        assert!(skipped.is_empty());
    }

    // Boundary: cap:0 -> may_start false for every bucket including absent,
    // next_dispatchable always None.
    #[test]
    fn boundary_cap_zero_may_start_always_false() {
        let in_flight: BTreeMap<String, usize> = BTreeMap::new();
        assert!(!may_start("b1", &in_flight, 0));
        assert!(!may_start("any", &in_flight, 0));
    }

    #[test]
    fn boundary_cap_zero_next_dispatchable_always_none() {
        let queue = vec![run("a1", "t1", "c1", "b1")];
        let in_flight: BTreeMap<String, usize> = BTreeMap::new();
        assert_eq!(next_dispatchable(&queue, &in_flight, 0), None);
    }

    // Boundary: cap:1 with empty in_flight -> first run dispatchable.
    #[test]
    fn boundary_cap_one_empty_in_flight_first_dispatchable() {
        let queue = vec![run("a1", "t1", "c1", "b1")];
        let in_flight: BTreeMap<String, usize> = BTreeMap::new();
        assert_eq!(next_dispatchable(&queue, &in_flight, 1), Some(0));
    }

    // Composition: runs and skips partition the eligible-checked arms.
    #[test]
    fn composition_runs_and_skips_partition_arms() {
        let rows = vec![row("t1", "c1", &["a1", "a2", "a3", "a4"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Disabled("reason".into()),
            "a3" => Registration::Enabled {
                bucket: "b3".into(),
            },
            "a4" => Registration::Disabled("another".into()),
            _ => Registration::Unregistered,
        };
        let (runs, skipped) = queue(&rows, &lookup).unwrap();
        assert_eq!(runs.len() + skipped.len(), 4);
        assert_eq!(runs.len(), 2);
        assert_eq!(skipped.len(), 2);
        // No arm in both
        let run_arms: Vec<&str> = runs.iter().map(|r| r.arm.as_str()).collect();
        let skip_arms: Vec<&str> = skipped.iter().map(|s| s.arm.as_str()).collect();
        for r in &run_arms {
            assert!(!skip_arms.contains(r));
        }
    }

    // First unregistered arm aborts, nothing after it examined.
    #[test]
    fn first_unregistered_aborts_rest_not_examined() {
        use std::cell::RefCell;
        let examined = RefCell::new(Vec::new());
        let rows = vec![row("t1", "c1", &["a1", "a2", "a3"])];
        let lookup = |name: &str| {
            examined.borrow_mut().push(name.to_string());
            match name {
                "a1" => Registration::Enabled {
                    bucket: "b1".into(),
                },
                "a2" => Registration::Unregistered,
                "a3" => Registration::Enabled {
                    bucket: "b3".into(),
                },
                _ => Registration::Unregistered,
            }
        };
        queue(&rows, &lookup).unwrap_err();
        assert_eq!(examined.borrow().as_slice(), &["a1", "a2"]);
    }

    // Disabled reason is non-empty per spec, carried unchanged.
    #[test]
    fn disabled_reason_carried_unchanged() {
        let rows = vec![row("t1", "c1", &["a1", "a2"])];
        let lookup = |name: &str| match name {
            "a1" => Registration::Enabled {
                bucket: "b1".into(),
            },
            "a2" => Registration::Disabled("some reason".into()),
            _ => Registration::Unregistered,
        };
        let (_, skipped) = queue(&rows, &lookup).unwrap();
        assert_eq!(skipped[0].reason, "some reason");
    }

    // next_dispatchable returns index into queue, not a copy.
    #[test]
    fn next_dispatchable_returns_index_not_copy() {
        let queue = vec![run("a1", "t1", "c1", "b1"), run("a2", "t1", "c1", "b2")];
        let in_flight: BTreeMap<String, usize> = BTreeMap::new();
        let idx = next_dispatchable(&queue, &in_flight, 1).unwrap();
        assert_eq!(idx, 0);
        // The returned index can be used to mark dispatched
        assert_eq!(&queue[idx].arm, "a1");
    }
}
