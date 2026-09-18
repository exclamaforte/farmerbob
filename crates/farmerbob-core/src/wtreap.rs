//! Worktree disposal: what may be done with one worktree directory, decided
//! from positive evidence on both sides or not at all.
//!
//! The measurement that motivates the rule: on 2026-09-17 this machine held
//! 327 worktree directories of which git listed 125. The 203 git does not
//! know about have identical observable shape whether they are finished runs
//! nobody cleaned up or live runs whose registration was lost — and one of
//! the two is being written to right now. The only disposal rule ever
//! proposed before this module, "delete what git does not list", answers
//! that question by reading the second case as the first and destroying a
//! run in progress. It is the project's recurring bug wearing a new hat.
//!
//! The rule encoded here: deleting requires positive evidence on BOTH sides.
//!
//! 1. Evidence the run is over: a settled result exists for the directory's
//!    key — the run's evidence lives elsewhere, so the tree is copy, not
//!    source.
//! 2. Evidence nothing holds the directory: git does not list the path
//!    ([`Worktree::registered`]), and no live run holds the key.
//!
//! Absence of a record is not evidence on either side. Where the evidence
//! cannot decide — [`Disposal::Indeterminate`] — nothing is deleted, and the
//! disk keeps whatever we cannot prove is trash.
//!
//! Everything here is pure: no I/O, no `std::fs`, no git. The module
//! decides; the caller acts. A module that deletes cannot be tested by a
//! test that is wrong about what it deletes; a module that only classifies
//! can. This one only classifies.
//!
//! Key spelling: a directory is named `<task>--<arm>`, and both sides
//! routinely contain single hyphens, so the double hyphen is the only
//! separator there is. [`split_key`] extracts it; exact byte comparison is
//! used everywhere, as everywhere else in this crate — `Park-Scope--glm` is
//! not `park-scope--glm`.

/// What may be done with one worktree directory.
///
/// A closed set of four: the variants are exhaustive over the
/// `(registered, live, settled)` evidence cube, and every directory lands in
/// exactly one of them. Precedence is fixed — registration wins over
/// everything, then liveness, then settlement; what remains is
/// [`Disposal::Indeterminate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposal {
    /// git lists it. Not ours to delete at all — removing the directory
    /// under git's registration leaves a broken admin entry, which is
    /// strictly worse than the disk. Registration wins over every other
    /// signal, including a key that is simultaneously live and settled.
    Registered,
    /// Unregistered, no live run holds it, and a settled result exists for
    /// its key. The run finished and its evidence is elsewhere. Safe to
    /// delete.
    Reapable,
    /// Unregistered, but a live run holds this key. Being written to now.
    /// Liveness beats settlement: a re-run of a task that already has a
    /// result is the normal case, and deleting its worktree mid-run
    /// destroys the run.
    InUse,
    /// Unregistered, not live, and no settled result for its key. Never
    /// [`Disposal::Reapable`], deliberately: this is the only class whose
    /// two causes — a forgotten finished run and a lost record of a real
    /// one — produce the identical directory, and `Reapable` here would
    /// delete the second to save disk on the first. Indeterminate is a real
    /// answer, not a failure to answer.
    Indeterminate,
}

/// One worktree directory, as observed on disk and in git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Worktree<'a> {
    /// The directory's basename, expected to be `<task>--<arm>`. Expected,
    /// not guaranteed: a name that does not split has no key, and no key
    /// means no evidence can ever attach to it.
    pub dir: &'a str,
    /// Whether `git worktree list` names this path.
    pub registered: bool,
}

/// Split a worktree directory name into its task and arm on the FIRST `--`.
///
/// Returns `None` when the name carries no `--`, or when either side is
/// empty. Task names carry single hyphens and arm names carry single
/// hyphens, so only `--` separates them; a name that cannot be split cannot
/// be named, and a directory we cannot name is never deleted.
///
/// Splitting on the *first* `--` is a choice, and the other choice — the
/// last `--` — is defensible: it would let a task name itself contain `--`.
/// First is chosen because the arm is the leaf of the hierarchy and
/// arms are the side more likely to grow compound spellings, so the task
/// keeps whatever precedes the earliest separator. Pinned by test.
pub fn split_key(dir: &str) -> Option<(&str, &str)> {
    let (task, arm) = dir.split_once("--")?;
    if task.is_empty() || arm.is_empty() {
        return None;
    }
    Some((task, arm))
}

/// Classify one directory. `live` and `settled` are directory keys, in the
/// same `<task>--<arm>` spelling as [`Worktree::dir`]; comparison is exact.
///
/// Precedence, each step decisive:
///
/// 1. Registration wins over everything: a registered directory is
///    [`Disposal::Registered`] whatever `live` and `settled` say, with no
///    exceptions — not even a key that is simultaneously live and settled.
/// 2. An unregistered directory whose name does not split is
///    [`Disposal::Indeterminate`], never [`Disposal::Reapable`]: we do not
///    delete a directory we cannot name, and an unnamed directory is where
///    a lost registration would hide.
/// 3. A key in `live` is [`Disposal::InUse`], even when the same key is
///    also in `settled` — liveness beats settlement, because a re-run of a
///    settled task is ordinary and its worktree is being written to now.
/// 4. A key in `settled` alone is [`Disposal::Reapable`].
/// 5. Otherwise [`Disposal::Indeterminate`]: a key in `live` or `settled`
///    that matches no directory is simply ignored.
pub fn disposal(wt: &Worktree, live: &[&str], settled: &[&str]) -> Disposal {
    if wt.registered {
        return Disposal::Registered;
    }
    // The split gate is what makes the key matchable, not decoration: an
    // unnameable directory cannot be matched against `live` or `settled`
    // with any confidence, so it can never accumulate positive evidence and
    // falls through to Indeterminate even if its literal spelling turns up
    // in either list.
    if split_key(wt.dir).is_none() {
        return Disposal::Indeterminate;
    }
    if live.contains(&wt.dir) {
        return Disposal::InUse;
    }
    if settled.contains(&wt.dir) {
        return Disposal::Reapable;
    }
    Disposal::Indeterminate
}

/// Every directory that may be deleted, sorted, without duplicates.
///
/// Its membership is exactly the directories [`disposal`] calls
/// [`Disposal::Reapable`] — the two functions cannot disagree, and a test
/// pins that over a mixed listing. Order is byte order (plain `str`
/// ordering), NOT natural order: `t--arm10` sorts before `t--arm9` here,
/// whatever [`crate::autopilot::natural_less`] would say about the same
/// pair. A reader expecting natural order will be wrong about `9` vs `10`;
/// that expectation is the caller's to correct, not this function's to
/// guess at.
///
/// Duplicates: a name appearing twice in `wts` appears once here. This is a
/// set, and deliberately unlike [`census`], which counts entries.
pub fn reapable(wts: &[Worktree], live: &[&str], settled: &[&str]) -> Vec<String> {
    let mut dirs: Vec<String> = wts
        .iter()
        .filter(|wt| disposal(wt, live, settled) == Disposal::Reapable)
        .map(|wt| wt.dir.to_string())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// How many directories fell into each class.
///
/// Four counts and nothing else. [`Census::default`] is all zeros, and that
/// is the correct census of an empty listing, not a placeholder for one not
/// yet taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Census {
    /// Directories git lists.
    pub registered: u32,
    /// Directories safe to delete.
    pub reapable: u32,
    /// Directories a live run holds.
    pub in_use: u32,
    /// Directories the evidence cannot decide.
    pub indeterminate: u32,
}

/// Count the classes over a whole listing.
///
/// Counts entries, not distinct names: the same directory name appearing
/// twice in `wts` is counted twice, while [`reapable`] would list it once.
/// That asymmetry is deliberate — a census describes the listing as
/// observed, and the sum of the four counts is always `wts.len()`.
pub fn census(wts: &[Worktree], live: &[&str], settled: &[&str]) -> Census {
    let mut c = Census::default();
    for wt in wts {
        match disposal(wt, live, settled) {
            Disposal::Registered => c.registered += 1,
            Disposal::Reapable => c.reapable += 1,
            Disposal::InUse => c.in_use += 1,
            Disposal::Indeterminate => c.indeterminate += 1,
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unregistered `<task>--<arm>` directory.
    fn unreg(dir: &str) -> Worktree<'_> {
        Worktree {
            dir,
            registered: false,
        }
    }

    /// A registered `<task>--<arm>` directory.
    fn reg(dir: &str) -> Worktree<'_> {
        Worktree {
            dir,
            registered: true,
        }
    }

    // --- split_key ------------------------------------------------------------

    #[test]
    fn split_key_splits_task_from_arm_on_the_double_hyphen() {
        // Both sides carry single hyphens; only `--` separates them.
        assert_eq!(
            split_key("park-scope--glm-53-flash"),
            Some(("park-scope", "glm-53-flash"))
        );
        assert_eq!(
            split_key("wt-reap--glm-53-flash"),
            Some(("wt-reap", "glm-53-flash"))
        );
    }

    #[test]
    fn split_key_splits_on_the_first_double_hyphen() {
        // A deliberate choice, stated in the doc; the last `--` is the
        // defensible alternative. Everything after the first separator
        // belongs to the arm, separators and all.
        assert_eq!(split_key("a--b--c"), Some(("a", "b--c")));
        // The first occurrence of the two-character sequence, so a third
        // hyphen stays with the arm.
        assert_eq!(split_key("a---b"), Some(("a", "-b")));
    }

    #[test]
    fn split_key_rejects_names_without_a_separator() {
        assert_eq!(split_key("plain"), None);
        assert_eq!(split_key(""), None);
    }

    #[test]
    fn split_key_rejects_an_empty_side() {
        assert_eq!(split_key("--b"), None);
        assert_eq!(split_key("a--"), None);
        assert_eq!(split_key("--"), None);
    }

    // --- disposal: the four clauses -------------------------------------------

    #[test]
    fn registration_wins_over_every_other_signal_at_once() {
        // Clause 1 pinned where all three hold simultaneously: registered,
        // live, and settled. No exception, so the key's other memberships
        // are unreachable.
        let wt = reg("park-scope--glm-53-flash");
        let live = ["park-scope--glm-53-flash"];
        let settled = ["park-scope--glm-53-flash"];
        assert_eq!(disposal(&wt, &live, &settled), Disposal::Registered);
    }

    #[test]
    fn registration_wins_even_when_the_name_cannot_split() {
        // Clause 1 admits no exceptions, clause 8's second half: an
        // unnameable registered directory is still git's, not ours.
        let wt = reg("dangling-admin-entry");
        assert_eq!(disposal(&wt, &[], &[]), Disposal::Registered);
    }

    #[test]
    fn liveness_beats_settlement() {
        // Clause 2: a re-run of a task that already has a result is the
        // normal case, and its worktree is being written to now.
        let wt = unreg("park-scope--glm-53-flash");
        let live = ["park-scope--glm-53-flash"];
        let settled = ["park-scope--glm-53-flash"];
        assert_eq!(disposal(&wt, &live, &settled), Disposal::InUse);
    }

    #[test]
    fn settled_and_quiet_is_reapable() {
        // Clause 3: positive evidence on both sides — the run is over AND
        // nothing holds the tree.
        let wt = unreg("park-scope--glm-53-flash");
        let settled = ["park-scope--glm-53-flash"];
        assert_eq!(disposal(&wt, &[], &settled), Disposal::Reapable);
    }

    #[test]
    fn no_evidence_at_all_is_indeterminate_never_reapable() {
        // Clause 4: a forgotten finished run and a run whose record was
        // lost produce the identical directory. Absence is not evidence.
        let wt = unreg("park-scope--glm-53-flash");
        let verdict = disposal(&wt, &[], &[]);
        assert_eq!(verdict, Disposal::Indeterminate);
        assert_ne!(verdict, Disposal::Reapable);
    }

    #[test]
    fn an_unnameable_unregistered_directory_is_indeterminate_even_when_its_spelling_is_listed() {
        // Clause 8: we do not delete a directory we cannot name. A literal
        // string match against `live`/`settled` must not rescue it — the
        // split gate is the semantics, not an optimization.
        let wt = unreg("dangling-admin-entry");
        let live = ["dangling-admin-entry"];
        let settled = ["dangling-admin-entry"];
        assert_eq!(disposal(&wt, &live, &settled), Disposal::Indeterminate);
    }

    #[test]
    fn the_evidence_cube_is_exhaustive_over_the_four_answers() {
        // The (registered, live, settled) cube, all eight cells, against
        // the precedence the spec fixes. A fifth answer anywhere is a
        // defect; this is where one would show.
        let key = "t--a";
        for &registered in &[false, true] {
            for &is_live in &[false, true] {
                for &is_settled in &[false, true] {
                    let wt = Worktree {
                        dir: key,
                        registered,
                    };
                    let live: &[&str] = if is_live { &[key] } else { &[] };
                    let settled: &[&str] = if is_settled { &[key] } else { &[] };
                    let expected = if registered {
                        Disposal::Registered
                    } else if is_live {
                        Disposal::InUse
                    } else if is_settled {
                        Disposal::Reapable
                    } else {
                        Disposal::Indeterminate
                    };
                    assert_eq!(
                        disposal(&wt, live, settled),
                        expected,
                        "registered={registered} live={is_live} settled={is_settled}"
                    );
                }
            }
        }
    }

    #[test]
    fn key_comparison_is_exact_not_case_folded() {
        // As everywhere in this crate: `Park-Scope--glm` is not
        // `park-scope--glm`. A near-miss spelling must not manufacture the
        // positive evidence deletion needs.
        let wt = unreg("park-scope--glm");
        assert_eq!(
            disposal(&wt, &[], &["Park-Scope--glm"]),
            Disposal::Indeterminate
        );
        assert_eq!(
            disposal(&wt, &["PARK-SCOPE--GLM"], &[]),
            Disposal::Indeterminate
        );
    }

    #[test]
    fn keys_matching_no_directory_are_ignored_not_errors() {
        // `live` and `settled` describe other directories too; their
        // presence neither deletes nor protects anything here.
        let wt = unreg("park-scope--glm-53-flash");
        let live = ["other--arm", "ghost--arm"];
        let settled = ["finished--arm", "park-scope--other"];
        assert_eq!(disposal(&wt, &live, &settled), Disposal::Indeterminate);
    }

    // --- reapable ---------------------------------------------------------------

    #[test]
    fn reapable_and_disposal_cannot_disagree() {
        // Clause 10: run both over one mixed listing and compare. The
        // listing covers every class, including a duplicate reapable entry.
        let wts = [
            reg("task-a--arm1"),
            unreg("task-b--arm2"),
            unreg("task-c--arm3"),
            unreg("task-d--arm4"),
            unreg("task-b--arm2"),
            unreg("no-separator"),
        ];
        let live = ["task-b--arm2"];
        let settled = ["task-c--arm3", "task-d--arm4", "absent--arm"];
        let expected_reapable: Vec<String> = wts
            .iter()
            .filter(|wt| disposal(wt, &live, &settled) == Disposal::Reapable)
            .map(|wt| wt.dir.to_string())
            .collect();
        assert_eq!(reapable(&wts, &live, &settled), expected_reapable);
        // And the listing is what it was designed to be: all four classes
        // present, so the comparison above is not vacuous.
        let c = census(&wts, &live, &settled);
        assert!(c.registered > 0 && c.reapable > 0 && c.in_use > 0 && c.indeterminate > 0);
    }

    #[test]
    fn reapable_is_sorted_in_byte_order_not_natural_order() {
        // The doc fixes byte order; `10` before `9` is the visible
        // consequence, and exactly where a natural-order implementation
        // would differ.
        let wts = [unreg("t--arm9"), unreg("t--arm10"), unreg("t--arm2")];
        assert_eq!(
            reapable(&wts, &[], &["t--arm9", "t--arm10", "t--arm2"]),
            ["t--arm10", "t--arm2", "t--arm9"]
        );
    }

    #[test]
    fn reapable_is_deduplicated() {
        // A set, not a listing: the same reapable name twice in, once out.
        let wts = [unreg("a--b"), unreg("a--b"), unreg("c--d")];
        let settled = ["a--b", "c--d"];
        assert_eq!(reapable(&wts, &[], &settled), ["a--b", "c--d"]);
    }

    #[test]
    fn reapable_of_an_empty_listing_is_empty() {
        assert!(reapable(&[], &["x--y"], &["z--w"]).is_empty());
    }

    // --- census -----------------------------------------------------------------

    #[test]
    fn census_sums_to_the_listing_length_over_a_mixed_listing() {
        // Clause 9: every directory lands in exactly one class. The listing
        // exercises all four; keys matching no directory are ignored.
        let wts = [
            reg("task-a--arm1"),
            unreg("task-b--arm2"),
            unreg("task-c--arm3"),
            unreg("task-d--arm4"),
            unreg("unnamed"),
        ];
        let live = ["task-b--arm2", "absent--arm"];
        let settled = ["task-c--arm3"];
        let c = census(&wts, &live, &settled);
        assert_eq!(c.registered, 1);
        assert_eq!(c.reapable, 1);
        assert_eq!(c.in_use, 1);
        assert_eq!(c.indeterminate, 2);
        assert_eq!(
            (c.registered + c.reapable + c.in_use + c.indeterminate) as usize,
            wts.len()
        );
    }

    #[test]
    fn census_of_an_empty_listing_is_all_zeros() {
        // The zero listing, at the boundary: all zeros is the correct
        // census of it, and Default is that value.
        assert_eq!(census(&[], &[], &[]), Census::default());
        assert_eq!(Census::default().registered, 0);
        assert_eq!(Census::default().reapable, 0);
        assert_eq!(Census::default().in_use, 0);
        assert_eq!(Census::default().indeterminate, 0);
    }

    #[test]
    fn an_all_registered_listing_deletes_nothing() {
        let wts = [reg("a--b"), reg("c--d"), reg("no-separator")];
        let c = census(&wts, &[], &["a--b"]);
        assert_eq!(c.registered as usize, wts.len());
        assert_eq!(c.reapable, 0);
        // Even a settled key must not pry a registered directory loose.
        assert!(reapable(&wts, &[], &["a--b", "c--d"]).is_empty());
    }

    #[test]
    fn the_cold_start_case_deletes_nothing() {
        // Empty `live` and empty `settled` against a non-empty unregistered
        // listing: no positive evidence exists yet, so every entry is
        // Indeterminate and the reaper goes home empty. This is the case
        // that must hold on the first run, before any ledger exists.
        let wts = [unreg("a--b"), unreg("c--d"), unreg("unnamed")];
        let c = census(&wts, &[], &[]);
        assert_eq!(c.indeterminate as usize, wts.len());
        assert_eq!(c.reapable, 0);
        assert!(reapable(&wts, &[], &[]).is_empty());
    }

    #[test]
    fn a_duplicated_directory_is_counted_twice_but_listed_once() {
        // The census counts entries; the reaper returns a set. Both halves
        // pinned, because they differ deliberately.
        let wts = [unreg("a--b"), unreg("a--b")];
        let settled = ["a--b"];
        let c = census(&wts, &[], &settled);
        assert_eq!(c.reapable, 2);
        assert_eq!(reapable(&wts, &[], &settled), ["a--b"]);
        // The sum invariant survives duplication too.
        assert_eq!((c.reapable) as usize, wts.len());
    }
}
