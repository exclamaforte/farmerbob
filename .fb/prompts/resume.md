<!-- fb:creates crates/farmerbob-core/src/resume.rs -->
# Task: auto-resume at reset, without stampeding the provider

Create `crates/farmerbob-core/src/resume.rs`, declare it from `lib.rs` with
`pub mod resume;`. Change nothing else except that one `pub mod` line.

## Context

`limit_signal` recognises a quota refusal, `quota` parks the bucket, `runstate` holds the run
in `BlockedOnQuota`. This is the last link: deciding which parked runs to restart, when.

The naive answer — resume everything the instant the bucket resets — is wrong in a specific
and expensive way. Every run parked against one bucket becomes due at the same instant, so
they all fire together, and the provider that just rate-limited you receives its largest
burst of the day. Observed here: one provider cut this project off for 153 hours.

So resumption is **ordered and rationed**. It must also be **idempotent**: a bucket can be
reported due repeatedly, and handing the same run back twice produces two agents writing to
one worktree, which is how a dispatcher deletes another dispatcher's work.

**Pure logic: no I/O, no clock reads, no sleeping.** The caller supplies the time and acts
on the plan.

## Exact API — implement these signatures verbatim

```rust
/// A run waiting on a bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    pub run_id: String,
    pub bucket: String,
    pub since_ms: u64,
    /// Absolute instant the provider stated, if it stated one.
    pub resume_at_ms: Option<u64>,
    /// How many times this run has already been resumed and blocked again.
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Most runs released per bucket per call. Must be at least 1; 0 is treated as 1.
    pub batch: usize,
    /// Minimum gap between releases within one bucket.
    pub stagger_ms: u64,
    /// Fallback wait when the provider stated no reset instant.
    pub default_window_ms: u64,
    /// A run blocked this many times is not resumed again; it is `Exhausted`.
    pub max_attempts: u32,
}

/// What to do with one parked run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Release at this instant, at or after `now_ms`.
    Resume { run_id: String, at_ms: u64 },
    /// Not yet due. Carries the instant it becomes due.
    Wait { run_id: String, until_ms: u64 },
    /// Blocked too many times; stop trying and surface it.
    Exhausted { run_id: String },
}

/// Plan the next releases. Returns one Decision per input run, in the input order.
///
/// Within a bucket, runs are released oldest `since_ms` first -- a run that has waited
/// longest goes first, so resumption cannot starve anyone.
pub fn plan(parked: &[Parked], p: Policy, now_ms: u64) -> Vec<Decision>;

/// The run ids `plan` would release, in release order. A convenience over `plan`, and it
/// must agree with it exactly.
pub fn due_now(parked: &[Parked], p: Policy, now_ms: u64) -> Vec<String>;

/// Remove runs already handed out, by id. Resumption is at-most-once per plan: calling
/// `plan` twice without recording the releases must not produce two live agents.
pub fn without(parked: &[Parked], released: &[String]) -> Vec<Parked>;

/// The earliest instant at which `plan` would return any `Resume`, or `None` when every
/// run is exhausted. This is what a scheduler sleeps until.
pub fn next_wakeup(parked: &[Parked], p: Policy, now_ms: u64) -> Option<u64>;
```

## Required behaviour

- A run is due when `now_ms >= resume_at_ms`, or, for `None`, when
  `now_ms - since_ms >= default_window_ms`. Both comparisons must not underflow on a
  `since_ms` in the future; such a run is simply not due.
- At most `batch` runs are released per bucket per call. Buckets are independent: a batch
  of 2 across three buckets releases up to 6.
- The k-th release within a bucket is scheduled at `now_ms + k * stagger_ms`, k starting at
  0. So the first goes immediately and the rest are spread.
- Ordering within a bucket is by ascending `since_ms`; ties by `run_id` ascending, so the
  plan is deterministic.
- A run with `attempts >= max_attempts` is `Exhausted` regardless of whether it is due, and
  does not consume a batch slot.
- `Wait` carries the instant the run becomes due, which for a stated reset is
  `resume_at_ms` and otherwise `since_ms + default_window_ms`, saturating.
- `due_now` returns exactly the `Resume` ids from `plan`, in release order.
- `next_wakeup` is `now_ms` when something is due immediately, otherwise the earliest
  `Wait` instant, and `None` when every run is `Exhausted` or the input is empty.
- `batch` of 0 behaves as 1. A `stagger_ms` of 0 is legal and releases the whole batch at
  `now_ms`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- All arithmetic saturating or checked; no overflow or underflow on adversarial timestamps.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: two buckets each release their own batch; the
k-th release is staggered; oldest-first ordering with a deterministic tie-break; an
exhausted run does not consume a slot; a `since_ms` in the future does not underflow and is
not due; `due_now` agrees with `plan`; `next_wakeup` is `None` when all are exhausted;
`batch` 0 behaves as 1.

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
