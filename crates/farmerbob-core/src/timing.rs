//! Timing measurements for a single agent run.
//!
//! A run is described by three quantities rather than one wallclock number:
//!
//! - **TTFW** (`ttfw_ms`) — dispatch to first write: how long until there is
//!   any evidence of progress.
//! - **Span** (`span_ms`) — first write to last write: how long the agent was
//!   actually producing.
//! - **Total** (`total_ms`) — dispatch to exit, including the tail after the
//!   last write. `tail_ms` exposes that tail on its own.
//!
//! The module is pure: it performs no I/O. The caller folds [`Mark`]s into a
//! [`Timing`] and persists a [`serde`]-serialized snapshot after every mark,
//! which is what makes the record crash-durable — the stream dies with the
//! process, the snapshot survives. A crashed run is recovered with
//! [`recover`].

use serde::{Deserialize, Serialize};

/// One observation, as seen by the harness. Times are unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    /// The agent was dispatched to begin work.
    Dispatched { at_ms: u64 },
    /// The agent produced a write (an artefact of progress).
    Wrote { at_ms: u64 },
    /// The agent exited.
    Exited { at_ms: u64 },
}

/// Accumulates marks into the three quantities.
///
/// Cheap to snapshot after every mark, which is what makes it crash-durable:
/// the caller persists the snapshot, not the stream. All cross-field
/// arithmetic is order-independent and saturating, so adversarial or
/// out-of-order marks cannot corrupt the record or panic.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Timing {
    dispatch_ms: Option<u64>,
    first_write_ms: Option<u64>,
    last_write_ms: Option<u64>,
    writes: u64,
    exit_ms: Option<u64>,
    complete: bool,
}

impl Timing {
    /// Creates an empty record awaiting its first mark.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one mark.
    ///
    /// Out-of-order or duplicate marks do not corrupt the record: a second
    /// `Dispatched` is ignored (the first wins), a `Wrote` before dispatch is
    /// ignored, and `Wrote` marks may arrive in any order.
    pub fn observe(&mut self, mark: Mark) {
        match mark {
            Mark::Dispatched { at_ms } => {
                if self.dispatch_ms.is_none() {
                    self.dispatch_ms = Some(at_ms);
                }
            }
            Mark::Wrote { at_ms } => {
                // A write before dispatch is impossible; ignore it without
                // incrementing `writes`.
                let Some(dispatch) = self.dispatch_ms else {
                    return;
                };
                if at_ms < dispatch {
                    return;
                }
                self.first_write_ms = Some(match self.first_write_ms {
                    Some(prev) => prev.min(at_ms),
                    None => at_ms,
                });
                self.last_write_ms = Some(match self.last_write_ms {
                    Some(prev) => prev.max(at_ms),
                    None => at_ms,
                });
                self.writes = self.writes.saturating_add(1);
            }
            Mark::Exited { at_ms } => {
                if self.exit_ms.is_none() {
                    self.exit_ms = Some(at_ms);
                    self.complete = true;
                }
            }
        }
    }

    /// Milliseconds from dispatch to the first write.
    ///
    /// `None` until both dispatch and a write have happened.
    pub fn ttfw_ms(&self) -> Option<u64> {
        match (self.dispatch_ms, self.first_write_ms) {
            (Some(d), Some(fw)) => fw.checked_sub(d),
            _ => None,
        }
    }

    /// Milliseconds from first write to last write.
    ///
    /// `None` until a write has happened. Zero when there was exactly one
    /// write.
    pub fn span_ms(&self) -> Option<u64> {
        match (self.first_write_ms, self.last_write_ms) {
            (Some(fw), Some(lw)) => lw.checked_sub(fw),
            _ => None,
        }
    }

    /// Milliseconds from dispatch to exit.
    ///
    /// `None` until the run has exited (or been [`recover`]ed).
    pub fn total_ms(&self) -> Option<u64> {
        match (self.dispatch_ms, self.effective_exit_ms()) {
            (Some(d), Some(e)) => e.checked_sub(d),
            _ => None,
        }
    }

    /// Milliseconds after the last write before exit.
    ///
    /// `None` unless both a write and an exit are known. Zero when exit did
    /// not outlast the last write.
    pub fn tail_ms(&self) -> Option<u64> {
        match (self.last_write_ms, self.effective_exit_ms()) {
            (Some(lw), Some(e)) => e.checked_sub(lw),
            _ => None,
        }
    }

    /// The number of valid writes folded so far.
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// True once an `Exited` mark has been folded (or the record has been
    /// [`recover`]ed).
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The exit time clamped up to the last write and dispatch, so that
    /// cross-clock skew between filesystem timestamps and process exit never
    /// produces an underflow.
    fn effective_exit_ms(&self) -> Option<u64> {
        let exit = self.exit_ms?;
        let mut bound = exit;
        if let Some(lw) = self.last_write_ms {
            bound = bound.max(lw);
        }
        if let Some(d) = self.dispatch_ms {
            bound = bound.max(d);
        }
        Some(bound)
    }
}

/// Recovers a record after a crash, where `Exited` never arrived.
///
/// `last_known_ms` is the newest time the harness is willing to vouch for,
/// e.g. the mtime of the log. Returns a `Timing` whose total is bounded by
/// that time instead of being unknown forever. Never shortens a known
/// quantity, and marks the result complete.
pub fn recover(partial: &Timing, last_known_ms: u64) -> Timing {
    let mut t = partial.clone();
    if t.exit_ms.is_none() {
        let mut bound = last_known_ms;
        if let Some(lw) = t.last_write_ms {
            bound = bound.max(lw);
        }
        if let Some(d) = t.dispatch_ms {
            bound = bound.max(d);
        }
        t.exit_ms = Some(bound);
    }
    t.complete = true;
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_order_writes_give_right_span() {
        let mut t = Timing::new();
        t.observe(Mark::Dispatched { at_ms: 0 });
        t.observe(Mark::Wrote { at_ms: 700 });
        t.observe(Mark::Wrote { at_ms: 100 });
        t.observe(Mark::Wrote { at_ms: 400 });
        t.observe(Mark::Exited { at_ms: 700 });
        assert_eq!(t.ttfw_ms(), Some(100));
        assert_eq!(t.span_ms(), Some(600));
        assert_eq!(t.writes(), 3);
    }

    #[test]
    fn pre_dispatch_write_is_ignored() {
        let mut t = Timing::new();
        t.observe(Mark::Wrote { at_ms: 50 });
        assert_eq!(t.writes(), 0);
        assert_eq!(t.first_write_ms, None);
        t.observe(Mark::Dispatched { at_ms: 100 });
        t.observe(Mark::Wrote { at_ms: 150 });
        assert_eq!(t.writes(), 1);
        assert_eq!(t.ttfw_ms(), Some(50));
    }

    #[test]
    fn exit_before_last_write_clamps_tail_to_zero() {
        let mut t = Timing::new();
        t.observe(Mark::Dispatched { at_ms: 0 });
        t.observe(Mark::Wrote { at_ms: 100 });
        t.observe(Mark::Wrote { at_ms: 200 });
        t.observe(Mark::Exited { at_ms: 150 });
        assert_eq!(t.tail_ms(), Some(0));
        assert_eq!(t.total_ms(), Some(200));
    }

    #[test]
    fn second_dispatch_is_ignored() {
        let mut t = Timing::new();
        t.observe(Mark::Dispatched { at_ms: 100 });
        t.observe(Mark::Dispatched { at_ms: 999 });
        t.observe(Mark::Wrote { at_ms: 150 });
        assert_eq!(t.ttfw_ms(), Some(50));
    }

    #[test]
    fn accessors_none_before_inputs() {
        let t = Timing::new();
        assert_eq!(t.ttfw_ms(), None);
        assert_eq!(t.span_ms(), None);
        assert_eq!(t.total_ms(), None);
        assert_eq!(t.tail_ms(), None);
        assert!(!t.is_complete());

        let mut t = Timing::new();
        t.observe(Mark::Dispatched { at_ms: 0 });
        assert_eq!(t.ttfw_ms(), None);
        assert_eq!(t.span_ms(), None);
        assert_eq!(t.total_ms(), None);
        assert_eq!(t.tail_ms(), None);
        assert!(!t.is_complete());
    }

    #[test]
    fn one_write_gives_span_zero() {
        let mut t = Timing::new();
        t.observe(Mark::Dispatched { at_ms: 0 });
        t.observe(Mark::Wrote { at_ms: 100 });
        t.observe(Mark::Exited { at_ms: 200 });
        assert_eq!(t.span_ms(), Some(0));
        assert_eq!(t.ttfw_ms(), Some(100));
        assert_eq!(t.tail_ms(), Some(100));
    }

    #[test]
    fn recover_bounds_total() {
        let mut partial = Timing::new();
        partial.observe(Mark::Dispatched { at_ms: 0 });
        partial.observe(Mark::Wrote { at_ms: 100 });
        assert_eq!(partial.total_ms(), None);
        assert!(!partial.is_complete());

        let r = recover(&partial, 500);
        assert_eq!(r.total_ms(), Some(500));
        assert!(r.is_complete());
        assert_eq!(r.writes(), 1);
    }

    #[test]
    fn recover_never_shortens_known_quantity() {
        let mut partial = Timing::new();
        partial.observe(Mark::Dispatched { at_ms: 0 });
        partial.observe(Mark::Wrote { at_ms: 100 });
        partial.observe(Mark::Wrote { at_ms: 800 });
        // A crash log mtime smaller than the last write must not shorten it.
        let r = recover(&partial, 300);
        assert_eq!(r.total_ms(), Some(800));
        assert_eq!(r.tail_ms(), Some(0));
    }

    #[test]
    fn serde_round_trip_preserves_quantities() {
        let mut t = Timing::new();
        t.observe(Mark::Dispatched { at_ms: 10 });
        t.observe(Mark::Wrote { at_ms: 40 });
        t.observe(Mark::Wrote { at_ms: 90 });
        t.observe(Mark::Exited { at_ms: 200 });
        let json = serde_json::to_string(&t).unwrap();
        let back: Timing = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        assert_eq!(back.ttfw_ms(), Some(30));
        assert_eq!(back.span_ms(), Some(50));
        assert_eq!(back.total_ms(), Some(190));
        assert_eq!(back.tail_ms(), Some(110));
        assert_eq!(back.writes(), 2);
        assert!(back.is_complete());
    }

    #[test]
    fn observe_is_idempotent_for_identical_dispatch_and_exit() {
        let mut a = Timing::new();
        let mut b = Timing::new();
        a.observe(Mark::Dispatched { at_ms: 0 });
        a.observe(Mark::Wrote { at_ms: 100 });
        a.observe(Mark::Exited { at_ms: 200 });
        b.observe(Mark::Dispatched { at_ms: 0 });
        b.observe(Mark::Dispatched { at_ms: 0 });
        b.observe(Mark::Wrote { at_ms: 100 });
        b.observe(Mark::Exited { at_ms: 200 });
        b.observe(Mark::Exited { at_ms: 200 });
        assert_eq!(a, b);
    }
}
