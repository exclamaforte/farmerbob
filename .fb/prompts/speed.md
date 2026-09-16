<!-- fb:creates crates/farmerbob-core/src/timing.rs -->
# Task: speed as a first-class, crash-durable measurement

Create `crates/farmerbob-core/src/timing.rs`, declare it from `lib.rs` with
`pub mod timing;`. Change nothing else except that one `pub mod` line.

## Context

Total wallclock does not say what a planning agent needs when choosing whom to dispatch to.
Measured on one identical task across ten implementers, time-to-first-artefact spanned
**25x** — 28s to 708s — while total time spanned only 25x in a different order. The two
rankings disagree. An agent that emits its first file in 28 seconds and finishes in 28 is a
different proposition from one that thinks for 594 seconds and then finishes instantly,
even though a single number can rank them adjacently.

Three quantities, not one:

- **TTFW** — time to first write. How long until there is any evidence of progress.
- **Span** — first write to last write. How long the agent was actually producing.
- **Total** — dispatch to exit, including the tail after the last write.

A large `total - (ttfw + span)` is an agent that finished working and then kept the slot.

This must survive a crash. A timing record that only exists in the process that died tells
you nothing about the run that died, which is exactly the run you wanted to know about.

**Pure logic: no I/O.** The caller feeds observations in; this module folds them.

## Exact API — implement these signatures verbatim

```rust
/// One observation, as seen by the harness. Times are unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Dispatched { at_ms: u64 },
    Wrote { at_ms: u64 },
    Exited { at_ms: u64 },
}

/// Accumulates marks into the three quantities. Cheap to snapshot after every mark,
/// which is what makes it crash-durable: the caller persists the snapshot, not the stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Timing { /* your fields */ }

impl Timing {
    pub fn new() -> Self;
    /// Fold one mark. Out-of-order or duplicate marks must not corrupt the record;
    /// see Required behaviour.
    pub fn observe(&mut self, mark: Mark);
    /// Milliseconds from dispatch to the first write. `None` until both have happened.
    pub fn ttfw_ms(&self) -> Option<u64>;
    /// Milliseconds from first write to last write. `None` until a write has happened.
    /// Zero when there was exactly one write.
    pub fn span_ms(&self) -> Option<u64>;
    /// Milliseconds from dispatch to exit. `None` until the run has exited.
    pub fn total_ms(&self) -> Option<u64>;
    /// Milliseconds after the last write before exit. `None` unless both are known.
    pub fn tail_ms(&self) -> Option<u64>;
    pub fn writes(&self) -> u64;
    /// True once an `Exited` mark has been folded.
    pub fn is_complete(&self) -> bool;
}

/// A record recovered from persistence after a crash, where `Exited` never arrived.
///
/// `last_known_ms` is the newest time the harness is willing to vouch for, e.g. the
/// mtime of the log. Returns a `Timing` whose total is bounded by that instead of
/// being unknown forever.
pub fn recover(partial: &Timing, last_known_ms: u64) -> Timing;
```

## Required behaviour

- A second `Dispatched` mark is ignored; the first wins. Re-dispatch is a new run, not an
  amendment to this one.
- A `Wrote` mark earlier than `Dispatched` is ignored as impossible, and does not increment
  `writes()`.
- `Wrote` marks may arrive out of order relative to one another; `span_ms` uses the true
  minimum and maximum, not arrival order.
- An `Exited` mark earlier than the last write is clamped to the last write, yielding a
  `tail_ms` of zero rather than an underflow. Filesystem timestamps and process exit are
  measured by different clocks and disagree by milliseconds.
- Every accessor returns `None` rather than a sentinel when its inputs have not happened.
  Zero and "unknown" must be distinguishable: an agent that wrote nothing and an agent that
  wrote instantly are opposite outcomes.
- `recover` never shortens a known quantity, and marks the result complete.
- `observe` is idempotent for identical `Dispatched` and `Exited` marks.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No arithmetic that can overflow or underflow on adversarial input. Use checked or
  saturating operations.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `Timing` must derive or implement `serde::Serialize` and `serde::Deserialize`, since the
  whole point is that it is persisted after every mark.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: out-of-order writes still give the right span; a
pre-dispatch write is ignored; exit before last write clamps tail to zero; a second dispatch
is ignored; every accessor is `None` before its inputs; one write gives span zero; `recover`
bounds the total; a round-trip through serde preserves every quantity.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- only the crate named in the task is modified

**Scored, in this order:**
1. **Conformance** — a test suite you will not see, derived from this spec, is run against
   your implementation. The assertions are hidden; the criteria are exactly what this
   document states.
2. **Panic-freedom** — no `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on
   any path reachable from input, outside `#[cfg(test)]`.
3. **`cargo clippy -- -D warnings` clean.**
4. **Test depth and generality** — number of distinct behaviours covered, not number of
   assertions. Your tests must be good enough to catch a bug in **any** correct-looking
   implementation of this spec, not only your own: test the behaviour the specification
   requires, not your particular implementation's internals. Asserting on exact error
   strings, private field names, or an output format the spec does not fix makes a test
   worthless.
5. **Documentation** — `///` on every public item.
6. **Structure** — coherent modules over one large file, where the crate warrants it.

**Not scored:** wallclock. Taking longer to produce better work is the preferred trade.
There is a generous resource budget; a run is cut early only if it stops making progress or
regresses past its own best error count.

## Handoff (required)

When you are done, write `.fb/handoff.md` in the repository root. Keep it under 300 words.

**Do not state anything the harness can check.** No test counts, no "all tests pass", no "this
handles empty input", no performance claims. Those are measured independently and a claim
about them adds nothing — the harness has already run them by the time anyone reads this.

Write only what cannot be measured:

- **Approach.** The shape of the solution and why this shape rather than an obvious alternative.
- **Trade-offs.** What you chose against, and what it would cost to choose differently.
- **Risk.** Where you think this is most likely to be wrong, or hardest to change later.
- **Deliberate omissions.** What the spec allows that you did not do, and why.

If you found the specification ambiguous or underdetermined, say exactly where. That is the
most valuable thing this file can contain: it routes back to the task author instead of
becoming a defect argued about later.
