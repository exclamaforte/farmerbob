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
//!
//! Listing-level contradictions: everything above classifies ONE entry. A
//! listing can also disagree with itself — the same `dir` observed once with
//! `registered: true` and once with `registered: false` — and no per-entry
//! answer can see that, because [`disposal`] is never shown more than one
//! entry at a time. The contradiction matters precisely because the two
//! halves are not symmetric: only the unregistered half can ever be called
//! [`Disposal::Reapable`], so a self-contradicting listing composes into a
//! `reapable` name that git lists — a directory unsafe to delete.
//! [`conflicts`] reports the disagreement, and [`safe_to_reap`] is the view
//! a deleter must use. The doctrine is this module's own: absence of
//! agreement is not evidence, whether the absence is in a missing record or
//! in the input itself.

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

/// A directory name observed more than once with disagreeing registration.
///
/// This is deliberately NOT a fifth [`Disposal`] variant. A conflict is a
/// property of the LISTING, not of a directory's state: [`disposal`] sees
/// one [`Worktree`] and can only report what that single entry's evidence
/// supports, while the disagreement is visible only across several entries.
/// Making it a variant would force `disposal` to report something no entry
/// it was shown could justify, so it is reported here instead, beside the
/// other aggregate views.
///
/// Both counts are non-zero by construction: a name is reported only when
/// at least one entry carried each flag — that is what "disagree" means —
/// so neither side of the disagreement can total zero. Their sum is the
/// number of entries carrying the name, because every entry is counted into
/// exactly one of the two buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The directory name that was observed inconsistently.
    pub dir: String,
    /// How many entries said git lists it.
    pub registered: u32,
    /// How many said it does not.
    pub unregistered: u32,
}

/// Every directory whose entries disagree about registration, sorted by
/// `dir`, without duplicates.
///
/// One observation cannot disagree with itself, so a name appearing once is
/// never reported. Identical duplicates — same name, same flag — are
/// agreement, not conflict, and keep the treatment they were already
/// pinned to have: counted twice by [`census`], listed once by [`reapable`].
///
/// A conflict is never resolved by majority, however lopsided the vote.
/// Two observations agreeing is not evidence that the third was wrong —
/// the agreeing pair could be the stale copies and the lone dissenter the
/// only observation taken after git's state actually changed — and deleting
/// a possibly-registered directory on a 2-1 vote is exactly the outcome
/// this module exists to prevent. Uncertainty in the input lands in the
/// same place as uncertainty in a missing record: nothing is deleted.
///
/// Nameability and registration-agreement are independent. A name that
/// does not split on `--` is reported here exactly like any other: a
/// conflict in an unnameable directory is still a conflict, whatever
/// [`disposal`] would go on to decide about that directory's disposal.
pub fn conflicts(wts: &[Worktree<'_>]) -> Vec<Conflict> {
    let mut entries: Vec<(&str, bool)> = wts.iter().map(|wt| (wt.dir, wt.registered)).collect();
    entries.sort_unstable();
    let mut out: Vec<Conflict> = Vec::new();
    let mut idx = 0;
    while idx < entries.len() {
        let dir = entries[idx].0;
        let mut registered = 0;
        let mut unregistered = 0;
        while idx < entries.len() && entries[idx].0 == dir {
            if entries[idx].1 {
                registered += 1;
            } else {
                unregistered += 1;
            }
            idx += 1;
        }
        if registered > 0 && unregistered > 0 {
            out.push(Conflict {
                dir: dir.to_string(),
                registered,
                unregistered,
            });
        }
    }
    out
}

/// [`reapable`], with contradictory listings excluded.
///
/// This is what a caller that DELETES should use. [`reapable`] is kept
/// unchanged because it is the honest answer to a different question —
/// "what did [`disposal`] say" — and it cannot see a disagreement it is
/// never told about: it classifies entries, and the contradiction lives
/// between them. The two functions differ exactly on the conflicts: a name
/// in `reapable` but not here is precisely a name whose entries disagree
/// about registration. Replacing one with the other would make one of the
/// two questions unanswerable.
///
/// The result is sorted, deduplicated, and always a subset of [`reapable`].
/// An empty result is a real answer: it means nothing in this listing is
/// safe to delete, which is a different statement from "nothing is
/// reapable" — the listing may be full of reapable-looking entries that a
/// single contradiction poisons.
pub fn safe_to_reap(wts: &[Worktree<'_>], live: &[&str], settled: &[&str]) -> Vec<String> {
    let poisoned: Vec<String> = conflicts(wts).into_iter().map(|c| c.dir).collect();
    reapable(wts, live, settled)
        .into_iter()
        .filter(|dir| !poisoned.contains(dir))
        .collect()
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

    // --- conflicts & safe_to_reap: the listing contradicts itself -------------

    #[test]
    fn a_name_observed_with_both_flags_is_a_conflict_counted_on_each_side() {
        // Clause 1: the same dir twice, once registered and once not, with
        // the disagreement counted per side.
        let wts = [unreg("a--b"), reg("a--b")];
        assert_eq!(
            conflicts(&wts),
            [Conflict {
                dir: "a--b".to_string(),
                registered: 1,
                unregistered: 1,
            }]
        );
    }

    #[test]
    fn a_conflicted_name_is_never_safe_to_reap_whatever_the_evidence_says() {
        // Clause 2: the exclusion holds over the whole (live, settled) cube
        // for the poisoned name, not just one convenient cell of it. The
        // listing carries only this name, so the whole result must be empty
        // in every cell.
        let key = "a--b";
        let wts = [unreg(key), reg(key)];
        for &is_live in &[false, true] {
            for &is_settled in &[false, true] {
                let live: &[&str] = if is_live { &[key] } else { &[] };
                let settled: &[&str] = if is_settled { &[key] } else { &[] };
                assert!(
                    safe_to_reap(&wts, live, settled).is_empty(),
                    "live={is_live} settled={is_settled}"
                );
            }
        }
    }

    #[test]
    fn identical_duplicates_are_not_conflicts_and_keep_their_old_pinning() {
        // Clause 3: same flag twice is agreement, not conflict — in both
        // directions — and the pre-existing pinning of identical duplicates
        // survives: counted twice by census, listed once by reapable.
        let twice_unreg = [unreg("a--b"), unreg("a--b")];
        assert!(conflicts(&twice_unreg).is_empty());
        let twice_reg = [reg("a--b"), reg("a--b")];
        assert!(conflicts(&twice_reg).is_empty());
        let settled = ["a--b"];
        assert_eq!(census(&twice_unreg, &[], &settled).reapable, 2);
        assert_eq!(reapable(&twice_unreg, &[], &settled), ["a--b"]);
        // With no conflicts the deletion-facing view IS the raw view.
        assert_eq!(
            safe_to_reap(&twice_unreg, &[], &settled),
            reapable(&twice_unreg, &[], &settled)
        );
    }

    #[test]
    fn reapable_keeps_saying_what_disposal_said_even_where_it_is_unsafe() {
        // Clause 4: `reapable` is unchanged by this task. On a
        // contradictory listing it still contains the contested name,
        // because one of its entries genuinely is Reapable, and
        // `safe_to_reap` is the deletion-facing view that drops exactly
        // that name. Both functions exist because they answer different
        // questions; this is the case where the answers differ.
        let wts = [
            unreg("contested--arm"),
            reg("contested--arm"),
            unreg("quiet--arm"),
        ];
        let settled = ["contested--arm", "quiet--arm"];
        assert_eq!(
            reapable(&wts, &[], &settled),
            ["contested--arm", "quiet--arm"]
        );
        assert_eq!(safe_to_reap(&wts, &[], &settled), ["quiet--arm"]);
    }

    #[test]
    fn safe_to_reap_is_always_a_subset_of_reapable() {
        // Clause 5, pinned as a property over a mixed listing rather than
        // prose: every name the safe view returns must also be in the raw
        // view — the safe view can only ever remove names.
        let wts = [
            unreg("contested--a"),
            reg("contested--a"),
            unreg("clean--b"),
            reg("clean--c"),
            unreg("live--d"),
            unreg("mystery--e"),
            unreg("no-separator"),
        ];
        let live = ["live--d"];
        let settled = [
            "contested--a",
            "clean--b",
            "clean--c",
            "live--d",
            "mystery--e",
        ];
        let raw = reapable(&wts, &live, &settled);
        let safe = safe_to_reap(&wts, &live, &settled);
        for name in &safe {
            assert!(
                raw.contains(name),
                "{name} in safe_to_reap but not reapable"
            );
        }
        // Built so the property is not vacuous: the raw view is non-empty
        // and the safe view removed the one contested name from it.
        assert_eq!(raw, ["clean--b", "contested--a", "mystery--e"]);
        assert_eq!(safe, ["clean--b", "mystery--e"]);
    }

    #[test]
    fn a_two_to_one_majority_does_not_resolve_a_conflict() {
        // Clause 6: two observations agreeing is not evidence that the
        // third was wrong — the agreeing pair could be the stale copies —
        // and deleting a possibly-registered directory on a 2-1 vote is
        // the outcome this module exists to prevent. Pinned in both
        // directions so "majority unregistered" gets no free pass either.
        let wts = [reg("a--b"), reg("a--b"), unreg("a--b")];
        assert_eq!(
            conflicts(&wts),
            [Conflict {
                dir: "a--b".to_string(),
                registered: 2,
                unregistered: 1,
            }]
        );
        let settled = ["a--b"];
        // The lone dissenter is the only entry disposal would ever call
        // Reapable here, and its disagreement with the others is exactly
        // why it must not be acted on.
        assert_eq!(reapable(&wts, &[], &settled), ["a--b"]);
        assert!(safe_to_reap(&wts, &[], &settled).is_empty());

        let mirrored = [reg("c--d"), unreg("c--d"), unreg("c--d")];
        assert_eq!(
            conflicts(&mirrored),
            [Conflict {
                dir: "c--d".to_string(),
                registered: 1,
                unregistered: 2,
            }]
        );
        let settled = ["c--d"];
        assert_eq!(reapable(&mirrored, &[], &settled), ["c--d"]);
        assert!(safe_to_reap(&mirrored, &[], &settled).is_empty());
    }

    #[test]
    fn a_conflict_in_an_unnameable_dir_is_still_a_conflict() {
        // Clause 7: nameability and registration-agreement are
        // independent. `disposal` refuses to delete an unnameable
        // directory regardless, but `conflicts` reports the disagreement
        // in the listing exactly as it would for a nameable one.
        let wts = [unreg("dangling-admin-entry"), reg("dangling-admin-entry")];
        assert_eq!(
            conflicts(&wts),
            [Conflict {
                dir: "dangling-admin-entry".to_string(),
                registered: 1,
                unregistered: 1,
            }]
        );
        // It was never reapable, so the exclusion here changes nothing —
        // but the poisoned name must flow through the filter without being
        // crashed on or resurrected.
        assert!(safe_to_reap(&wts, &[], &["dangling-admin-entry"]).is_empty());
    }

    #[test]
    fn an_empty_listing_has_no_conflicts_and_nothing_safe_to_reap() {
        assert!(conflicts(&[]).is_empty());
        assert!(safe_to_reap(&[], &["x--y"], &["z--w"]).is_empty());
    }

    #[test]
    fn a_conflict_free_listing_leaves_the_two_views_equal() {
        // Boundary: with nothing contradictory, `safe_to_reap` equals
        // `reapable` exactly — the pin is the equality, not merely the
        // subset relation.
        let wts = [
            unreg("clean--b"),
            unreg("clean--b"), // duplicate, same flag: agreement
            reg("clean--c"),
            unreg("live--d"),
            unreg("mystery--e"),
        ];
        let live = ["live--d"];
        let settled = ["clean--b", "clean--c"];
        assert!(conflicts(&wts).is_empty());
        assert_eq!(
            safe_to_reap(&wts, &live, &settled),
            reapable(&wts, &live, &settled)
        );
        assert_eq!(safe_to_reap(&wts, &live, &settled), ["clean--b"]);
    }

    #[test]
    fn a_listing_where_every_dir_conflicts_reaps_nothing() {
        // Boundary: every distinct name is poisoned, so the safe view is
        // empty while `conflicts` carries one entry per distinct name —
        // not one per conflicting entry.
        let wts = [
            unreg("a--one"),
            reg("a--one"),
            unreg("b--two"),
            reg("b--two"),
            reg("c--three"),
            unreg("c--three"),
        ];
        let settled = ["a--one", "b--two", "c--three"];
        assert!(safe_to_reap(&wts, &[], &settled).is_empty());
        let cs = conflicts(&wts);
        assert_eq!(cs.len(), 3);
        for c in &cs {
            assert_eq!(c.registered, 1);
            assert_eq!(c.unregistered, 1);
        }
    }

    #[test]
    fn a_name_observed_once_cannot_conflict_with_itself() {
        // Boundary: one observation has nothing to disagree with, whichever
        // flag it carries.
        let wts = [unreg("a--b"), reg("c--d")];
        assert!(conflicts(&wts).is_empty());
    }

    #[test]
    fn conflict_counts_sum_to_the_entries_carrying_the_name() {
        // The counting contract: each side non-zero, sides summing to the
        // number of entries carrying the name. Every entry lands in exactly
        // one bucket, and a name is only reported when both buckets are
        // non-empty — that is what "disagree" means — so neither side can
        // be zero by construction.
        let wts = [
            reg("a--b"),
            unreg("a--b"),
            reg("a--b"),
            reg("a--b"), // 3 registered, 1 unregistered
            reg("c--d"),
            reg("c--d"),
            unreg("c--d"),
            unreg("c--d"), // 2 and 2
        ];
        let cs = conflicts(&wts);
        assert_eq!(cs.len(), 2);
        for c in &cs {
            assert!(c.registered > 0 && c.unregistered > 0);
            let carrying = wts.iter().filter(|wt| wt.dir == c.dir).count();
            assert_eq!((c.registered + c.unregistered) as usize, carrying);
        }
        // Sorted by dir, so the index order below is the pinned order.
        assert_eq!(cs[0].registered, 3);
        assert_eq!(cs[0].unregistered, 1);
        assert_eq!(cs[1].registered, 2);
        assert_eq!(cs[1].unregistered, 2);
    }

    #[test]
    fn conflicts_are_sorted_and_one_per_distinct_name() {
        // Byte order, like everywhere else in this module: the reporting
        // order is the name's own sort, not the order the disagreement was
        // observed in, and a thrice-observed name yields one entry.
        let wts = [
            unreg("z--last"),
            reg("z--last"),
            unreg("a--first"),
            reg("a--first"),
            unreg("m--mid"),
            reg("m--mid"),
            unreg("m--mid"),
        ];
        let cs = conflicts(&wts);
        let names: Vec<&str> = cs.iter().map(|c| c.dir.as_str()).collect();
        assert_eq!(names, ["a--first", "m--mid", "z--last"]);
        assert_eq!(cs[1].registered, 1);
        assert_eq!(cs[1].unregistered, 2);
    }

    #[test]
    fn safe_to_reap_is_sorted_and_deduplicated() {
        // Inherited from `reapable` and pinned here so a reimplementation
        // of the safe view cannot quietly lose the set semantics. One
        // poisoned name (observed three ways) is removed; the rest survive
        // in byte order.
        let wts = [
            unreg("t--arm9"),
            unreg("t--arm10"),
            unreg("t--arm2"),
            unreg("t--arm2"),
            reg("t--arm2"),
        ];
        let settled = ["t--arm9", "t--arm10", "t--arm2"];
        assert_eq!(safe_to_reap(&wts, &[], &settled), ["t--arm10", "t--arm9"]);
    }
}
