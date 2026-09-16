<!-- fb:creates crates/farmerbob-core/src/lease_manager.rs -->
# Task: implement the resource lease manager

Create `crates/farmerbob-core/src/lease_manager.rs`, declare it from `lib.rs` with
`pub mod lease_manager;`. Change nothing else. `lease.rs` already defines `Lease`,
`LeaseClass` and `LeaseId` — read it and reuse those types rather than redefining them.

## Context

farmerbob runs many agents at once on one machine with one GPU. Agents keep writing code in
parallel, but a *measured* experiment must be the sole tenant: any concurrent CUDA work makes
the timing meaningless, which destroys the comparison the whole system exists to produce.

The same mechanism arbitrates three other scarce things, so nothing here may be GPU-specific:
provider accounts (a shared account rate-limits), build/verification slots (concurrent rustc
exhausts memory), and named hardware.

**Pure logic: no I/O, no threads, no clock.** Time is a parameter so the queue is testable.

## Exact API — implement these signatures verbatim

```rust
use crate::lease::{Lease, LeaseClass, LeaseId};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ResourceName(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HolderId(pub String);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RequestOutcome {
    Granted { lease: LeaseId, token: String },
    Queued { position: usize },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Grant { pub resource: ResourceName, pub holder: HolderId, pub lease: LeaseId, pub token: String }

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LeaseStatus {
    pub resource: ResourceName,
    pub holder: Option<HolderId>,
    pub held_secs: u64,
    pub queue_depth: usize,
    pub waiting: Vec<HolderId>,
}

#[derive(Debug, Default)]
pub struct LeaseManager { /* your fields */ }

impl LeaseManager {
    pub fn new() -> Self;
    /// Register a resource with a concurrency limit. `limit` of 1 is exclusive.
    pub fn register(&mut self, resource: ResourceName, limit: usize);
    /// `now` is seconds since an arbitrary epoch, supplied by the caller.
    pub fn request(&mut self, resource: &ResourceName, holder: HolderId, now: u64) -> Option<RequestOutcome>;
    /// Release by token. Grants to the next waiter if there is one.
    pub fn release(&mut self, token: &str) -> Option<Grant>;
    /// Revoke leases held longer than `max_hold_secs`, advancing the queue. Returns what was revoked.
    pub fn expire(&mut self, now: u64, max_hold_secs: u64) -> Vec<Grant>;
    /// A holder's process vanished: drop its lease AND any queued requests from it.
    pub fn holder_died(&mut self, holder: &HolderId) -> Vec<Grant>;
    pub fn status(&self, resource: &ResourceName, now: u64) -> Option<LeaseStatus>;
}
```

## Invariants — write one test per invariant, named after it

1. Never more than `limit` holders of a resource at once.
2. Grants are strictly FIFO among waiters.
3. A resource is **never left locked**: after `release`, `expire` or `holder_died`, either a
   waiter holds it or it is free. This is the one that matters most — a leaked lease
   deadlocks every future measurement.
4. The same holder requesting a resource twice does not get two leases.
5. `release` with an unknown or already-released token is a harmless no-op, not a panic.
6. `request` for an unregistered resource returns `None` rather than inventing one.
7. `holder_died` removes that holder's queued requests as well as its held lease.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No arithmetic that can overflow or divide by zero, including a `now` earlier than the
  acquisition time.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

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
