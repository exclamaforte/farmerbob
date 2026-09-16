<!-- fb:creates crates/farmerbob-core/src/attempt_log.rs -->
# Task: implement the append-only attempt log

Create `crates/farmerbob-core/src/attempt_log.rs`, declare it from `lib.rs` with
`pub mod attempt_log;`. Change nothing else.

## Context

farmerbob routes work to agents and learns which are good at what. The statistics it learns
from are a **derived view**, never the source of truth: the definition of success will change,
judges get recalibrated, acceptance criteria get tightened. If posteriors are built from
stored scores, recalibrating means every historical number is silently wrong.

So this module stores raw evidence and recomputes. It is pure data plus aggregation — no I/O,
no clock.

The distinction that does the most work here: **most failures are not the arm's fault.** In
one measured round, nine of eleven no-results were an invalidated task, an adapter that could
not do the work, an upstream rate limit caused by our own dispatch burst, a stale config, a
harness bug, and a run the orchestrator itself killed. Counting those against an arm teaches
the router to avoid arms for things that were done *to* them.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OutcomeClass {
    ArmResult,
    Infrastructure,
    OrchestratorCancelled,
    TaskInvalid,
    HarnessBug,
    Unknown,
}

impl OutcomeClass {
    /// Only `ArmResult` may update an arm's success statistics.
    pub fn counts_for_posterior(self) -> bool;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Attempt {
    pub attempt_id: String,
    pub arm: String,
    /// Bucket the router used, e.g. "feature:routine".
    pub bucket: String,
    pub outcome: OutcomeClass,
    /// Whether the arm's work was accepted. Meaningless unless `outcome` is `ArmResult`.
    pub accepted: Option<bool>,
    pub cost_usd: f64,
    pub latency_s: u64,
    /// 1 for a first attempt; higher for a retry from a failing state.
    pub attempt_number: u32,
    /// Version of the scoring suite this was judged against. Attempts judged by different
    /// suite versions are not comparable.
    pub suite_version: u32,
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Posterior { pub alpha: f64, pub beta: f64 }

impl Posterior {
    /// Mean of the Beta distribution.
    pub fn mean(&self) -> f64;
    /// Observations behind it, i.e. alpha + beta - 2 given a Beta(1,1) start.
    pub fn n(&self) -> f64;
}

#[derive(Debug, Default)]
pub struct AttemptLog { /* your fields */ }

impl AttemptLog {
    pub fn new() -> Self;
    /// Append. The log is never mutated in place.
    pub fn record(&mut self, a: Attempt);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Recompute a posterior for one arm in one bucket, from raw evidence.
    ///
    /// - only `ArmResult` attempts contribute
    /// - only `attempt_number == 1`; a retry from a failing patch is an easier problem and
    ///   corrupts a first-attempt posterior
    /// - only attempts whose `suite_version` equals `suite_version`
    /// - `accepted == Some(true)` adds to alpha, `Some(false)` to beta, `None` is skipped
    /// - starts from Beta(1,1)
    pub fn posterior(&self, arm: &str, bucket: &str, suite_version: u32) -> Posterior;

    /// Total measured spend for an arm, across every attempt including excluded ones --
    /// a run that failed for infrastructure reasons still cost money.
    pub fn spend(&self, arm: &str) -> f64;

    /// Mean latency over contributing attempts only. `None` when there are none.
    pub fn mean_latency(&self, arm: &str, bucket: &str) -> Option<f64>;

    /// Attempts excluded from the posterior, by class, for one arm.
    pub fn excluded(&self, arm: &str) -> Vec<(OutcomeClass, usize)>;
}
```

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No division by zero and no NaN escaping from any accessor.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: an `Infrastructure` outcome does not move the
posterior but does count toward `spend`; `attempt_number == 2` is excluded; a different
`suite_version` is excluded; `accepted: None` on an `ArmResult` is skipped rather than counted
as failure; `posterior` on an arm with no attempts returns Beta(1,1) with mean 0.5;
`mean_latency` returns `None` rather than dividing by zero.

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
