//! Worktree allocation: which run may hold a tree, and when a tree may be
//! reclaimed.
//!
//! Each run gets its own git worktree. Allocation looks trivial and is not:
//! this project destroyed eight of ten runs in one wave because two
//! dispatchers each believed they owned the same tree, and one removed it
//! while the other's agent was still writing. The rules here are therefore
//! about ownership, not paths:
//!
//! - A tree is owned by exactly one run at a time. A second allocation can
//!   never quietly displace an owner; only the separate, named
//!   [`Pool::force_reclaim`] can remove a live owner.
//! - One run, one tree: a run that already holds a slot cannot take another.
//! - A finished run's claim stays allocated (the tree still exists on disk)
//!   but becomes reclaimable, and the pool can nominate the best candidate:
//!   the oldest finished claim, never a live one.
//! - Disk is finite. A full pool answers [`AllocError::PoolExhausted`] and
//!   leaves the decision to the caller; it never evicts on its own.
//!
//! Pure logic: no filesystem, no git. The caller performs the operations
//! this decides on.

use std::collections::HashMap;

/// A run's claim on a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The run that holds the tree.
    pub run_id: String,
    /// Relative slot name, e.g. "task--arm". Unique per live claim.
    pub slot: String,
    /// When the claim was taken, in milliseconds since the epoch.
    pub since_ms: u64,
    /// False once the run has finished, whatever its verdict.
    pub live: bool,
}

/// Why an allocation or release was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError {
    /// Another run holds this slot and is still live.
    HeldByLiveRun {
        /// The contested slot.
        slot: String,
        /// The run that holds it.
        holder: String,
    },
    /// The pool is at capacity and nothing is reclaimable.
    PoolExhausted {
        /// The capacity that was reached.
        capacity: usize,
    },
    /// This run already holds a different slot.
    AlreadyHolding {
        /// The run asking for a second tree.
        run_id: String,
        /// The slot the run already holds.
        slot: String,
    },
}

/// The worktree pool: every current claim, keyed by slot.
///
/// Invariants: a slot appears at most once; a run holds at most one slot;
/// a live claim's slot is never taken except through
/// [`Pool::force_reclaim`]. A `capacity` of 0 means unbounded, so
/// `Pool::default()` is an unbounded pool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pool {
    capacity: usize,
    claims: HashMap<String, Claim>,
}

impl Pool {
    /// Create a new pool. A `capacity` of 0 means unbounded.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            claims: HashMap::new(),
        }
    }

    /// Claim a slot for a run.
    ///
    /// Checks run from the caller's own state to the environment's, so every
    /// outcome is total and named:
    ///
    /// 1. The same run re-claiming its own slot succeeds as a no-op, so a
    ///    retry is safe even when the pool is full. A retry never revives a
    ///    claim the run has already finished.
    /// 2. A run already holding a different slot is
    ///    [`AllocError::AlreadyHolding`] -- one run, one tree, whatever the
    ///    state of the slot it asked for.
    /// 3. A slot held live by another run is [`AllocError::HeldByLiveRun`],
    ///    naming the holder. It is never silently displaced.
    /// 4. A slot held by a finished run transfers to the new run, replacing
    ///    the old claim wholesale (new `since_ms`, live again).
    /// 5. A fresh slot when the pool is full is
    ///    [`AllocError::PoolExhausted`], even when other slots are
    ///    reclaimable: this module decides, it does not evict on its own.
    ///    The caller reclaims through [`reclaim_candidate`] and
    ///    [`Pool::release`] or [`Pool::force_reclaim`], then retries.
    pub fn allocate(&mut self, run_id: &str, slot: &str, now_ms: u64) -> Result<(), AllocError> {
        let mut transfers = false;
        if let Some(claim) = self.claims.get(slot) {
            if claim.run_id == run_id {
                return Ok(());
            }
            if claim.live {
                return Err(AllocError::HeldByLiveRun {
                    slot: slot.to_string(),
                    holder: claim.run_id.clone(),
                });
            }
            // Held by a finished run: ownership transfers below.
            transfers = true;
        }

        if let Some(held) = self.held_slot(run_id) {
            return Err(AllocError::AlreadyHolding {
                run_id: run_id.to_string(),
                slot: held.to_string(),
            });
        }

        if !transfers && self.is_full() {
            return Err(AllocError::PoolExhausted {
                capacity: self.capacity,
            });
        }

        self.claims.insert(
            slot.to_string(),
            Claim {
                run_id: run_id.to_string(),
                slot: slot.to_string(),
                since_ms: now_ms,
                live: true,
            },
        );
        Ok(())
    }

    /// Mark a run finished. Its slot stays allocated but becomes reclaimable.
    ///
    /// Returns true if the run holds a claim, live or already finished, and
    /// false if the run is unknown. Idempotent: the second call changes
    /// nothing.
    pub fn finish(&mut self, run_id: &str) -> bool {
        let mut known = false;
        for claim in self.claims.values_mut() {
            if claim.run_id == run_id {
                claim.live = false;
                known = true;
            }
        }
        known
    }

    /// Release a slot held by a FINISHED run. Refuses a live holder -- use
    /// `force_reclaim`. Releasing a slot nobody holds succeeds silently,
    /// since releasing nothing is not an error.
    pub fn release(&mut self, slot: &str) -> Result<(), AllocError> {
        let live_holder = match self.claims.get(slot) {
            Some(claim) if claim.live => Some(claim.run_id.clone()),
            _ => None,
        };
        if let Some(holder) = live_holder {
            return Err(AllocError::HeldByLiveRun {
                slot: slot.to_string(),
                holder,
            });
        }
        let _ = self.claims.remove(slot);
        Ok(())
    }

    /// Release regardless of liveness. Separate and named so that destroying
    /// a live run's work is always a deliberate act.
    ///
    /// Returns true if a claim was removed, false if the slot was free.
    pub fn force_reclaim(&mut self, slot: &str) -> bool {
        self.claims.remove(slot).is_some()
    }

    /// Which run holds a slot, if any.
    pub fn holder(&self, slot: &str) -> Option<&Claim> {
        self.claims.get(slot)
    }

    /// How many claims are still live.
    pub fn live_count(&self) -> usize {
        self.claims.values().filter(|claim| claim.live).count()
    }

    /// How many slots are allocated, live and finished together.
    pub fn len(&self) -> usize {
        self.claims.len()
    }

    /// Whether no slot is allocated at all.
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    /// The slot this run currently holds, if any. The pool's invariant keeps
    /// it to at most one.
    fn held_slot(&self, run_id: &str) -> Option<&str> {
        self.claims
            .values()
            .find(|claim| claim.run_id == run_id)
            .map(|claim| claim.slot.as_str())
    }

    /// Whether a fresh claim would exceed the declared capacity. A capacity
    /// of 0 is unbounded and never full.
    fn is_full(&self) -> bool {
        self.capacity != 0 && self.claims.len() >= self.capacity
    }
}

/// The best slot to reclaim when the pool is full: the oldest FINISHED claim
/// by `since_ms`. `None` when every claim is live -- the caller must wait,
/// not evict. Ties on `since_ms` break by slot name.
pub fn reclaim_candidate(p: &Pool) -> Option<String> {
    reclaimable(p).into_iter().next()
}

/// Slots allocated to runs that are no longer live, oldest first. The
/// cleanup worklist. Ties on `since_ms` break by slot name, matching
/// [`reclaim_candidate`].
pub fn reclaimable(p: &Pool) -> Vec<String> {
    let mut finished: Vec<&Claim> = p.claims.values().filter(|claim| !claim.live).collect();
    finished.sort_by(|a, b| a.since_ms.cmp(&b.since_ms).then_with(|| a.slot.cmp(&b.slot)));
    finished
        .into_iter()
        .map(|claim| claim.slot.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocating_over_a_live_holder_names_that_holder() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "task--arm", 100).unwrap();

        let err = p.allocate("run-b", "task--arm", 200).unwrap_err();
        match err {
            AllocError::HeldByLiveRun { slot, holder } => {
                assert_eq!(slot, "task--arm");
                assert_eq!(holder, "run-a");
            }
            other => panic!("expected HeldByLiveRun, got {other:?}"),
        }
        // The refusal displaced nobody.
        assert_eq!(
            p.holder("task--arm").map(|c| c.run_id.as_str()),
            Some("run-a")
        );
        assert_eq!(p.live_count(), 1);
    }

    #[test]
    fn allocating_over_a_finished_holder_transfers() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "task--arm", 100).unwrap();
        assert!(p.finish("run-a"));

        p.allocate("run-b", "task--arm", 200).unwrap();

        let claim = p.holder("task--arm").unwrap();
        assert_eq!(claim.run_id, "run-b");
        assert_eq!(claim.since_ms, 200);
        assert!(claim.live);
        assert_eq!(p.len(), 1);
        assert_eq!(p.live_count(), 1);
        // The old owner holds nothing anymore.
        assert!(!p.finish("run-a"));
    }

    #[test]
    fn reallocating_the_same_run_to_the_same_slot_is_a_noop() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "task--arm", 100).unwrap();

        // A dispatcher retry, some time later: no second claim, no moved
        // start time.
        p.allocate("run-a", "task--arm", 500).unwrap();

        let claim = p.holder("task--arm").unwrap();
        assert_eq!(claim.run_id, "run-a");
        assert_eq!(claim.since_ms, 100);
        assert!(claim.live);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn a_run_holding_two_slots_is_refused() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "s1", 100).unwrap();

        assert!(matches!(
            p.allocate("run-a", "s2", 200).unwrap_err(),
            AllocError::AlreadyHolding { .. }
        ));
        // Nothing was taken: the run keeps its slot and s2 stays free.
        assert_eq!(p.holder("s1").map(|c| c.run_id.as_str()), Some("run-a"));
        assert_eq!(p.holder("s2"), None);
    }

    #[test]
    fn a_finished_claim_still_blocks_a_second_slot() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "s1", 100).unwrap();
        p.finish("run-a");

        assert!(matches!(
            p.allocate("run-a", "s2", 200).unwrap_err(),
            AllocError::AlreadyHolding { .. }
        ));
    }

    #[test]
    fn capacity_zero_is_unbounded() {
        let mut p = Pool::new(0);
        for i in 0..16 {
            p.allocate(&format!("run-{i}"), &format!("slot-{i}"), i as u64)
                .unwrap();
        }
        assert_eq!(p.len(), 16);
        assert_eq!(p.live_count(), 16);
    }

    #[test]
    fn default_pool_is_unbounded() {
        let mut p = Pool::default();
        for i in 0..4 {
            p.allocate(&format!("run-{i}"), &format!("slot-{i}"), 100)
                .unwrap();
        }
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn release_refuses_a_live_holder_but_tolerates_an_empty_slot() {
        let mut p = Pool::new(2);

        // Releasing nothing is not an error.
        p.release("no-such-slot").unwrap();

        p.allocate("run-a", "s", 100).unwrap();
        let err = p.release("s").unwrap_err();
        match err {
            AllocError::HeldByLiveRun { slot, holder } => {
                assert_eq!(slot, "s");
                assert_eq!(holder, "run-a");
            }
            other => panic!("expected HeldByLiveRun, got {other:?}"),
        }

        // Once finished, the same release goes through.
        assert!(p.finish("run-a"));
        p.release("s").unwrap();
        assert_eq!(p.holder("s"), None);
        assert!(p.is_empty());
    }

    #[test]
    fn reclaim_candidate_is_none_when_every_claim_is_live() {
        let mut p = Pool::new(4);
        // An empty pool has nothing to reclaim either.
        assert_eq!(reclaim_candidate(&p), None);

        p.allocate("run-a", "s1", 100).unwrap();
        p.allocate("run-b", "s2", 50).unwrap();
        assert_eq!(reclaim_candidate(&p), None);
        assert!(reclaimable(&p).is_empty());
    }

    #[test]
    fn reclaim_candidate_takes_the_oldest_finished_claim() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "old", 100).unwrap();
        p.allocate("run-b", "new", 200).unwrap();
        p.allocate("run-c", "live", 300).unwrap();
        p.finish("run-b");
        p.finish("run-a");

        assert_eq!(reclaim_candidate(&p), Some("old".to_string()));
        // Ordered by since_ms, and the live claim never appears.
        assert_eq!(
            reclaimable(&p),
            vec!["old".to_string(), "new".to_string()]
        );
    }

    #[test]
    fn reclaim_candidate_breaks_ties_by_slot_name() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "zeta", 100).unwrap();
        p.allocate("run-b", "alpha", 100).unwrap();
        p.allocate("run-c", "mid", 100).unwrap();
        for run in ["run-a", "run-b", "run-c"] {
            p.finish(run);
        }

        assert_eq!(reclaim_candidate(&p), Some("alpha".to_string()));
    }

    #[test]
    fn pool_exhausted_when_full_and_nothing_is_reclaimable() {
        let mut p = Pool::new(2);
        p.allocate("run-a", "s1", 100).unwrap();
        p.allocate("run-b", "s2", 100).unwrap();

        assert_eq!(
            p.allocate("run-c", "s3", 300).unwrap_err(),
            AllocError::PoolExhausted { capacity: 2 }
        );
        // The refusal added nothing.
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn a_reclaimable_slot_does_not_stop_pool_exhaustion() {
        let mut p = Pool::new(2);
        p.allocate("run-a", "s1", 100).unwrap();
        p.allocate("run-b", "s2", 100).unwrap();
        p.finish("run-a"); // s1 is reclaimable, but reclaiming is the caller's act.

        assert_eq!(
            p.allocate("run-c", "s3", 300).unwrap_err(),
            AllocError::PoolExhausted { capacity: 2 }
        );

        // Once the caller releases the finished claim, the allocation fits.
        p.release("s1").unwrap();
        p.allocate("run-c", "s3", 300).unwrap();
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn transferring_a_finished_slot_works_even_at_capacity() {
        let mut p = Pool::new(2);
        p.allocate("run-a", "s1", 100).unwrap();
        p.allocate("run-b", "s2", 100).unwrap();
        p.finish("run-a");

        // A transfer replaces one claim with another; it needs no fresh
        // capacity.
        p.allocate("run-c", "s1", 300).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p.live_count(), 2);
        assert_eq!(
            p.holder("s1").map(|c| c.run_id.as_str()),
            Some("run-c")
        );
    }

    #[test]
    fn reclaimable_lists_finished_slots_oldest_first() {
        let mut p = Pool::new(4);
        p.allocate("run-a", "slow", 300).unwrap();
        p.allocate("run-b", "quick", 100).unwrap();
        p.allocate("run-c", "busy", 200).unwrap();
        p.finish("run-a");
        p.finish("run-b");

        assert_eq!(
            reclaimable(&p),
            vec!["quick".to_string(), "slow".to_string()]
        );
        assert_eq!(reclaim_candidate(&p), Some("quick".to_string()));
    }

    #[test]
    fn finish_an_unknown_run_is_false() {
        let mut p = Pool::new(1);
        assert!(!p.finish("ghost"));
        p.allocate("run-a", "s", 100).unwrap();
        assert!(!p.finish("ghost"));
        // The known run was untouched.
        assert!(p.holder("s").unwrap().live);
    }

    #[test]
    fn finish_is_idempotent() {
        let mut p = Pool::new(1);
        p.allocate("run-a", "s", 100).unwrap();
        assert!(p.finish("run-a"));
        assert!(p.finish("run-a"));

        // Finishing twice must not resurrect, duplicate, or remove the
        // claim.
        assert_eq!(p.live_count(), 0);
        assert_eq!(p.len(), 1);
        assert_eq!(reclaimable(&p), vec!["s".to_string()]);
    }

    #[test]
    fn force_reclaim_removes_even_a_live_holder() {
        let mut p = Pool::new(2);
        p.allocate("run-a", "s", 100).unwrap();

        assert!(p.force_reclaim("s"));
        assert_eq!(p.holder("s"), None);
        assert!(p.is_empty());

        // The slot is immediately usable by another run.
        p.allocate("run-b", "s", 200).unwrap();
        assert_eq!(p.holder("s").map(|c| c.run_id.as_str()), Some("run-b"));
    }

    #[test]
    fn force_reclaim_of_an_unheld_slot_is_false() {
        let mut p = Pool::new(1);
        p.allocate("run-a", "s", 100).unwrap();
        assert!(!p.force_reclaim("other"));
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn force_reclaim_also_takes_a_finished_claim() {
        let mut p = Pool::new(1);
        p.allocate("run-a", "s", 100).unwrap();
        p.finish("run-a");
        assert!(p.force_reclaim("s"));
        assert!(p.is_empty());
    }

    #[test]
    fn holder_reports_the_full_claim() {
        let mut p = Pool::new(1);
        assert_eq!(p.holder("task--arm"), None);

        p.allocate("run-a", "task--arm", 1234).unwrap();
        let claim = p.holder("task--arm").unwrap();
        assert_eq!(claim.run_id, "run-a");
        assert_eq!(claim.slot, "task--arm");
        assert_eq!(claim.since_ms, 1234);
        assert!(claim.live);
    }

    #[test]
    fn live_count_len_and_is_empty_track_the_pool() {
        let mut p = Pool::new(4);
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
        assert_eq!(p.live_count(), 0);

        p.allocate("run-a", "s1", 100).unwrap();
        p.allocate("run-b", "s2", 100).unwrap();
        p.allocate("run-c", "s3", 100).unwrap();
        assert!(!p.is_empty());
        assert_eq!(p.len(), 3);
        assert_eq!(p.live_count(), 3);

        p.finish("run-a");
        p.finish("run-b");
        assert_eq!(p.len(), 3);
        assert_eq!(p.live_count(), 1);

        p.release("s1").unwrap();
        assert!(p.force_reclaim("s2"));
        assert_eq!(p.len(), 1);
        assert_eq!(p.live_count(), 1);
    }

    #[test]
    fn a_released_slot_can_be_reallocated() {
        let mut p = Pool::new(1);
        p.allocate("run-a", "s", 100).unwrap();
        p.finish("run-a");
        p.release("s").unwrap();

        p.allocate("run-b", "s", 200).unwrap();
        assert_eq!(p.holder("s").map(|c| c.run_id.as_str()), Some("run-b"));
        assert_eq!(p.len(), 1);
    }
}
