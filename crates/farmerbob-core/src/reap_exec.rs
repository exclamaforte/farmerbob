//! Execution-time re-check for worktree reap plans.
//!
//! [`crate::wtreap`] decides which worktree directories are disposable. [`crate::reap_plan`]
//! turns that into an ordered list of steps. Neither has ever been run directly
//! against disks in production, and the reason is that between building a plan
//! and executing it, a new agent run may start and claim a directory.
//!
//! Executing a stale plan would unregister or delete a live agent's working tree
//! mid-run. This module re-checks a plan against the world as it is NOW,
//! immediately before each step, and refuses the steps that no longer hold.
//! It performs no I/O, issues no commands, and modifies nothing on disk.

use std::collections::BTreeSet;

use crate::reap_plan::{Plan, Step};

/// Whether one step may still be performed, checked against current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Perform it.
    Go,
    /// Do not. The world changed since the plan was built.
    Stop {
        /// Which fact changed, for the log a caller writes.
        why: Stale,
    },
}

/// What invalidated a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// A live run now holds this directory. The strongest refusal.
    NowInUse,
    /// The plan says unregister, but git no longer registers it.
    NoLongerRegistered,
    /// The plan says delete an unregistered directory, but it is registered now.
    NowRegistered,
    /// The directory is gone already.
    Absent,
}

/// What a caller should actually do, in order, having re-checked everything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Admitted {
    /// Steps that still hold, in the plan's original order.
    pub go: Vec<Step>,
    /// Steps refused, each with the reason, in the plan's original order.
    pub stopped: Vec<(Step, Stale)>,
}

/// Evaluates admission for a single step against abstract presence predicates.
fn check_step(
    step: &Step,
    is_live: impl Fn(&str) -> bool,
    is_registered: impl Fn(&str) -> bool,
    is_present: impl Fn(&str) -> bool,
) -> Admit {
    match step {
        Step::Unregister { dir } => {
            let d = dir.as_str();
            if is_live(d) {
                Admit::Stop {
                    why: Stale::NowInUse,
                }
            } else if !is_registered(d) {
                Admit::Stop {
                    why: Stale::NoLongerRegistered,
                }
            } else {
                Admit::Go
            }
        }
        Step::Delete { dir } => {
            let d = dir.as_str();
            if is_live(d) {
                Admit::Stop {
                    why: Stale::NowInUse,
                }
            } else if is_registered(d) {
                Admit::Stop {
                    why: Stale::NowRegistered,
                }
            } else if !is_present(d) {
                Admit::Stop { why: Stale::Absent }
            } else {
                Admit::Go
            }
        }
    }
}

/// Re-check one step against the world.
///
/// `live` are directory keys with a running agent. `registered` are the
/// directories git currently lists. `present` are the directories that exist
/// on disk.
///
/// # Precedence Ladder
///
/// Precedence is strictly determined in the order declared by [`Stale`]:
///
/// For an [`Step::Unregister`] of `d`:
/// 1. `d` in `live` -> [`Stale::NowInUse`]
/// 2. `d` not in `registered` -> [`Stale::NoLongerRegistered`]
/// 3. otherwise -> [`Admit::Go`]
///
/// For a [`Step::Delete`] of `d`:
/// 1. `d` in `live` -> [`Stale::NowInUse`]
/// 2. `d` in `registered` -> [`Stale::NowRegistered`]
/// 3. `d` not in `present` -> [`Stale::Absent`]
/// 4. otherwise -> [`Admit::Go`]
///
/// Note on [`Step::Delete`]: a delete step for a registered directory yields
/// [`Stale::NowRegistered`] even though the plan may contain an earlier
/// [`Step::Unregister`] for it. `admit` inspects a single step in isolation and
/// cannot know whether the earlier step has run; sequencing and state accumulation
/// are handled by [`admit_plan`].
///
/// # Examples
///
/// ```
/// use farmerbob_core::reap_plan::Step;
/// use farmerbob_core::reap_exec::{admit, Admit, Stale};
///
/// let step = Step::Delete { dir: "run--1".to_string() };
/// assert_eq!(
///     admit(&step, &["run--1"], &[], &["run--1"]),
///     Admit::Stop { why: Stale::NowInUse }
/// );
/// ```
pub fn admit(step: &Step, live: &[&str], registered: &[&str], present: &[&str]) -> Admit {
    check_step(
        step,
        |d| live.contains(&d),
        |d| registered.contains(&d),
        |d| present.contains(&d),
    )
}

/// Re-check a whole plan.
///
/// **Only `plan.steps` is re-checked.** `plan.skipped` is carried by neither
/// return field and is not consulted: those directories were already refused
/// when the plan was built, and re-deriving that here would put the same
/// decision in two places.
///
/// `admit_plan` accumulates the effect of every step it admits:
/// - admitting an [`Step::Unregister`] of `d` removes `d` from the running set of registered directories;
/// - admitting a [`Step::Delete`] of `d` removes `d` from the running set of present directories;
/// - a refused step changes nothing.
///
/// As a consequence, a [`Step::Delete`] following an admitted [`Step::Unregister`] is judged
/// with `d` no longer registered, allowing normal unregister-then-delete sequences to proceed.
/// Repeated steps resolve naturally: a second [`Step::Unregister`] of `d` yields [`Stale::NoLongerRegistered`],
/// and a second [`Step::Delete`] of `d` yields [`Stale::Absent`].
///
/// # Guarantees
///
/// - Every step in `plan.steps` appears exactly once across [`Admitted::go`] and
///   [`Admitted::stopped`], preserving original relative order within each list.
///   Their lengths sum to `plan.steps.len()`.
/// - An empty plan yields an empty [`Admitted`], equal to [`Admitted::default()`].
/// - When all three input slices are empty against a non-empty plan, every [`Step::Unregister`]
///   is refused as [`Stale::NoLongerRegistered`] and every [`Step::Delete`] as [`Stale::Absent`].
/// - A plan whose every step is refused yields `go` empty and `stopped` carrying all of them.
///   This is a successful re-check, not a failure.
///
/// # Examples
///
/// ```
/// use farmerbob_core::reap_plan::{Plan, Step};
/// use farmerbob_core::reap_exec::{admit_plan, Admitted};
///
/// let plan = Plan {
///     steps: vec![
///         Step::Unregister { dir: "run--1".to_string() },
///         Step::Delete { dir: "run--1".to_string() },
///     ],
///     skipped: Vec::new(),
/// };
/// let admitted = admit_plan(&plan, &[], &["run--1"], &["run--1"]);
/// assert_eq!(admitted.go.len(), 2);
/// assert!(admitted.stopped.is_empty());
/// ```
pub fn admit_plan(plan: &Plan, live: &[&str], registered: &[&str], present: &[&str]) -> Admitted {
    let live_set: BTreeSet<&str> = live.iter().copied().collect();
    let mut reg_set: BTreeSet<&str> = registered.iter().copied().collect();
    let mut pres_set: BTreeSet<&str> = present.iter().copied().collect();

    let mut go = Vec::new();
    let mut stopped = Vec::new();

    for step in &plan.steps {
        let admission = check_step(
            step,
            |d| live_set.contains(&d),
            |d| reg_set.contains(&d),
            |d| pres_set.contains(&d),
        );

        match admission {
            Admit::Go => {
                match step {
                    Step::Unregister { dir } => {
                        let d: &str = dir.as_str();
                        reg_set.remove(&d);
                    }
                    Step::Delete { dir } => {
                        let d: &str = dir.as_str();
                        pres_set.remove(&d);
                    }
                }
                go.push(step.clone());
            }
            Admit::Stop { why } => {
                stopped.push((step.clone(), why));
            }
        }
    }

    Admitted { go, stopped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reap_plan::Skip;

    #[test]
    fn unregister_live_and_registered_yields_now_in_use() {
        let step = Step::Unregister {
            dir: "run--1".to_string(),
        };
        let admitted = admit(&step, &["run--1"], &["run--1"], &["run--1"]);
        assert_eq!(
            admitted,
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn unregister_live_and_unregistered_yields_now_in_use() {
        let step = Step::Unregister {
            dir: "run--1".to_string(),
        };
        let admitted = admit(&step, &["run--1"], &[], &["run--1"]);
        assert_eq!(
            admitted,
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn unregister_live_registered_and_absent_yields_now_in_use() {
        let step = Step::Unregister {
            dir: "run--1".to_string(),
        };
        let admitted = admit(&step, &["run--1"], &["run--1"], &[]);
        assert_eq!(
            admitted,
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn unregister_live_unregistered_and_absent_yields_now_in_use() {
        let step = Step::Unregister {
            dir: "run--1".to_string(),
        };
        let admitted = admit(&step, &["run--1"], &[], &[]);
        assert_eq!(
            admitted,
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn unregister_not_live_not_registered_yields_no_longer_registered() {
        let step = Step::Unregister {
            dir: "run--1".to_string(),
        };
        // present on disk, but not registered
        assert_eq!(
            admit(&step, &[], &[], &["run--1"]),
            Admit::Stop {
                why: Stale::NoLongerRegistered
            }
        );
        // absent from disk and not registered
        assert_eq!(
            admit(&step, &[], &[], &[]),
            Admit::Stop {
                why: Stale::NoLongerRegistered
            }
        );
    }

    #[test]
    fn unregister_ladder_fallthrough_to_go() {
        let step = Step::Unregister {
            dir: "run--1".to_string(),
        };
        // registered and present
        assert_eq!(admit(&step, &[], &["run--1"], &["run--1"]), Admit::Go);
        // registered and absent: unregister does not inspect present
        assert_eq!(admit(&step, &[], &["run--1"], &[]), Admit::Go);
    }

    #[test]
    fn delete_live_registered_present_yields_now_in_use() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &["run--1"], &["run--1"], &["run--1"]),
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn delete_live_registered_absent_yields_now_in_use() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &["run--1"], &["run--1"], &[]),
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn delete_live_unregistered_absent_yields_now_in_use() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &["run--1"], &[], &[]),
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn delete_live_unregistered_present_yields_now_in_use() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &["run--1"], &[], &["run--1"]),
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn delete_registered_and_absent_yields_now_registered() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        // Registered outranks absent in the Delete ladder
        assert_eq!(
            admit(&step, &[], &["run--1"], &[]),
            Admit::Stop {
                why: Stale::NowRegistered
            }
        );
    }

    #[test]
    fn delete_registered_and_present_yields_now_registered() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &[], &["run--1"], &["run--1"]),
            Admit::Stop {
                why: Stale::NowRegistered
            }
        );
    }

    #[test]
    fn delete_unregistered_and_absent_yields_absent() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &[], &[], &[]),
            Admit::Stop { why: Stale::Absent }
        );
    }

    #[test]
    fn delete_ladder_fallthrough_to_go() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(admit(&step, &[], &[], &["run--1"]), Admit::Go);
    }

    #[test]
    fn isolated_delete_on_registered_does_not_anticipate_earlier_unregister() {
        // admit sees one step and reports what it observes; sequence is handled in admit_plan
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        assert_eq!(
            admit(&step, &[], &["run--1"], &["run--1"]),
            Admit::Stop {
                why: Stale::NowRegistered
            }
        );
    }

    #[test]
    fn empty_plan_yields_default_admitted() {
        let plan = Plan::default();
        let admitted = admit_plan(&plan, &["live"], &["reg"], &["pres"]);
        assert_eq!(admitted, Admitted::default());
        assert!(admitted.go.is_empty());
        assert!(admitted.stopped.is_empty());
    }

    #[test]
    fn all_three_slices_empty_against_non_empty_plan() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--2".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &[], &[], &[]);
        assert!(admitted.go.is_empty());
        assert_eq!(
            admitted.stopped,
            vec![
                (
                    Step::Unregister {
                        dir: "run--1".to_string(),
                    },
                    Stale::NoLongerRegistered
                ),
                (
                    Step::Delete {
                        dir: "run--2".to_string(),
                    },
                    Stale::Absent
                ),
            ]
        );
    }

    #[test]
    fn plan_all_steps_refused_is_successful_recheck() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &["run--1"], &["run--1"], &["run--1"]);
        assert!(admitted.go.is_empty());
        assert_eq!(admitted.stopped.len(), 2);
        assert_eq!(admitted.stopped[0].1, Stale::NowInUse);
        assert_eq!(admitted.stopped[1].1, Stale::NowInUse);
    }

    #[test]
    fn admit_plan_accumulates_unregister_then_delete() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &[], &["run--1"], &["run--1"]);
        assert_eq!(admitted.go, plan.steps);
        assert!(admitted.stopped.is_empty());
    }

    #[test]
    fn admit_plan_repeated_unregister_stops_second() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &[], &["run--1"], &["run--1"]);
        assert_eq!(
            admitted.go,
            vec![Step::Unregister {
                dir: "run--1".to_string(),
            }]
        );
        assert_eq!(
            admitted.stopped,
            vec![(
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Stale::NoLongerRegistered
            )]
        );
    }

    #[test]
    fn admit_plan_repeated_delete_stops_second() {
        let plan = Plan {
            steps: vec![
                Step::Delete {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        // Unregistered but present on disk
        let admitted = admit_plan(&plan, &[], &[], &["run--1"]);
        assert_eq!(
            admitted.go,
            vec![Step::Delete {
                dir: "run--1".to_string(),
            }]
        );
        assert_eq!(
            admitted.stopped,
            vec![(
                Step::Delete {
                    dir: "run--1".to_string(),
                },
                Stale::Absent
            )]
        );
    }

    #[test]
    fn admit_plan_refused_unregister_does_not_mutate_registered() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        // run--1 is live: unregister is refused
        let admitted = admit_plan(&plan, &["run--1"], &["run--1"], &["run--1"]);
        assert!(admitted.go.is_empty());
        assert_eq!(admitted.stopped.len(), 2);
        assert_eq!(admitted.stopped[0].1, Stale::NowInUse);
        assert_eq!(admitted.stopped[1].1, Stale::NowInUse);
    }

    #[test]
    fn admit_plan_refused_delete_does_not_mutate_present() {
        let plan = Plan {
            steps: vec![
                Step::Delete {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        // run--1 is registered: first delete is refused as NowRegistered,
        // so run--1 is NOT removed from present. Second delete also sees NowRegistered.
        let admitted = admit_plan(&plan, &[], &["run--1"], &["run--1"]);
        assert!(admitted.go.is_empty());
        assert_eq!(
            admitted.stopped,
            vec![
                (
                    Step::Delete {
                        dir: "run--1".to_string(),
                    },
                    Stale::NowRegistered
                ),
                (
                    Step::Delete {
                        dir: "run--1".to_string(),
                    },
                    Stale::NowRegistered
                ),
            ]
        );
    }

    #[test]
    fn admit_plan_already_unregistered_delete_proceeds() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        // run--1 was already unregistered externally, but still present on disk
        let admitted = admit_plan(&plan, &[], &[], &["run--1"]);
        assert_eq!(
            admitted.go,
            vec![Step::Delete {
                dir: "run--1".to_string(),
            }]
        );
        assert_eq!(
            admitted.stopped,
            vec![(
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Stale::NoLongerRegistered
            )]
        );
    }

    #[test]
    fn admit_plan_partition_and_order_preservation() {
        let steps = vec![
            Step::Unregister {
                dir: "a".to_string(),
            },
            Step::Delete {
                dir: "a".to_string(),
            },
            Step::Unregister {
                dir: "b".to_string(),
            },
            Step::Delete {
                dir: "b".to_string(),
            },
            Step::Unregister {
                dir: "c".to_string(),
            },
            Step::Delete {
                dir: "c".to_string(),
            },
        ];
        let plan = Plan {
            steps: steps.clone(),
            skipped: Vec::new(),
        };
        // "a" is normal: registered and present -> both admitted
        // "b" is live: in live, registered, present -> both stopped (NowInUse)
        // "c" is already unregistered: not registered, present -> unregister stopped (NoLongerRegistered), delete admitted
        let admitted = admit_plan(&plan, &["b"], &["a", "b"], &["a", "b", "c"]);

        assert_eq!(
            admitted.go,
            vec![
                Step::Unregister {
                    dir: "a".to_string(),
                },
                Step::Delete {
                    dir: "a".to_string(),
                },
                Step::Delete {
                    dir: "c".to_string(),
                },
            ]
        );

        assert_eq!(
            admitted.stopped,
            vec![
                (
                    Step::Unregister {
                        dir: "b".to_string(),
                    },
                    Stale::NowInUse
                ),
                (
                    Step::Delete {
                        dir: "b".to_string(),
                    },
                    Stale::NowInUse
                ),
                (
                    Step::Unregister {
                        dir: "c".to_string(),
                    },
                    Stale::NoLongerRegistered
                ),
            ]
        );

        // Partition invariant: sum of lengths equals plan steps count
        assert_eq!(admitted.go.len() + admitted.stopped.len(), plan.steps.len());
    }

    #[test]
    fn admit_plan_ignores_plan_skipped() {
        let plan = Plan {
            steps: vec![Step::Delete {
                dir: "unreg".to_string(),
            }],
            skipped: vec![
                ("held".to_string(), Skip::InUse),
                ("conf".to_string(), Skip::Conflicted),
            ],
        };
        let admitted = admit_plan(&plan, &[], &[], &["unreg"]);
        assert_eq!(
            admitted.go,
            vec![Step::Delete {
                dir: "unreg".to_string(),
            }]
        );
        assert!(admitted.stopped.is_empty());
    }

    #[test]
    fn exact_string_matching_boundary() {
        let step = Step::Delete {
            dir: "run--1".to_string(),
        };
        // Different casing, spaces, trailing slash must not match
        assert_eq!(
            admit(&step, &["RUN--1"], &["run--1 "], &["run--1/"]),
            Admit::Stop { why: Stale::Absent }
        );
        // Prefix matching must not match
        assert_eq!(
            admit(&step, &["run--10"], &["run--1--extra"], &[]),
            Admit::Stop { why: Stale::Absent }
        );
    }

    #[test]
    fn extra_unmatched_names_in_slices_ignored() {
        let plan = Plan {
            steps: vec![Step::Delete {
                dir: "active".to_string(),
            }],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(
            &plan,
            &["extraneous-live-1", "extraneous-live-2"],
            &["extraneous-reg-1"],
            &["extraneous-pres-1", "active"],
        );
        assert_eq!(
            admitted.go,
            vec![Step::Delete {
                dir: "active".to_string(),
            }]
        );
        assert!(admitted.stopped.is_empty());
    }

    #[test]
    fn directory_in_all_three_slices_at_once_resolves_to_now_in_use() {
        let step_unreg = Step::Unregister {
            dir: "d".to_string(),
        };
        let step_del = Step::Delete {
            dir: "d".to_string(),
        };
        assert_eq!(
            admit(&step_unreg, &["d"], &["d"], &["d"]),
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
        assert_eq!(
            admit(&step_del, &["d"], &["d"], &["d"]),
            Admit::Stop {
                why: Stale::NowInUse
            }
        );
    }

    #[test]
    fn admit_plan_repeated_unregister_then_delete() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &[], &["run--1"], &["run--1"]);
        assert_eq!(
            admitted.go,
            vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ]
        );
        assert_eq!(
            admitted.stopped,
            vec![(
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Stale::NoLongerRegistered,
            )]
        );
        assert_eq!(admitted.go.len() + admitted.stopped.len(), plan.steps.len());
    }

    #[test]
    fn admit_plan_repeated_delete_after_unregister() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &[], &["run--1"], &["run--1"]);
        assert_eq!(
            admitted.go,
            vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ]
        );
        assert_eq!(
            admitted.stopped,
            vec![(
                Step::Delete {
                    dir: "run--1".to_string(),
                },
                Stale::Absent,
            )]
        );
        assert_eq!(admitted.go.len() + admitted.stopped.len(), plan.steps.len());
    }

    #[test]
    fn admit_plan_with_duplicate_entries_in_slices() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        // Duplicates in slices do not cause errors or prevent proper unregistration/deletion
        let admitted = admit_plan(
            &plan,
            &["live--1", "live--1"],
            &["run--1", "run--1"],
            &["run--1", "run--1"],
        );
        assert_eq!(admitted.go, plan.steps);
        assert!(admitted.stopped.is_empty());
    }

    #[test]
    fn admit_plan_preserves_multiple_directory_interleaved_order() {
        let plan = Plan {
            steps: vec![
                Step::Unregister {
                    dir: "run--1".to_string(),
                },
                Step::Delete {
                    dir: "run--1".to_string(),
                },
                Step::Unregister {
                    dir: "run--2".to_string(),
                },
                Step::Delete {
                    dir: "run--2".to_string(),
                },
            ],
            skipped: Vec::new(),
        };
        let admitted = admit_plan(&plan, &[], &["run--1", "run--2"], &["run--1", "run--2"]);
        assert_eq!(admitted.go, plan.steps);
        assert!(admitted.stopped.is_empty());
    }

    #[test]
    fn task_scope_is_clean() {
        use crate::scope::{Change, Declared, assess};

        let declared = Declared {
            target: "crates/farmerbob-core/src/reap_exec.rs".to_string(),
        };
        let changes = vec![
            Change {
                path: "crates/farmerbob-core/src/reap_exec.rs".to_string(),
                deleted: false,
            },
            Change {
                path: "crates/farmerbob-core/src/lib.rs".to_string(),
                deleted: false,
            },
        ];
        let sc = assess(&declared, &changes);
        assert!(sc.target_changed);
        assert_eq!(sc.allowed, vec!["crates/farmerbob-core/src/lib.rs"]);
        assert!(sc.departures.is_empty());
    }
}
