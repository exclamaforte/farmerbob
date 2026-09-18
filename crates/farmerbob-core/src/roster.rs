//! Roster selection: joining availability, staleness, and the refusal ledger
//! into one dispatch decision.
//!
//! Every wave's arms used to be chosen by a human typing names into a
//! `.tsv`. The consequences are in the record: arms that refused on quota
//! were dispatched five waves running, five free arms sat eligible and
//! undispatched for 28.8 hours, and one wave was queued against a module
//! that did not exist yet. This module makes the join mechanical:
//! [`availability::availability`] says whether an arm CAN run,
//! [`availability::neglected`] says which dispatchable arms have not run
//! lately, and [`choose`] combines them into "dispatch these".
//!
//! This module decides ELIGIBILITY AND ORDER, not merit. `Because` is a
//! closed set of three and there is deliberately no scoring and no
//! preferred arm: ranking arms by past performance is a different decision
//! with its own evidence, and folding it in here would hide which of the
//! two produced a roster.

use std::collections::{BTreeMap, HashMap};

use crate::availability::{Arm, LastSeen, availability, dispatchable, neglected};

/// One arm's case for being in the next wave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The arm's id.
    pub name: String,
    /// Why it is being offered.
    pub because: Because,
}

/// The reason an arm is in the roster, in the order the enum declares,
/// which is also the priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Because {
    /// Dispatchable and never run. An arm with no record is the one the
    /// field knows least about.
    NeverRun,
    /// Dispatchable and not run within the staleness window.
    Neglected,
    /// Dispatchable and run recently. The ordinary case.
    Routine,
}

/// Why an arm is NOT in the roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Excluded {
    /// `availability` says it cannot be dispatched.
    NotAvailable,
    /// It refused its last `n` dispatches without producing anything.
    RefusingRecently,
    /// The roster was already full.
    RosterFull,
}

/// A roster decision: who, why, and who was left out and why.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roster {
    /// Chosen arms, in dispatch order.
    pub chosen: Vec<Candidate>,
    /// Everyone else, sorted by name, each with a reason.
    pub excluded: Vec<(String, Excluded)>,
}

/// Choose up to `size` arms.
///
/// `last_dispatch` and `stale_after_ms` have the same meaning as in
/// [`availability::neglected`], and the stale-versus-recent question is
/// delegated to that function so the two cannot disagree about the same
/// input: a name absent from `last_dispatch` is [`Because::NeverRun`], one
/// dispatched at least `stale_after_ms` ago is [`Because::Neglected`], and
/// any other dispatchable arm is [`Because::Routine`].
///
/// `recent_refusals` maps an arm to how many of its last dispatches
/// produced nothing; an arm at or above `refusal_cutoff` is excluded as
/// [`Excluded::RefusingRecently`], even when it is available and neglected.
/// **A `refusal_cutoff` of 0 turns the rule off entirely**: the `>=`
/// comparison is not applied at all, so nobody is excluded on refusal
/// grounds. Reading it the other way (`0 >= 0`) would report every arm as
/// refusing and empty every roster. The rule exists because an arm that
/// cannot run is a wasted slot, and the record shows five consecutive
/// waves spent on two such arms.
///
/// Exclusion reasons are checked in the order [`Excluded`] declares and the
/// FIRST match is reported: an arm that is unavailable AND refusing AND
/// would not have fit reports [`Excluded::NotAvailable`]. Arms that would
/// have qualified but did not fit are excluded as [`Excluded::RosterFull`],
/// never silently dropped, so every arm given appears exactly once across
/// [`Roster::chosen`] and [`Roster::excluded`].
///
/// Every degenerate input is a real answer, not an error: a `size` of 0
/// leaves [`Roster::chosen`] empty with everyone eligible excluded as full,
/// an empty `arms` slice yields [`Roster::default`], and a roster that
/// cannot be filled is reported as the empty roster it is.
pub fn choose(
    arms: &[Arm],
    last_dispatch: &[(&str, u64)],
    recent_refusals: &[(&str, u32)],
    now_ms: u64,
    stale_after_ms: u64,
    size: usize,
    refusal_cutoff: u32,
) -> Roster {
    // The staleness half of every `Because` comes from `neglected` itself,
    // so this module cannot disagree with it about any input: duplicate
    // dispatch rows, future timestamps, and the `stale_after_ms == 0`
    // boundary are all `neglected`'s own pinned rules, inherited verbatim.
    let stale = neglected(arms, last_dispatch, now_ms, stale_after_ms);
    let mut last_seen: HashMap<&str, LastSeen> = HashMap::with_capacity(stale.len());
    for entry in &stale {
        last_seen.insert(entry.name.as_str(), entry.last_seen);
    }

    // The same name twice in `recent_refusals`: the LAST entry wins, so a
    // later row corrects an earlier one.
    let mut refusals: HashMap<&str, u32> = HashMap::new();
    for &(name, count) in recent_refusals {
        refusals.insert(name, count);
    }

    // The same name twice in `arms`: the FIRST row wins, matching
    // `neglected`'s own deduplication.
    let mut unique: BTreeMap<&str, &Arm> = BTreeMap::new();
    for arm in arms {
        unique.entry(arm.name.as_str()).or_insert(arm);
    }

    let mut eligible: Vec<(Because, &str)> = Vec::new();
    let mut excluded: Vec<(String, Excluded)> = Vec::new();

    for (&name, &arm) in &unique {
        // Reasons are checked in `Excluded`'s declaration order and the
        // first match is reported.
        if !dispatchable(availability(arm, now_ms)) {
            excluded.push((arm.name.clone(), Excluded::NotAvailable));
            continue;
        }
        let refused = refusals.get(name).copied().unwrap_or(0);
        if refusal_cutoff >= 1 && refused >= refusal_cutoff {
            excluded.push((arm.name.clone(), Excluded::RefusingRecently));
            continue;
        }
        let because = match last_seen.get(name) {
            Some(LastSeen::Never) => Because::NeverRun,
            Some(LastSeen::Ago(_)) => Because::Neglected,
            None => Because::Routine,
        };
        eligible.push((because, arm.name.as_str()));
    }

    eligible.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    let chosen: Vec<Candidate> = eligible
        .iter()
        .take(size)
        .map(|&(because, name)| Candidate {
            name: name.to_string(),
            because,
        })
        .collect();
    for &(_, name) in eligible.iter().skip(size) {
        excluded.push((name.to_string(), Excluded::RosterFull));
    }
    excluded.sort_by(|a, b| a.0.cmp(&b.0));

    Roster { chosen, excluded }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(name: &str, status: &str, parked_until_ms: Option<u64>) -> Arm {
        Arm {
            name: name.to_string(),
            status: status.to_string(),
            parked_until_ms,
        }
    }

    fn ready(name: &str) -> Arm {
        arm(name, "verified", None)
    }

    fn chosen_pairs(roster: &Roster) -> Vec<(&str, Because)> {
        roster
            .chosen
            .iter()
            .map(|c| (c.name.as_str(), c.because))
            .collect()
    }

    // Clause 1: an arm `dispatchable` rejects is NotAvailable whatever else
    // is true of it. This one arm is also refusing AND, at size 0, cannot
    // fit: all three reasons match, and the first declared one is reported
    // (clause 4's triple match).
    #[test]
    fn clause1_unavailable_reports_not_available_even_when_refusing_and_overflowing() {
        let arms = [arm("a", "disabled", None)];
        let refusals = [("a", 9u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 0, 1);
        assert!(roster.chosen.is_empty());
        assert_eq!(
            roster.excluded,
            vec![("a".to_string(), Excluded::NotAvailable)]
        );
    }

    #[test]
    fn clause1_parked_and_unverified_arms_are_not_available() {
        let arms = [
            arm("parked", "verified", Some(2_000)),
            arm("unverified", "untested", None),
        ];
        let roster = choose(&arms, &[], &[], 1_000, 200, 5, 1);
        assert!(roster.chosen.is_empty());
        assert_eq!(
            roster.excluded,
            vec![
                ("parked".to_string(), Excluded::NotAvailable),
                ("unverified".to_string(), Excluded::NotAvailable),
            ]
        );
    }

    // Clause 1 composition: ParkExpired is dispatchable, so it must not be
    // reported NotAvailable -- it is eligible and, absent a dispatch record,
    // NeverRun.
    #[test]
    fn clause1_park_expired_arm_is_available_and_never_run() {
        let arms = [arm("expired", "verified", Some(500))];
        let roster = choose(&arms, &[], &[], 1_000, 200, 1, 1);
        assert_eq!(
            roster.chosen,
            vec![Candidate {
                name: "expired".to_string(),
                because: Because::NeverRun,
            }]
        );
        assert!(roster.excluded.is_empty());
    }

    // Clause 2: the refusal rule beats even the highest priority -- the arm
    // is available and never run, and is still excluded.
    #[test]
    fn clause2_refusing_arm_excluded_even_though_available_and_never_run() {
        let arms = [ready("a")];
        let refusals = [("a", 3u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 1);
        assert!(roster.chosen.is_empty());
        assert_eq!(
            roster.excluded,
            vec![("a".to_string(), Excluded::RefusingRecently)]
        );
    }

    // Clause 2 boundary: exactly at the cutoff is excluded (`>=`).
    #[test]
    fn clause2_boundary_refusal_count_exactly_at_cutoff_is_excluded() {
        let arms = [ready("a")];
        let refusals = [("a", 2u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 2);
        assert_eq!(
            roster.excluded,
            vec![("a".to_string(), Excluded::RefusingRecently)]
        );
    }

    // Clause 2 boundary: one below the cutoff is still eligible.
    #[test]
    fn clause2_boundary_refusal_count_one_below_cutoff_is_eligible() {
        let arms = [ready("a")];
        let refusals = [("a", 1u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 2);
        assert_eq!(roster.chosen.len(), 1);
        assert!(roster.excluded.is_empty());
    }

    // Clause 7: a name absent from `recent_refusals` counts as zero, and
    // zero is never at or above a cutoff of 1. The ghost entry in the
    // refusal list matches no arm and excludes nothing.
    #[test]
    fn clause7_arm_absent_from_refusals_counts_as_zero() {
        let arms = [ready("a")];
        let refusals = [("arm-not-present", 7u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 1);
        assert_eq!(roster.chosen.len(), 1);
        assert!(roster.excluded.is_empty());
    }

    // Clause 8: a cutoff of 0 means the rule is OFF, not that every arm is
    // refusing. Even u32::MAX refusals exclude nobody.
    #[test]
    fn clause8_cutoff_zero_turns_the_refusal_rule_off_entirely() {
        let arms = [ready("a")];
        let refusals = [("a", u32::MAX)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 0);
        assert_eq!(roster.chosen.len(), 1);
        assert!(roster.excluded.is_empty());
    }

    // Clause 3: every NeverRun precedes every Neglected precedes every
    // Routine, whatever the alphabetical order; within one Because, names
    // ascend.
    #[test]
    fn clause3_because_order_beats_name_order() {
        let arms = [ready("m-routine"), ready("a-neglected"), ready("z-never")];
        let dispatch = [("m-routine", 950), ("a-neglected", 700)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 200, 3, 1);
        assert_eq!(
            chosen_pairs(&roster),
            vec![
                ("z-never", Because::NeverRun),
                ("a-neglected", Because::Neglected),
                ("m-routine", Because::Routine),
            ]
        );
    }

    #[test]
    fn clause3_within_one_because_names_ascending() {
        let arms = [ready("beta"), ready("alpha")];
        let roster = choose(&arms, &[], &[], 1_000, 200, 2, 1);
        assert_eq!(
            chosen_pairs(&roster),
            vec![("alpha", Because::NeverRun), ("beta", Because::NeverRun)]
        );
    }

    // Clause 4: RefusingRecently is checked before RosterFull, so an arm
    // that is refusing AND would not have fit reports the refusal, not the
    // fullness.
    #[test]
    fn clause4_refusing_beats_roster_full_when_both_apply() {
        let arms = [ready("a"), ready("r")];
        let refusals = [("r", 1u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 1);
        assert_eq!(roster.chosen.len(), 1);
        assert_eq!(roster.chosen[0].name, "a");
        assert_eq!(
            roster.excluded,
            vec![("r".to_string(), Excluded::RefusingRecently)]
        );
    }

    // Clause 5: chosen never exceeds size, the overflow is RosterFull (not
    // dropped), and the counts conserve every arm.
    #[test]
    fn clause5_chosen_capped_at_size_and_overflow_is_roster_full() {
        let arms = [ready("a"), ready("b"), ready("c"), ready("d")];
        let roster = choose(&arms, &[], &[], 1_000, 200, 3, 1);
        assert_eq!(roster.chosen.len(), 3);
        assert_eq!(
            roster.excluded,
            vec![("d".to_string(), Excluded::RosterFull)]
        );
        assert_eq!(roster.chosen.len() + roster.excluded.len(), arms.len());
    }

    // Clause 5: which arm is cut follows priority, not a global name sort --
    // the routine arm loses its slot to higher-priority arms even though
    // "g" sorts before "n".
    #[test]
    fn clause5_overflow_follows_priority_not_name_order() {
        let arms = [ready("n"), ready("g"), ready("r")];
        let dispatch = [("g", 700), ("r", 990)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 200, 1, 1);
        assert_eq!(
            roster.chosen,
            vec![Candidate {
                name: "n".to_string(),
                because: Because::NeverRun,
            }]
        );
        assert_eq!(
            roster.excluded,
            vec![
                ("g".to_string(), Excluded::RosterFull),
                ("r".to_string(), Excluded::RosterFull),
            ]
        );
    }

    // Clause 6: absence from `last_dispatch` is NeverRun, not Neglected.
    // The window is wide enough that a collapsed Ago would read as Routine,
    // flipping the order -- so this distinguishes all three treatments.
    #[test]
    fn clause6_absent_from_last_dispatch_is_never_run_not_neglected() {
        let arms = [ready("b"), ready("a")];
        let dispatch = [("b", 900)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 2_000, 2, 1);
        assert_eq!(
            chosen_pairs(&roster),
            vec![("a", Because::NeverRun), ("b", Because::Routine)]
        );
    }

    // Boundary: `size` of 0 is not an error; every eligible arm is excluded
    // as RosterFull.
    #[test]
    fn boundary_size_zero_excludes_eligible_arms_as_roster_full() {
        let arms = [ready("a")];
        let roster = choose(&arms, &[], &[], 1_000, 200, 0, 1);
        assert!(roster.chosen.is_empty());
        assert_eq!(
            roster.excluded,
            vec![("a".to_string(), Excluded::RosterFull)]
        );
    }

    // Boundary: an EMPTY arms slice is `Roster::default()`, whatever the
    // other arguments say.
    #[test]
    fn boundary_empty_arms_is_the_default_roster() {
        assert_eq!(choose(&[], &[], &[], 1_000, 200, 0, 1), Roster::default());
        assert_eq!(
            choose(&[], &[("ghost", 5)], &[("ghost", 2)], 1_000, 200, 9, 1),
            Roster::default()
        );
    }

    // Boundary: `size` larger than the number of available arms -- everyone
    // available is chosen and nobody is excluded as RosterFull. The
    // unavailable arm still reports NotAvailable.
    #[test]
    fn boundary_size_beyond_available_leaves_no_roster_full() {
        let arms = [ready("a"), arm("d", "disabled", None)];
        let roster = choose(&arms, &[], &[], 1_000, 200, 50, 1);
        assert_eq!(roster.chosen.len(), 1);
        assert_eq!(roster.chosen[0].name, "a");
        assert_eq!(
            roster.excluded,
            vec![("d".to_string(), Excluded::NotAvailable)]
        );
    }

    // Boundary: every arm unavailable -- an empty chosen list and all
    // NotAvailable, reported as the real answer it is.
    #[test]
    fn boundary_all_unavailable_is_an_empty_roster_not_an_error() {
        let arms = [
            arm("d", "disabled", None),
            arm("p", "verified", Some(2_000)),
            arm("u", "whatever", None),
        ];
        let roster = choose(&arms, &[], &[], 1_000, 200, 5, 1);
        assert!(roster.chosen.is_empty());
        assert_eq!(
            roster.excluded,
            vec![
                ("d".to_string(), Excluded::NotAvailable),
                ("p".to_string(), Excluded::NotAvailable),
                ("u".to_string(), Excluded::NotAvailable),
            ]
        );
    }

    // Boundary: `stale_after_ms` of 0. Delegates `neglected`'s own pinned
    // boundary -- a dispatch at exactly now_ms is Ago(0) and stale -- so the
    // arm must come out Neglected, not Routine.
    #[test]
    fn boundary_stale_after_zero_makes_a_dispatch_at_now_neglected() {
        let arms = [ready("now-arm"), ready("fresh-arm")];
        let dispatch = [("now-arm", 1_000)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 0, 2, 1);
        assert_eq!(
            chosen_pairs(&roster),
            vec![
                ("fresh-arm", Because::NeverRun),
                ("now-arm", Because::Neglected),
            ]
        );
    }

    // Boundary: the staleness edge itself, at N and one past it -- and with
    // names chosen so a name sort would disagree with the Because sort.
    #[test]
    fn boundary_dispatch_exactly_at_the_stale_threshold_is_neglected() {
        let arms = [ready("edge"), ready("aaa")];
        let dispatch = [("edge", 800), ("aaa", 801)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 200, 2, 1);
        assert_eq!(
            chosen_pairs(&roster),
            vec![("edge", Because::Neglected), ("aaa", Because::Routine)]
        );
    }

    // Boundary: names in `last_dispatch` matching no arm are ignored; the
    // real arm is unaffected and no ghost appears anywhere.
    #[test]
    fn boundary_last_dispatch_names_matching_no_arm_are_ignored() {
        let arms = [ready("a")];
        let dispatch = [("ghost", 100), ("phantom", 1_000)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 200, 1, 1);
        assert_eq!(
            roster.chosen,
            vec![Candidate {
                name: "a".to_string(),
                because: Because::NeverRun,
            }]
        );
        assert!(roster.excluded.is_empty());
    }

    // The same name twice in `arms`: the FIRST row wins and the arm appears
    // once -- here the first row makes it available despite a disabled
    // duplicate.
    #[test]
    fn composition_duplicate_arm_rows_first_row_wins_when_first_is_available() {
        let arms = [ready("a"), arm("a", "disabled", None)];
        let roster = choose(&arms, &[], &[], 1_000, 200, 5, 1);
        assert_eq!(roster.chosen.len() + roster.excluded.len(), 1);
        assert_eq!(
            roster.chosen,
            vec![Candidate {
                name: "a".to_string(),
                because: Because::NeverRun,
            }]
        );
    }

    #[test]
    fn composition_duplicate_arm_rows_first_row_wins_when_first_is_unavailable() {
        let arms = [arm("b", "disabled", None), ready("b")];
        let roster = choose(&arms, &[], &[], 1_000, 200, 5, 1);
        assert_eq!(roster.chosen.len() + roster.excluded.len(), 1);
        assert_eq!(
            roster.excluded,
            vec![("b".to_string(), Excluded::NotAvailable)]
        );
    }

    // The same name twice in `last_dispatch`, in the order `neglected`'s own
    // test pins (chronological): the latest timestamp wins, so the arm
    // counts as recently run. `choose` inherits this from `neglected`
    // rather than re-deciding it.
    #[test]
    fn composition_duplicate_last_dispatch_rows_latest_timestamp_is_recent() {
        let arms = [ready("a")];
        let dispatch = [("a", 100), ("a", 950)];
        let roster = choose(&arms, &dispatch, &[], 1_000, 200, 1, 1);
        assert_eq!(
            roster.chosen,
            vec![Candidate {
                name: "a".to_string(),
                because: Because::Routine,
            }]
        );
    }

    // The same name twice in `recent_refusals`: the LAST entry wins, in
    // both directions.
    #[test]
    fn composition_duplicate_refusal_rows_last_entry_wins() {
        let arms = [ready("a"), ready("b")];
        let refusals = [("a", 5u32), ("a", 0u32), ("b", 0u32), ("b", 5u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 5, 1);
        assert_eq!(roster.chosen.len(), 1);
        assert_eq!(roster.chosen[0].name, "a");
        assert_eq!(
            roster.excluded,
            vec![("b".to_string(), Excluded::RefusingRecently)]
        );
    }

    // `excluded` is sorted by name, across all three reasons.
    #[test]
    fn composition_excluded_sorted_by_name_across_all_reasons() {
        let arms = [
            ready("m"),
            arm("z", "disabled", None),
            ready("a"),
            ready("n"),
        ];
        let refusals = [("a", 1u32)];
        let roster = choose(&arms, &[], &refusals, 1_000, 200, 1, 1);
        assert_eq!(roster.chosen.len(), 1);
        assert_eq!(roster.chosen[0].name, "m");
        assert_eq!(
            roster.excluded,
            vec![
                ("a".to_string(), Excluded::RefusingRecently),
                ("n".to_string(), Excluded::RosterFull),
                ("z".to_string(), Excluded::NotAvailable),
            ]
        );
    }

    // Composition: every arm given appears exactly once across chosen and
    // excluded, and the counts conserve.
    #[test]
    fn composition_every_arm_appears_exactly_once_across_chosen_and_excluded() {
        let arms = [
            ready("av"),
            ready("ov"),
            ready("ro"),
            ready("re"),
            arm("na", "disabled", None),
            arm("pk", "verified", Some(2_000)),
        ];
        let dispatch = [("ro", 990)];
        let refusals = [("re", 4u32)];
        let roster = choose(&arms, &dispatch, &refusals, 1_000, 200, 2, 1);
        assert_eq!(roster.chosen.len() + roster.excluded.len(), arms.len());
        let mut all_names: Vec<&str> = roster.chosen.iter().map(|c| c.name.as_str()).collect();
        all_names.extend(roster.excluded.iter().map(|(n, _)| n.as_str()));
        all_names.sort();
        all_names.dedup();
        assert_eq!(all_names.len(), arms.len());
    }
}
