<!-- fb:modifies crates/farmerbob-core/src/lease.rs -->
# Task: implement the resource lease manager

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.

`crates/farmerbob-core/src/lease.rs` ALREADY EXISTS and defines the `Lease`, `LeaseClass`
and `LeaseId` **types**. It has no manager. Add the `LeaseManager` described below to that
file, reusing the existing types rather than redefining them. Do not modify anything else.

Read the existing file first. If a type you need is already there, use it.

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

- `LeaseManager::new() -> LeaseManager`
- `register_resource(&mut self, name: ResourceName, max_hold: Duration)` — the max hold is
  **per resource and supplied here**. There is no crate-level default and no global constant.

**Use these exact parameter types. Do NOT make them generic** — no `impl Into<ResourceName>`,
no `impl AsRef<str>`, no custom clock trait. Time is `chrono::DateTime<Utc>` and durations are
`chrono::Duration`, everywhere, concretely.

**These three types are fixed, field for field, in this order:**

```rust
pub enum RequestOutcome { Granted(LeaseToken), Queued { position: usize } }

pub struct Grant {
    pub resource: ResourceName,
    pub holder:   HolderId,
    pub lease:    LeaseId,
}

pub struct LeaseStatus {
    pub resource:  ResourceName,
    pub holder:    Option<HolderId>,
    pub held_for:  Option<Duration>,
    pub queue_depth: usize,
    pub waiting:   Vec<HolderId>,   // FIFO order, front of queue first
}
```
- `request(resource, holder, now) -> RequestOutcome` where `RequestOutcome` is either
  `Granted(LeaseToken)` or `Queued { position: usize }`.
- `release(token, now) -> Option<Grant>` — releases, then grants to the next waiter in
  FIFO order if there is one. Returns who just got it, so the caller can wake them.
- `expire(now) -> Vec<LeaseToken>` — revokes leases held past their max hold duration
  and advances the queue. Returns what was revoked.
- `holder_died(holder, now) -> Vec<Grant>` — a holder's process vanished: force-release
  whatever it held, drop any queued requests from it, advance the queue.
- `status(resource, now) -> LeaseStatus` — current holder, how long held, queue depth,
  and the ordered list of waiting holders.

**Time provenance — pinned, because the first version of this spec did not pin it and three
independent critics found the consequence in three different implementations.** Every method
that can create a grant takes `now`, and **a grant always stamps `acquired_at = now`**. A
lease handed to a waiter by `release`, `expire` or `holder_died` starts its full TTL at the
moment of handover; it NEVER inherits the predecessor's `acquired_at`. No method may call
`Utc::now()` internally — the clock is always the caller's.

**`holder_died` returns EVERY grant it produces, and an empty `Vec` when it produced none.**

**The order is the order the resources were REGISTERED via `register_resource`, not the order
the dying holder acquired them.** The previous wording said "the order the resources were
granted", and two independent critics -- or-qwen38-flash and or-muse-spark, reviewing
different subjects -- showed that is ambiguous: implementations iterate in registration order,
which disagrees with acquisition order whenever resources are first granted out of
registration order. or-qwen38-flash gave the probe: acquire gpu1 then gpu0, kill the holder,
and the grants come back `["gpu0", "gpu1"]`. Registration order is chosen because it is
stable, observable from the manager alone, and needs no per-grant sequence number. A holder may hold several resources at once.
The previous `Option<Grant>` could report at most one, which made invariant 3 below
unreportable for the second and subsequent resource, and every implementation either
dropped the extra grants or silently skipped advancing those queues.

**`status(resource, now).held_for` is `now - acquired_at` for the current holder, and is
absent when there is no holder.** Without a `now` it was not computable at all, and several
implementations correctly-by-necessity returned `Duration::zero()` for a field the spec
described as "how long the current holder has held the lease".

## Required invariants — test each one

1. Only ever **one** holder per resource at a time.
2. Grants are strictly **FIFO** among waiters.
3. A resource is **never left locked** after `release`, `expire`, or `holder_died` —
   if a waiter exists it is granted, otherwise the resource is free.
4. The same holder requesting twice does not get two leases.
5. `release` with an unknown or already-released token is a harmless no-op, not a panic.
6. After `holder_died(h, now)` no resource `h` held is left holder-less with a non-empty
   queue, and the returned `Vec<Grant>` names every waiter that was promoted. A resource
   with no holder and a waiting queue is the specific failure this invariant exists to
   forbid: the next requester is granted ahead of the queue, breaking invariant 2.

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
