<!-- fb:creates crates/farmerbob-core/src/quota.rs -->
# Task: implement usage-limit detection and resumption state

Create `crates/farmerbob-core/src/quota.rs`, declare it from `lib.rs` with `pub mod quota;`.
Change nothing else.

## Context

Most of farmerbob's agents are free until they hit a provider usage limit. A run that hits
one must **park cleanly with its context intact** and come back by itself when the window
resets — not fail and lose an hour of work.

Two facts shape this. A parked run must **release its slot** rather than hold a seat while
waiting. And a parked run is **not a failure**: it says nothing about the arm's ability, so it
must never reach the success statistics.

The subtlety that makes this worth its own module: several providers share one bucket. Two
different CLIs authenticated to the same account draw down the same limit, so the bucket —
not the arm — is what gets parked.

**Pure logic: no I/O, no clock.** Times are passed in as seconds since an arbitrary epoch.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Bucket(pub String);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LimitHit {
    pub bucket: Bucket,
    pub detected_at: u64,
    /// When the provider says the window resets. `None` when it did not say.
    pub reset_at: Option<u64>,
    /// The text the detection matched, kept as evidence.
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BucketState {
    Available,
    /// Parked until `until`; `attempts` counts consecutive parks, for backoff.
    Parked { until: u64, attempts: u32 },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResumeHandle {
    /// Adapter-specific session identifier, so the run continues rather than restarting.
    pub session_id: String,
    pub worktree: String,
}

#[derive(Debug, Default)]
pub struct QuotaTracker { /* your fields */ }

impl QuotaTracker {
    pub fn new(default_window_secs: u64) -> Self;

    /// Scan output for a limit. `markers` are substrings that identify a refusal; matching is
    /// case-insensitive. Returns `None` when nothing matched.
    ///
    /// When the output contains `retry after <N>` or `reset in <N>` (seconds, decimal digits),
    /// `reset_at` is `now + N`. Otherwise `reset_at` is `None` and the caller falls back to
    /// the configured window.
    pub fn detect(&self, bucket: &Bucket, output: &str, markers: &[String], now: u64) -> Option<LimitHit>;

    /// Park a bucket. Uses `hit.reset_at` when present, else `now + default_window_secs`
    /// doubled per consecutive park, capped at 24 hours.
    pub fn park(&mut self, hit: &LimitHit, handle: ResumeHandle, now: u64);

    pub fn state(&self, bucket: &Bucket, now: u64) -> BucketState;

    /// Buckets whose park has expired, with the handles waiting on them. Clears them.
    pub fn due(&mut self, now: u64) -> Vec<(Bucket, Vec<ResumeHandle>)>;

    /// A successful run clears the consecutive-park counter for that bucket.
    pub fn succeeded(&mut self, bucket: &Bucket);

    /// Wall time a bucket has spent parked. Excluded from an arm's latency, because waiting
    /// on a provider is not the arm being slow.
    pub fn parked_secs(&self, bucket: &Bucket, now: u64) -> u64;
}
```

## Required behaviour

- `state` returns `Available` once `now >= until`, without needing `due` to be called first.
- Backoff doubles per consecutive park and saturates at 86400 seconds; it never overflows.
- `succeeded` resets `attempts` to zero so one success un-does the backoff.
- `due` returns each expired bucket exactly once; calling it twice does not re-deliver.
- Parking a bucket that is already parked extends it and increments `attempts` rather than
  creating a second entry.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No overflow on any input, including `now` far in the future and a `reset_at` in the past.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a `retry after 30` in the output yields
`reset_at = now + 30`; a limit with no stated reset falls back to the window; backoff doubles
then saturates at 86400; `succeeded` resets it; `due` delivers once; two arms sharing a bucket
both park when either hits the limit.

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
