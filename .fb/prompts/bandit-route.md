<!-- fb:creates crates/farmerbob-core/src/router.rs -->
# Task: implement the implementer router

Create `crates/farmerbob-core/src/router.rs`, declare it from `lib.rs` with `pub mod router;`.
Change nothing else. `attempt_log.rs` already defines `Posterior` and `OutcomeClass` — read it
and reuse them.

## Context

farmerbob picks which agent attempts a task. Two things make this harder than argmax.

**Eligibility is not capability.** An arm whose provider is rate-limited, or which cannot do
multi-turn work, or which is a paid route to a model already available free, must not be
selected — and none of that is evidence about how good it is. Eligibility is a pre-dispatch
filter; the posterior is untouched by it.

**Exploration must be cheap, not free.** An arm with no history might be excellent. Thompson
sampling handles that: draw from each arm's posterior and take the best draw, so uncertain
arms win sometimes and consistently-good arms win usually.

**Pure logic: no I/O, no clock, no randomness of its own.** Random draws are supplied by the
caller, which is what makes the policy testable.

## Exact API — implement these signatures verbatim

```rust
use crate::attempt_log::Posterior;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ArmName(pub String);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Ineligible {
    Parked { until: u64 },
    MissingCapability { needed: String },
    RedundantPaidRoute { free_arm: String },
    Disabled { reason: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArmInfo {
    pub name: ArmName,
    pub posterior: Posterior,
    /// Dollars per 1M input tokens; zero for plan-based arms.
    pub price_in: f64,
    /// Observed mean latency in seconds. `None` when never measured.
    pub mean_latency_s: Option<f64>,
    /// Capabilities this arm has, e.g. "multiturn".
    pub capabilities: Vec<String>,
    pub parked_until: Option<u64>,
    pub redundant_with: Option<String>,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Weights {
    pub task_value: f64,
    pub dollar_weight: f64,
    pub latency_weight: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Choice { pub arm: ArmName, pub score: f64, pub sampled_p: f64 }

/// Why each excluded arm was excluded. Eligibility is a filter, never a score.
pub fn eligibility(
    arm: &ArmInfo,
    needed: &[String],
    now: u64,
    healthy_free_arms: &[String],
) -> Option<Ineligible>;

/// Score one arm given an already-drawn success probability.
///   score = task_value * p - dollar_weight * price_in - latency_weight * latency
/// An unmeasured latency is treated as the worst observed among `all`, never as zero.
pub fn score(arm: &ArmInfo, sampled_p: f64, w: &Weights, all: &[ArmInfo]) -> f64;

/// Pick. `draws` supplies one sample per eligible arm, in the order they appear in `arms`
/// after filtering. Returns `None` when nothing is eligible.
pub fn select(
    arms: &[ArmInfo],
    needed: &[String],
    now: u64,
    healthy_free_arms: &[String],
    w: &Weights,
    draws: &[f64],
) -> Option<Choice>;
```

## Required behaviour

- A parked arm is ineligible until `now >= until`, and parking never changes its posterior.
- A paid arm with `redundant_with` pointing at an arm in `healthy_free_arms` is ineligible.
- An arm missing a needed capability is ineligible.
- `score` treats `mean_latency_s == None` as the maximum latency among `all`, so an unmeasured
  arm is never flattered by an absence of data.
- `select` is deterministic given the same `draws`.
- Fewer `draws` than eligible arms is not a panic: score only as many as there are draws.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No NaN escaping `score`, including an empty `all` or all-zero weights.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a parked arm is skipped and its posterior unchanged;
a paid duplicate is skipped while its free twin is healthy but selected once the twin is
unhealthy; an unmeasured latency ranks no better than the worst measured; identical draws give
identical selections; `select` with an empty roster returns `None` rather than panicking.

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
