# Task: implement the resource lease manager

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.

Create `crates/farmerbob-core/src/lease.rs` and declare it from `lib.rs` with
`pub mod lease;`. Do not modify anything else.

## Context

farmerbob runs many AI coding agents at once on one machine with a single GPU. Agents
keep writing code in parallel, but when one wants to *measure* a GPU kernel it must
be the sole tenant — any concurrent CUDA work makes the timing meaningless. So access
is handed out as exclusive leases from a queue.

## What to implement

A `LeaseManager` — pure in-memory logic, **no I/O, no threads, no async, no clock
calls**. Time is passed in by the caller as a parameter so it can be tested.

```rust
pub struct ResourceName(pub String);
pub struct LeaseToken(pub String);
pub struct HolderId(pub String);   // the run holding or wanting the lease
```

API:

- `LeaseManager::new()` and `register_resource(name, ResourceName)`.
- `request(resource, holder, now) -> RequestOutcome` where `RequestOutcome` is either
  `Granted(LeaseToken)` or `Queued { position: usize }`.
- `release(token) -> Option<Grant>` — releases, then grants to the next waiter in FIFO
  order if there is one. Returns who just got it, so the caller can wake them.
- `expire(now) -> Vec<LeaseToken>` — revokes leases held past their max hold duration
  and advances the queue. Returns what was revoked.
- `holder_died(holder) -> Option<Grant>` — a holder's process vanished: force-release
  whatever it held, drop any queued requests from it, advance the queue.
- `status(resource) -> LeaseStatus` — current holder, how long held, queue depth,
  and the ordered list of waiting holders.

## Required invariants — test each one

1. Only ever **one** holder per resource at a time.
2. Grants are strictly **FIFO** among waiters.
3. A resource is **never left locked** after `release`, `expire`, or `holder_died` —
   if a waiter exists it is granted, otherwise the resource is free.
4. The same holder requesting twice does not get two leases.
5. `release` with an unknown or already-released token is a harmless no-op, not a panic.

## Rules

- No `unwrap()` on anything that can fail in normal operation.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run
  them yourself and fix any failures before finishing.
- Write a unit test per invariant above, named after it.

When done, briefly state what you implemented.

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
4. **Test depth** — number of distinct behaviours covered, not number of assertions.
5. **Documentation** — `///` on every public item.
6. **Structure** — coherent modules over one large file, where the crate warrants it.

**Not scored:** wallclock. Taking longer to produce better work is the preferred trade.
There is a generous resource budget; a run is cut early only if it stops making progress or
regresses past its own best error count.
