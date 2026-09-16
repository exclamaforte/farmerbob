<!-- fb:creates crates/farmerbob-core/src/divergence.rs -->
# Task: cut a run on divergence, never on wallclock

Create `crates/farmerbob-core/src/divergence.rs`, declare it from `lib.rs` with
`pub mod divergence;`. Change nothing else except that one `pub mod` line.

## Context

Slow and good is the goal. Measured on one identical task, time-to-first-artefact spanned
**25x** across implementers, and the slowest arms produced some of the best work — one took
1579 seconds and passed, another failed in 8. A wallclock timeout would have killed the
winner and kept the failure. An earlier bug in this project did exactly that: a launcher's
5-minute cap silently truncated every long run, and the arm's successes were all under five
minutes because its longer attempts were being severed mid-thought.

So a run is cut only when it stops *converging*. Convergence is measured on the run's own
error count: a candidate that reduces its compile errors from 40 to 12 to 3 is working, and
one that has sat at 7 errors for six consecutive checks is not. The rule must also never
punish a run for a transient spike — a refactor that temporarily raises the error count
before lowering it is normal, and productive.

**Pure logic: no I/O, no clock reads.** Observations are fed in.

## Exact API — implement these signatures verbatim

```rust
/// One progress sample taken during a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub at_ms: u64,
    /// Compile/test errors outstanding. Lower is better; 0 means the gate passes.
    pub errors: u32,
    /// Bytes written to the worktree so far. Monotonic.
    pub bytes_written: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Converging, or not yet enough evidence. Keep going.
    Continue,
    /// No improvement on the best error count for `stalled_checks` consecutive samples
    /// AND no new bytes written. Both conditions are required.
    Cut { reason: String, best_errors: u32, stalled_checks: u32 },
    /// The run reached zero errors. Stop for success, not for failure.
    Done,
}

/// Tunables. A caller that wants a wallclock timeout must not find one here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Consecutive non-improving samples tolerated before cutting. Must be >= 2.
    pub patience: u32,
    /// A sample writing at least this many new bytes counts as progress on its own.
    pub min_bytes_progress: u64,
}

impl Policy {
    /// patience 6, min_bytes_progress 1.
    pub fn lenient() -> Self;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tracker { /* your fields */ }

impl Tracker {
    pub fn new(policy: Policy) -> Self;
    /// Fold one sample and decide. Later samples with an EARLIER `at_ms` than the
    /// newest seen are ignored entirely.
    pub fn observe(&mut self, s: Sample) -> Decision;
    /// Lowest error count seen. `None` before any sample.
    pub fn best_errors(&self) -> Option<u32>;
    /// Consecutive samples since the last improvement.
    pub fn stalled(&self) -> u32;
}

/// Did this run regress past its own best? Returns the amount, `None` when it never did.
/// A run whose errors climb above its best and stay there is diverging, not converging.
pub fn regression(samples: &[Sample]) -> Option<u32>;
```

## Required behaviour

- A cut requires BOTH no error improvement for `patience` consecutive samples AND no new
  bytes in that window. A run writing code is working even while its error count is flat.
- A temporary rise in errors followed by a new best does NOT reset progress to zero and
  does not count toward `patience` once the new best lands.
- `errors == 0` yields `Done` immediately, ahead of any cut condition.
- `patience` below 2 is treated as 2: cutting on a single non-improving sample is a
  wallclock timeout wearing a different name.
- Out-of-order samples are ignored, and do not advance `stalled`.
- `best_errors` never rises.
- No wallclock duration appears in any cut decision. `at_ms` is used only to order samples.
- `regression` compares each sample against the best seen BEFORE it, and returns the
  largest such excess.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No arithmetic that can overflow or underflow on adversarial input.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a flat error count with new bytes is not cut; a
flat error count with no bytes is cut at exactly `patience`; a spike followed by a new best
clears the stall; zero errors is Done even when stalled; patience of 0 or 1 behaves as 2;
an out-of-order sample changes nothing; `regression` finds the largest excess over the
running best; a run that only improves never cuts.

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
