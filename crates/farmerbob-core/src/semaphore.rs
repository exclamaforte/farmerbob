//! The slot semaphore's decision arithmetic.
//!
//! `fb-sem.sh` bounds concurrency by spinning over N lock files and taking
//! the first that `flock -n` accepts. Which slot is free, and what to do
//! when none is, is arithmetic over the states the caller observed, and this
//! module holds that arithmetic as pure functions. The caller holds the
//! locks; this decides.
//!
//! [`Slot::Unknown`] is the load-bearing state: a failed `flock -n` probe is
//! ambiguous between busy and broken, and a count that cannot tell "held"
//! from "could not read" is the kind of count this project's concurrency
//! bugs have come from. So an unreadable slot is never granted and never
//! reported as free capacity.

/// The state of one slot, as the caller found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Nothing holds it.
    Free,
    /// Something holds it.
    Held,
    /// The caller could not tell. NEVER treated as free.
    Unknown,
}

/// Which slot a caller should take, or why it may take none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Take this index. Always the LOWEST free index.
    Take(usize),
    /// Every slot is held; wait.
    Full,
    /// There are no slots at all.
    NoSlots,
}

/// Pick a slot from what the caller observed.
///
/// Grants the LOWEST index whose state is [`Slot::Free`]. A slot of unknown
/// state is never granted: not knowing whether a slot is taken is not
/// permission to take it. Returns [`Grant::Full`] when every slot is held or
/// unreadable, and [`Grant::NoSlots`] when there are no slots at all -- a
/// different fact, since `Full` means wait while `NoSlots` means the caller
/// asked for capacity that does not exist.
pub fn grant(slots: &[Slot]) -> Grant {
    if slots.is_empty() {
        return Grant::NoSlots;
    }
    match slots.iter().position(|slot| *slot == Slot::Free) {
        Some(index) => Grant::Take(index),
        None => Grant::Full,
    }
}

/// How many slots are free, and how many the caller could not read.
///
/// The second number is never folded into the first: an unreadable slot is
/// not capacity. So `free + unknown <= slots.len()`, with [`Slot::Held`]
/// making up the remainder -- every slot is counted as exactly one of the
/// three, and none is counted twice.
pub fn census(slots: &[Slot]) -> (usize, usize) {
    let mut free = 0;
    let mut unknown = 0;
    for slot in slots {
        match slot {
            Slot::Free => free += 1,
            Slot::Unknown => unknown += 1,
            Slot::Held => {}
        }
    }
    (free, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index of a `Take` decision, `None` otherwise, so assertions read
    /// as values instead of match arms.
    fn taken(decision: &Grant) -> Option<usize> {
        match decision {
            Grant::Take(index) => Some(*index),
            Grant::Full | Grant::NoSlots => None,
        }
    }

    /// Clause 1: with two free slots the grant is the LOWER one. "Any free
    /// slot" and "the lowest free slot" are different contracts; this pins
    /// the reproducible one.
    #[test]
    fn grant_takes_the_lowest_free_index() {
        assert_eq!(
            taken(&grant(&[Slot::Held, Slot::Free, Slot::Free])),
            Some(1)
        );
        assert_eq!(taken(&grant(&[Slot::Free, Slot::Free])), Some(0));
    }

    /// Clause 2: no free slot, but at least one held or unknown, means wait.
    #[test]
    fn grant_is_full_when_no_slot_is_free() {
        assert_eq!(grant(&[Slot::Held, Slot::Held]), Grant::Full);
        assert_eq!(grant(&[Slot::Held, Slot::Unknown, Slot::Held]), Grant::Full);
    }

    /// Clause 3: Unknown is never granted. A slice whose only non-Held
    /// entry is Unknown is still Full -- not knowing whether a slot is
    /// taken is not permission to take it.
    #[test]
    fn unknown_is_never_granted() {
        assert_eq!(grant(&[Slot::Held, Slot::Held, Slot::Unknown]), Grant::Full);
        assert_eq!(grant(&[Slot::Unknown]), Grant::Full);
        assert_eq!(grant(&[Slot::Unknown, Slot::Unknown]), Grant::Full);
    }

    /// Clause 4 plus the zero boundary: an EMPTY slice is NoSlots, a
    /// different fact from Full. `Full` means wait; `NoSlots` means the
    /// caller asked for capacity that does not exist.
    #[test]
    fn an_empty_slice_is_noslots_not_full() {
        assert_eq!(grant(&[]), Grant::NoSlots);
        assert_eq!(census(&[]), (0, 0));
    }

    /// Clause 5: one slice containing all three states. Free and Unknown
    /// are counted, Held makes up the remainder, nothing is counted twice.
    #[test]
    fn census_partitions_one_slice_of_all_three_states() {
        assert_eq!(census(&[Slot::Free, Slot::Held, Slot::Unknown]), (1, 1));
        assert_eq!(
            census(&[
                Slot::Free,
                Slot::Held,
                Slot::Held,
                Slot::Unknown,
                Slot::Free
            ]),
            (2, 1)
        );
    }

    /// Clause 6: Unknown is counted separately and never added to free. An
    /// all-Unknown slice reports zero free capacity, not n.
    #[test]
    fn census_counts_unknown_separately_never_as_free() {
        assert_eq!(census(&[Slot::Unknown; 4]), (0, 4));
    }

    /// Clauses 3 and 6 tested together, through the same slice: both are
    /// about Unknown, at the two ends of the API, and an implementation
    /// that treats Unknown as free in one and as held in the other passes
    /// each test alone while reporting a capacity it cannot honour.
    #[test]
    fn the_same_all_unknown_slice_reads_the_same_at_both_ends() {
        let slots = [Slot::Unknown; 3];
        assert_eq!(census(&slots), (0, 3));
        assert_eq!(grant(&slots), Grant::Full);
    }

    /// Clause 7: grant says Take exactly when census's free count is
    /// positive, over the empty, all-held, all-unknown and mixed slices --
    /// the two functions must never disagree about where capacity is.
    #[test]
    fn grant_and_census_agree_on_where_capacity_is() {
        let cases: &[&[Slot]] = &[
            &[],
            &[Slot::Held, Slot::Held],
            &[Slot::Unknown, Slot::Unknown],
            &[Slot::Unknown, Slot::Held, Slot::Unknown],
            &[Slot::Free, Slot::Held, Slot::Unknown],
            &[Slot::Held, Slot::Unknown, Slot::Free],
            &[Slot::Free, Slot::Free],
        ];
        for slots in cases {
            let decision = grant(slots);
            let free = census(slots).0;
            let agrees = match decision {
                Grant::Take(_) => free > 0,
                Grant::Full | Grant::NoSlots => free == 0,
            };
            assert!(
                agrees,
                "grant said {decision:?} but census counted {free} free in {slots:?}"
            );
        }
    }

    /// The one-slot boundaries, at Free, Held and Unknown.
    #[test]
    fn one_slot_boundaries() {
        assert_eq!(grant(&[Slot::Free]), Grant::Take(0));
        assert_eq!(census(&[Slot::Free]), (1, 0));
        assert_eq!(grant(&[Slot::Held]), Grant::Full);
        assert_eq!(census(&[Slot::Held]), (0, 0));
        assert_eq!(grant(&[Slot::Unknown]), Grant::Full);
        assert_eq!(census(&[Slot::Unknown]), (0, 1));
    }

    /// Every slot free: the grant is the first slot and census reports all
    /// of them, no unknowns.
    #[test]
    fn every_slot_free_grants_the_first_and_censuses_all() {
        let slots = [Slot::Free; 5];
        assert_eq!(grant(&slots), Grant::Take(0));
        assert_eq!(census(&slots), (5, 0));
    }

    /// The LAST slot free with all earlier ones held: the scan must reach
    /// the end of the slice, where an off-by-one would show.
    #[test]
    fn the_last_slot_free_is_found_by_the_scan() {
        let slots = [Slot::Held, Slot::Held, Slot::Held, Slot::Free];
        assert_eq!(grant(&slots), Grant::Take(3));
    }
}
