//! Ordered plan for worktree removal.
//!
//! [`wtreap`] decides which worktree directories may be deleted. It is careful:
//! an unregistered directory with no settled result is indeterminate, a listing
//! that contradicts itself is excluded, and [`wtreap::safe_to_reap`] is a strict
//! subset of [`wtreap::reapable`].
//!
//! The gap this module bridges is that "may be deleted" and "here is what to do"
//! are different values. A directory that git still registers needs its
//! registration removed BEFORE the directory goes, or the admin entry is left
//! dangling — which [`wtreap`]'s documentation calls strictly worse than the disk
//! it would free. An ordered plan is what a caller can execute; a set of names
//! is not.
//!
//! # Boundaries and Guarantees
//!
//! - An empty listing yields an empty [`Plan`], which equals [`Plan::default()`].
//! - A listing where nothing may be deleted yields a [`Plan`] with `steps` empty
//!   and `skipped` carrying every directory with a reason. This is a successful
//!   plan, not a failure.
//! - A listing where everything may be deleted and all are unregistered produces
//!   exactly one [`Step::Delete`] per directory, in sorted order, with `skipped`
//!   empty.
//! - The same directory name appearing twice with identical entries is one
//!   directory, and appears once in the plan.
//! - A directory whose name does not split on `--` is classified as indeterminate
//!   by [`wtreap`] when unregistered, so it is skipped as [`Skip::Indeterminate`].
//! - Every directory in the listing appears exactly once across `steps` and
//!   `skipped` — counted by directory name, where a registered name in `steps`
//!   carries two steps ([`Step::Unregister`] followed by [`Step::Delete`]).
//! - `steps` is in execution order: within a directory [`Step::Unregister`]
//!   precedes [`Step::Delete`], and directories are visited in sorted name order.
//!   Note that `steps` is not sorted as a whole — it is grouped by directory and
//!   ordered within, because sorting `steps` as a whole alphabetically would place
//!   `Delete` before `Unregister`, breaking execution ordering.
//! - `skipped` is sorted by directory name with one entry per directory.

use std::collections::{BTreeMap, BTreeSet};

use crate::wtreap::{self, Disposal, Worktree};

/// One step a caller may perform. Ordered within a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `git worktree remove` this path. Always precedes the [`Step::Delete`]
    /// of the same directory.
    Unregister {
        /// Directory basename.
        dir: String,
    },
    /// Remove this directory from disk.
    Delete {
        /// Directory basename.
        dir: String,
    },
}

/// What a caller should do, and what it must not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Steps in execution order.
    pub steps: Vec<Step>,
    /// Directories deliberately left alone, each with the reason, sorted by
    /// directory name. Present so a plan that does almost nothing can say why
    /// rather than looking like a plan that found nothing.
    pub skipped: Vec<(String, Skip)>,
}

/// Why a directory is not in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// A live run holds it.
    InUse,
    /// Unregistered, not live, and no settled result: the two causes are
    /// indistinguishable, so neither is acted on.
    Indeterminate,
    /// Observations disagree about whether git registers it.
    Conflicted,
    /// Registered with git, and this plan was asked not to unregister.
    RegisteredAndUnregisterNotPermitted,
}

/// Build a plan.
///
/// `may_unregister` gates whether the plan is allowed to touch git's
/// administrative records. When false, a registered directory is skipped
/// entirely rather than half-processed.
pub fn plan(wts: &[Worktree<'_>], live: &[&str], settled: &[&str], may_unregister: bool) -> Plan {
    if wts.is_empty() {
        return Plan::default();
    }

    // Listing-level contradictions defer to wtreap::conflicts.
    let conflicts: BTreeSet<String> = wtreap::conflicts(wts).into_iter().map(|c| c.dir).collect();

    // Group entries by directory name. BTreeMap sorts by directory name,
    // ensuring deterministic traversal in sorted name order.
    let mut dir_map: BTreeMap<&str, bool> = BTreeMap::new();
    for wt in wts {
        dir_map.entry(wt.dir).or_insert(wt.registered);
    }

    let mut steps = Vec::new();
    let mut skipped = Vec::new();

    for (&dir, &registered) in &dir_map {
        // Disagreeing registration observations poison the directory.
        if conflicts.contains(dir) {
            skipped.push((dir.to_string(), Skip::Conflicted));
            continue;
        }

        // Consult wtreap::disposal as if unregistered to evaluate key validity,
        // liveness, and settled evidence according to wtreap's established rules.
        let unreg_wt = Worktree {
            dir,
            registered: false,
        };
        let disp = wtreap::disposal(&unreg_wt, live, settled);

        // A live run holds it: skipped as InUse regardless of registration.
        if disp == Disposal::InUse {
            skipped.push((dir.to_string(), Skip::InUse));
            continue;
        }

        if registered {
            if !may_unregister {
                skipped.push((dir.to_string(), Skip::RegisteredAndUnregisterNotPermitted));
            } else {
                match disp {
                    Disposal::Reapable => {
                        steps.push(Step::Unregister {
                            dir: dir.to_string(),
                        });
                        steps.push(Step::Delete {
                            dir: dir.to_string(),
                        });
                    }
                    _ => {
                        skipped.push((dir.to_string(), Skip::Indeterminate));
                    }
                }
            }
        } else {
            match disp {
                Disposal::Reapable => {
                    steps.push(Step::Delete {
                        dir: dir.to_string(),
                    });
                }
                _ => {
                    skipped.push((dir.to_string(), Skip::Indeterminate));
                }
            }
        }
    }

    Plan { steps, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreg(dir: &str) -> Worktree<'_> {
        Worktree {
            dir,
            registered: false,
        }
    }

    fn reg(dir: &str) -> Worktree<'_> {
        Worktree {
            dir,
            registered: true,
        }
    }

    // Boundary: Empty listing yields default Plan.
    #[test]
    fn boundary_empty_listing_equals_default() {
        let p_false = plan(&[], &[], &[], false);
        let p_true = plan(&[], &[], &[], true);
        assert_eq!(p_false, Plan::default());
        assert_eq!(p_true, Plan::default());
        assert!(p_false.steps.is_empty());
        assert!(p_false.skipped.is_empty());
    }

    // Clause 1: For any directory that appears as a Delete, if that directory
    // is registered then an Unregister for the SAME directory appears EARLIER in steps.
    #[test]
    fn clause1_registered_directory_has_unregister_before_delete() {
        let wts = vec![reg("task--arm")];
        let p = plan(&wts, &[], &["task--arm"], true);
        assert_eq!(
            p.steps,
            vec![
                Step::Unregister {
                    dir: "task--arm".to_string()
                },
                Step::Delete {
                    dir: "task--arm".to_string()
                },
            ]
        );
        assert!(p.skipped.is_empty());
    }

    // Clause 1 property: over a mixed listing of registered and unregistered directories,
    // every Delete for a registered directory is preceded by Unregister for the same directory.
    #[test]
    fn clause1_mixed_listing_unregister_always_precedes_delete_for_registered() {
        let wts = vec![
            reg("c-task--arm1"),
            unreg("a-task--arm2"),
            reg("b-task--arm3"),
            unreg("d-task--arm4"),
        ];
        let settled = [
            "a-task--arm2",
            "b-task--arm3",
            "c-task--arm1",
            "d-task--arm4",
        ];
        let p = plan(&wts, &[], &settled, true);

        // All 4 directories are reapable.
        for wt in &wts {
            let delete_pos = p.steps.iter().position(|s| {
                *s == Step::Delete {
                    dir: wt.dir.to_string(),
                }
            });
            assert!(delete_pos.is_some(), "Delete step missing for {}", wt.dir);

            let unreg_pos = p.steps.iter().position(|s| {
                *s == Step::Unregister {
                    dir: wt.dir.to_string(),
                }
            });

            if wt.registered {
                assert!(
                    unreg_pos.is_some(),
                    "Unregister step missing for registered {}",
                    wt.dir
                );
                assert!(
                    unreg_pos.unwrap() < delete_pos.unwrap(),
                    "Unregister did not precede Delete for {}",
                    wt.dir
                );
            } else {
                assert!(
                    unreg_pos.is_none(),
                    "Unregister unexpectedly present for unregistered {}",
                    wt.dir
                );
            }
        }
    }

    // Clause 2: An unregistered, reapable directory yields a Delete and NO Unregister.
    #[test]
    fn clause2_unregistered_reapable_yields_delete_and_no_unregister() {
        let wts = vec![unreg("clean--arm")];
        let p = plan(&wts, &[], &["clean--arm"], true);
        assert_eq!(
            p.steps,
            vec![Step::Delete {
                dir: "clean--arm".to_string()
            }]
        );
        assert!(p.skipped.is_empty());

        // Same holding with may_unregister: false
        let p_no_unreg = plan(&wts, &[], &["clean--arm"], false);
        assert_eq!(
            p_no_unreg.steps,
            vec![Step::Delete {
                dir: "clean--arm".to_string()
            }]
        );
        assert!(p_no_unreg.skipped.is_empty());
    }

    // Clause 3: With may_unregister: false, a registered directory produces NO steps
    // and appears in skipped as RegisteredAndUnregisterNotPermitted. Not a Delete,
    // not a bare Unregister.
    #[test]
    fn clause3_registered_with_may_unregister_false_is_skipped() {
        let wts = vec![reg("park--glm")];
        let p = plan(&wts, &[], &["park--glm"], false);
        assert!(p.steps.is_empty());
        assert_eq!(
            p.skipped,
            vec![(
                "park--glm".to_string(),
                Skip::RegisteredAndUnregisterNotPermitted
            )]
        );
    }

    #[test]
    fn clause3_mixed_listing_with_may_unregister_false() {
        let wts = vec![reg("registered--arm"), unreg("unregistered--arm")];
        let settled = ["registered--arm", "unregistered--arm"];
        let p = plan(&wts, &[], &settled, false);

        assert_eq!(
            p.steps,
            vec![Step::Delete {
                dir: "unregistered--arm".to_string()
            }]
        );
        assert_eq!(
            p.skipped,
            vec![(
                "registered--arm".to_string(),
                Skip::RegisteredAndUnregisterNotPermitted
            )]
        );
    }

    // Clause 4: Every directory in the listing appears EXACTLY ONCE across
    // steps and skipped -- counted by directory name, where a name in steps
    // may carry two steps. Pin the partition.
    #[test]
    fn clause4_partition_property_across_diverse_cases() {
        let wts = vec![
            reg("dir-a--arm"),       // will be settled, registered (2 steps if may_unregister)
            unreg("dir-b--arm"),     // will be settled, unregistered (1 step)
            reg("dir-c--arm"),       // will be live (skipped InUse)
            unreg("dir-d--arm"),     // will be indeterminate (no settled)
            reg("dir-e--arm"),       // will be conflicted
            unreg("dir-e--arm"),     // will be conflicted
            unreg("unnameable_dir"), // invalid split key (skipped Indeterminate)
        ];
        let live = ["dir-c--arm"];
        let settled = ["dir-a--arm", "dir-b--arm", "dir-e--arm"];

        for &may_unreg in &[false, true] {
            let p = plan(&wts, &live, &settled, may_unreg);

            let all_unique_dirs: BTreeSet<String> = wts.iter().map(|w| w.dir.to_string()).collect();

            // Distinct directory names in steps
            let mut step_dirs = BTreeSet::new();
            for s in &p.steps {
                match s {
                    Step::Unregister { dir } => {
                        step_dirs.insert(dir.clone());
                    }
                    Step::Delete { dir } => {
                        step_dirs.insert(dir.clone());
                    }
                }
            }

            // Distinct directory names in skipped
            let mut skip_dirs = BTreeSet::new();
            for (dir, _) in &p.skipped {
                assert!(
                    skip_dirs.insert(dir.clone()),
                    "Duplicate directory in skipped: {dir}"
                );
            }

            // Partition check: step_dirs and skip_dirs are disjoint
            for d in &step_dirs {
                assert!(
                    !skip_dirs.contains(d),
                    "Directory {d} appears in BOTH steps and skipped"
                );
            }

            // Partition check: union equals all_unique_dirs
            let mut union_dirs = BTreeSet::new();
            for d in &step_dirs {
                union_dirs.insert(d.clone());
            }
            for d in &skip_dirs {
                union_dirs.insert(d.clone());
            }
            assert_eq!(union_dirs, all_unique_dirs);

            // Exact count: step_dirs.len() + skip_dirs.len() == unique directories count
            assert_eq!(step_dirs.len() + skip_dirs.len(), all_unique_dirs.len());
        }
    }

    // Clause 5: A conflicted directory -- observed both registered and unregistered --
    // is skipped as Conflicted and never appears in steps, whatever may_unregister says.
    // This defers to wtreap::conflicts and must not re-derive the rule.
    #[test]
    fn clause5_conflicted_directory_skipped_as_conflicted_regardless_of_may_unregister() {
        let wts = vec![reg("task--arm"), unreg("task--arm")];
        let settled = ["task--arm"];

        for &may_unreg in &[false, true] {
            let p = plan(&wts, &[], &settled, may_unreg);
            assert!(
                p.steps.is_empty(),
                "Conflicted directory produced steps when may_unregister={may_unreg}"
            );
            assert_eq!(p.skipped, vec![("task--arm".to_string(), Skip::Conflicted)]);
        }
    }

    #[test]
    fn clause5_conflicted_not_resolved_by_majority() {
        // 2 registered, 1 unregistered: not resolved by 2-1 vote
        let wts = vec![reg("task--arm"), reg("task--arm"), unreg("task--arm")];
        let p = plan(&wts, &[], &["task--arm"], true);
        assert!(p.steps.is_empty());
        assert_eq!(p.skipped, vec![("task--arm".to_string(), Skip::Conflicted)]);

        // 1 registered, 2 unregistered: not resolved by 2-1 vote
        let wts_rev = vec![reg("task--arm"), unreg("task--arm"), unreg("task--arm")];
        let p_rev = plan(&wts_rev, &[], &["task--arm"], true);
        assert!(p_rev.steps.is_empty());
        assert_eq!(
            p_rev.skipped,
            vec![("task--arm".to_string(), Skip::Conflicted)]
        );
    }

    #[test]
    fn clause5_conflicted_trumps_liveness_and_settlement() {
        let wts = vec![reg("task--arm"), unreg("task--arm")];
        let live = ["task--arm"];
        let settled = ["task--arm"];
        let p = plan(&wts, &live, &settled, true);
        assert_eq!(p.skipped, vec![("task--arm".to_string(), Skip::Conflicted)]);
    }

    // Clause 6: A directory a live run holds is skipped as InUse.
    #[test]
    fn clause6_directory_held_by_live_run_skipped_as_in_use() {
        let wts_unreg = vec![unreg("live--arm")];
        let p_unreg = plan(&wts_unreg, &["live--arm"], &["live--arm"], true);
        assert!(p_unreg.steps.is_empty());
        assert_eq!(
            p_unreg.skipped,
            vec![("live--arm".to_string(), Skip::InUse)]
        );

        let wts_reg = vec![reg("live--arm")];
        let p_reg_true = plan(&wts_reg, &["live--arm"], &["live--arm"], true);
        assert!(p_reg_true.steps.is_empty());
        assert_eq!(
            p_reg_true.skipped,
            vec![("live--arm".to_string(), Skip::InUse)]
        );

        let p_reg_false = plan(&wts_reg, &["live--arm"], &["live--arm"], false);
        assert!(p_reg_false.steps.is_empty());
        assert_eq!(
            p_reg_false.skipped,
            vec![("live--arm".to_string(), Skip::InUse)]
        );
    }

    #[test]
    fn clause6_liveness_beats_settlement() {
        let wts = vec![unreg("re-run--arm")];
        let p = plan(&wts, &["re-run--arm"], &["re-run--arm"], true);
        assert!(p.steps.is_empty());
        assert_eq!(p.skipped, vec![("re-run--arm".to_string(), Skip::InUse)]);
    }

    // Clause 7: The plan never contains a Delete for a directory wtreap::safe_to_reap
    // would not return.
    // Pin this property over unregistered listings and when may_unregister is false.
    #[test]
    fn clause7_unregistered_listing_deletes_are_subset_of_safe_to_reap() {
        let wts = vec![
            unreg("safe-a--1"),
            unreg("safe-b--2"),
            unreg("live-c--3"),
            unreg("indeterminate-d--4"),
            unreg("unnameable"),
        ];
        let live = ["live-c--3"];
        let settled = ["safe-a--1", "safe-b--2", "unnameable"];

        let safe = wtreap::safe_to_reap(&wts, &live, &settled);
        let p = plan(&wts, &live, &settled, true);

        for step in &p.steps {
            if let Step::Delete { dir } = step {
                assert!(
                    safe.contains(dir),
                    "Directory {dir} deleted in plan but not in wtreap::safe_to_reap"
                );
            }
        }
    }

    #[test]
    fn clause7_may_unregister_false_deletes_are_subset_of_safe_to_reap() {
        let wts = vec![
            reg("reg-a--1"),
            unreg("unreg-b--2"),
            unreg("live-c--3"),
            reg("live-d--4"),
        ];
        let live = ["live-c--3", "live-d--4"];
        let settled = ["reg-a--1", "unreg-b--2"];

        let safe = wtreap::safe_to_reap(&wts, &live, &settled);
        let p = plan(&wts, &live, &settled, false);

        for step in &p.steps {
            if let Step::Delete { dir } = step {
                assert!(
                    safe.contains(dir),
                    "Directory {dir} deleted in plan but not in wtreap::safe_to_reap"
                );
            }
        }
    }

    // Boundary: A listing where NOTHING may be deleted.
    #[test]
    fn boundary_nothing_may_be_deleted_is_successful_plan_with_empty_steps() {
        let wts = vec![
            unreg("task-a--arm1"), // not settled -> Indeterminate
            unreg("task-b--arm2"), // live -> InUse
            reg("task-c--arm3"),   // registered with may_unregister=false -> RegisteredNotPermitted
        ];
        let live = ["task-b--arm2"];
        let settled = ["task-c--arm3"];
        let p = plan(&wts, &live, &settled, false);

        assert!(
            p.steps.is_empty(),
            "Steps must be empty when nothing is deletable"
        );
        assert_eq!(
            p.skipped,
            vec![
                ("task-a--arm1".to_string(), Skip::Indeterminate),
                ("task-b--arm2".to_string(), Skip::InUse),
                (
                    "task-c--arm3".to_string(),
                    Skip::RegisteredAndUnregisterNotPermitted
                ),
            ]
        );
    }

    // Boundary: A listing where EVERYTHING may be deleted and all are unregistered.
    #[test]
    fn boundary_everything_deletable_unregistered_sorted_delete_empty_skipped() {
        let wts = vec![
            unreg("z-task--arm"),
            unreg("m-task--arm"),
            unreg("a-task--arm"),
        ];
        let settled = ["z-task--arm", "m-task--arm", "a-task--arm"];
        let p = plan(&wts, &[], &settled, true);

        assert!(p.skipped.is_empty());
        assert_eq!(
            p.steps,
            vec![
                Step::Delete {
                    dir: "a-task--arm".to_string()
                },
                Step::Delete {
                    dir: "m-task--arm".to_string()
                },
                Step::Delete {
                    dir: "z-task--arm".to_string()
                },
            ]
        );
    }

    // Boundary: The SAME directory name appearing twice with identical entries.
    #[test]
    fn boundary_same_directory_name_with_identical_entries_appears_once() {
        let wts = vec![unreg("task--arm"), unreg("task--arm"), unreg("task--arm")];
        let settled = ["task--arm"];
        let p = plan(&wts, &[], &settled, true);

        assert!(p.skipped.is_empty());
        assert_eq!(
            p.steps,
            vec![Step::Delete {
                dir: "task--arm".to_string()
            }]
        );

        let wts_reg = vec![reg("task--arm"), reg("task--arm")];
        let p_reg = plan(&wts_reg, &[], &settled, true);
        assert!(p_reg.skipped.is_empty());
        assert_eq!(
            p_reg.steps,
            vec![
                Step::Unregister {
                    dir: "task--arm".to_string()
                },
                Step::Delete {
                    dir: "task--arm".to_string()
                },
            ]
        );
    }

    // Boundary: A directory whose name does not split on `--`.
    #[test]
    fn boundary_name_without_double_hyphen_skipped_as_indeterminate() {
        let wts = vec![unreg("single-hyphen"), unreg("no_hyphens"), unreg("")];
        let settled = ["single-hyphen", "no_hyphens", ""];
        let p = plan(&wts, &[], &settled, true);

        assert!(p.steps.is_empty());
        assert_eq!(
            p.skipped,
            vec![
                ("".to_string(), Skip::Indeterminate),
                ("no_hyphens".to_string(), Skip::Indeterminate),
                ("single-hyphen".to_string(), Skip::Indeterminate),
            ]
        );
    }

    #[test]
    fn boundary_registered_name_without_double_hyphen_behavior() {
        let wts = vec![reg("no-double-hyphen")];
        let settled = ["no-double-hyphen"];

        // With may_unregister: true, unnameable directory cannot be deleted -> Indeterminate
        let p_true = plan(&wts, &[], &settled, true);
        assert!(p_true.steps.is_empty());
        assert_eq!(
            p_true.skipped,
            vec![("no-double-hyphen".to_string(), Skip::Indeterminate)]
        );

        // With may_unregister: false, registered directory is skipped as RegisteredAndUnregisterNotPermitted
        let p_false = plan(&wts, &[], &settled, false);
        assert!(p_false.steps.is_empty());
        assert_eq!(
            p_false.skipped,
            vec![(
                "no-double-hyphen".to_string(),
                Skip::RegisteredAndUnregisterNotPermitted
            )]
        );
    }

    // Registered directory without settled result.
    #[test]
    fn registered_unsettled_directory_behavior() {
        let wts = vec![reg("task--arm")];

        // When may_unregister is true, without settled result it cannot be deleted -> Indeterminate
        let p_true = plan(&wts, &[], &[], true);
        assert!(p_true.steps.is_empty());
        assert_eq!(
            p_true.skipped,
            vec![("task--arm".to_string(), Skip::Indeterminate)]
        );

        // When may_unregister is false -> RegisteredAndUnregisterNotPermitted
        let p_false = plan(&wts, &[], &[], false);
        assert!(p_false.steps.is_empty());
        assert_eq!(
            p_false.skipped,
            vec![(
                "task--arm".to_string(),
                Skip::RegisteredAndUnregisterNotPermitted
            )]
        );
    }

    // Composition: steps execution order is grouped by directory and ordered within,
    // not sorted as a whole.
    #[test]
    fn composition_steps_execution_order_not_sorted_as_a_whole() {
        let wts = vec![reg("b--arm"), reg("a--arm")];
        let settled = ["a--arm", "b--arm"];
        let p = plan(&wts, &[], &settled, true);

        // Expected order:
        // Directory "a--arm": Unregister, then Delete
        // Directory "b--arm": Unregister, then Delete
        assert_eq!(
            p.steps,
            vec![
                Step::Unregister {
                    dir: "a--arm".to_string()
                },
                Step::Delete {
                    dir: "a--arm".to_string()
                },
                Step::Unregister {
                    dir: "b--arm".to_string()
                },
                Step::Delete {
                    dir: "b--arm".to_string()
                },
            ]
        );

        // A reader sorting steps as a whole would put all Deletes before Unregisters ('D' < 'U')
        // or compare enum variants, which breaks execution order. Verify steps is NOT in that order.
        let mut sorted_as_a_whole = p.steps.clone();
        sorted_as_a_whole.sort_by(|a, b| match (a, b) {
            (Step::Delete { .. }, Step::Unregister { .. }) => std::cmp::Ordering::Less,
            (Step::Unregister { .. }, Step::Delete { .. }) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
        assert_ne!(
            p.steps, sorted_as_a_whole,
            "steps must not be sorted with Deletes ahead of Unregisters"
        );
    }

    // Composition: skipped is sorted by directory name with one entry per directory.
    #[test]
    fn composition_skipped_is_sorted_by_directory_name() {
        let wts = vec![
            unreg("z--arm"),
            unreg("a--arm"),
            unreg("m--arm"),
            unreg("z--arm"), // duplicate
        ];
        let p = plan(&wts, &[], &[], true);

        let dirs: Vec<&str> = p.skipped.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(dirs, vec!["a--arm", "m--arm", "z--arm"]);
    }

    // Determinism: calling plan twice over the same input produces identical plans.
    #[test]
    fn determinism_two_runs_produce_identical_plans() {
        let wts = vec![
            reg("alpha--1"),
            unreg("beta--2"),
            reg("gamma--3"),
            unreg("delta--4"),
        ];
        let live = ["gamma--3"];
        let settled = ["alpha--1", "beta--2"];

        let p1 = plan(&wts, &live, &settled, true);
        let p2 = plan(&wts, &live, &settled, true);
        assert_eq!(p1, p2);
    }

    // Unrelated entries in live or settled are ignored.
    #[test]
    fn unrelated_live_or_settled_keys_ignored() {
        let wts = vec![unreg("present--arm")];
        let live = ["ghost-live--arm"];
        let settled = ["present--arm", "ghost-settled--arm"];

        let p = plan(&wts, &live, &settled, true);
        assert_eq!(
            p.steps,
            vec![Step::Delete {
                dir: "present--arm".to_string()
            }]
        );
        assert!(p.skipped.is_empty());
    }
}


// ESCALATED: 1 confirmed finding(s) by critic gemini-38-flash, found on glm-53-flash.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_reap_plan_glm_53_flash {
    use super::*;

    #[test]
    fn claim_1() {
        // "A registered directory held by a live run is scheduled for unregistration and deletion instead of being skipped as InUse when may_unregister is true."
        let wts = [Worktree {
            dir: "t--a",
            registered: true,
        }];

        assert_eq!(
            plan(&wts, &["t--a"], &[], true),
            Plan {
                steps: vec![],
                skipped: vec![("t--a".to_string(), Skip::InUse)],
            }
        );
    }
}
