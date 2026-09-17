<!-- fb:creates crates/farmerbob-core/src/wtalloc.rs -->
# Task: worktree allocation — deciding which runs may hold a tree, and when to reclaim

Create `crates/farmerbob-core/src/wtalloc.rs`, declare it from `lib.rs` with
`pub mod wtalloc;`. Change nothing else except that one `pub mod` line.

## Context

Each run gets its own git worktree. Allocation looks trivial and is not: this project
destroyed eight of ten runs in one wave because two dispatchers each believed they owned the
same worktree, and one removed it while the other's agent was still writing.

The rules that prevent that are about *ownership*, not about paths:

- A worktree is owned by exactly one run at a time, and an owner is displaced only by
  explicit reclamation, never by a second allocation quietly winning.
- Reclaiming a tree whose run is still live is the destructive case. It must be impossible
  to express accidentally, which means a live holder can only be reclaimed through a
  distinct, named operation.
- Disk is finite. When the pool is full the allocator must say so rather than overcommit,
  and it must be able to nominate the best reclamation candidate — which is the oldest
  *finished* run, never a live one.

**Pure logic: no filesystem, no git.** The caller performs the operations this decides on.

## Exact API — implement these signatures verbatim

```rust
/// A run's claim on a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub run_id: String,
    /// Relative slot name, e.g. "task--arm". Unique per live claim.
    pub slot: String,
    pub since_ms: u64,
    /// False once the run has finished, whatever its verdict.
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError {
    /// Another run holds this slot and is still live.
    HeldByLiveRun { slot: String, holder: String },
    /// The pool is at capacity and nothing is reclaimable.
    PoolExhausted { capacity: usize },
    /// This run already holds a different slot.
    AlreadyHolding { run_id: String, slot: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pool { /* your fields */ }

impl Pool {
    /// `capacity` of 0 means unbounded.
    pub fn new(capacity: usize) -> Self;
    /// Claim a slot for a run.
    pub fn allocate(&mut self, run_id: &str, slot: &str, now_ms: u64) -> Result<(), AllocError>;
    /// Mark a run finished. Its slot stays allocated but becomes reclaimable.
    pub fn finish(&mut self, run_id: &str) -> bool;
    /// Release a slot held by a FINISHED run. Refuses a live holder -- use `force_reclaim`.
    pub fn release(&mut self, slot: &str) -> Result<(), AllocError>;
    /// Release regardless of liveness. Separate and named so that destroying a live run's
    /// work is always a deliberate act.
    pub fn force_reclaim(&mut self, slot: &str) -> bool;
    pub fn holder(&self, slot: &str) -> Option<&Claim>;
    pub fn live_count(&self) -> usize;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

/// The best slot to reclaim when the pool is full: the oldest FINISHED claim by `since_ms`.
/// `None` when every claim is live -- the caller must wait, not evict.
pub fn reclaim_candidate(p: &Pool) -> Option<String>;

/// Slots allocated to runs that are no longer live, oldest first. The cleanup worklist.
pub fn reclaimable(p: &Pool) -> Vec<String>;
```

## Required behaviour

- `allocate` on a slot held by a LIVE run is `HeldByLiveRun`, naming the holder. It must
  never silently displace.
- `allocate` on a slot held by a FINISHED run succeeds and transfers ownership.
- `allocate` for a run that already holds a different slot is `AlreadyHolding`. One run,
  one tree.
- Re-allocating the SAME run to the SAME slot succeeds and is a no-op, so a retry is safe.
- `allocate` when `len() >= capacity` and nothing is reclaimable is `PoolExhausted`. When
  something is reclaimable, allocation still fails with `PoolExhausted` — this module
  decides, it does not evict on its own.
- `capacity` 0 is unbounded and never yields `PoolExhausted`.
- `release` on a live holder is `HeldByLiveRun`; on an unheld slot it succeeds silently,
  since releasing nothing is not an error.
- `reclaim_candidate` ignores live claims entirely and breaks a `since_ms` tie by slot name.
- `finish` returns false for an unknown run and is idempotent.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: allocating over a live holder names that holder;
allocating over a finished holder transfers; the same run re-allocating its own slot is a
no-op; a run holding two slots is refused; capacity 0 never exhausts; release refuses a live
holder but tolerates an empty slot; `reclaim_candidate` is None when all are live; ties
break by slot name.

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

**How to read a list in this spec.** Every enumerated list of keywords, formats or cases
states its own status, and you should read it literally:

- *"exactly these and no others"* — accepting anything further is a defect.
- *"at least these; accepting more is neither required nor penalised"* — a superset is fine,
  and **your tests may not assert on cases outside the list**, because another correct
  implementation may reasonably not handle them.
- *"at least these, plus the obvious morphological variants"* — the stemming rule is pinned
  where it says so.

If a list carries no such marker, treat it as the second form, and say so in your handoff.

**And to the composition of anything aggregate you return.** If a function returns a
table, a tuple, or a collection whose membership is not forced by its type, the spec states
exactly what is in it and in what order. Where it does not, say so in your handoff and do
not let your tests assert on it.

**The same applies to every numeric boundary.** Where a clause says "after N has elapsed",
"at least N", or "below N", the behaviour AT N and at the degenerate value (N = 0, an empty
collection, a timestamp that runs backwards) is part of the contract. If the spec does not
pin it, your tests may not assert on it either -- another correct implementation may
reasonably choose the other side. Say in your handoff which boundary you found unpinned and
which way you resolved it.

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
because a test asserted a case the specification never fixed.

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
