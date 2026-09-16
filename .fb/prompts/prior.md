<!-- fb:creates crates/farmerbob-core/src/prior.rs -->
# Task: seed the bandit from an external prior, weighted by evidence

Create `crates/farmerbob-core/src/prior.rs`, declare it from `lib.rs` with
`pub mod prior;`. Change nothing else except that one `pub mod` line.

## Context

farmerbob routes tasks to implementers with a Beta-Bernoulli bandit. Starting every arm at
`alpha = beta = 1` means the first few runs dominate the posterior, and at n = 1..5 per arm
this project's own numbers are noise — one unlucky dispatch can bury a good model for a
long time.

An external prior fixes the cold start, but it must not become permanent: a prior is a
belief held *before* evidence, and it has to yield to evidence at a defensible rate. The
whole design question here is **how fast** a prior is allowed to be overruled, and the
answer must be a parameter, not a constant buried in a function.

Note on cost: arms already carry real prices in dollars per million tokens. Do **not**
introduce a tier enum that restates them. A discrete tier thrown over a continuous quantity
loses the ordering inside each tier and goes stale the moment a price changes. Rank on the
number.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// A prior belief about one arm, from a source outside this project.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorBelief {
    pub arm: String,
    /// Expected success rate, 0.0..=1.0.
    pub capability: f64,
    /// How much evidence this belief is worth, in pseudo-observations.
    /// 2.0 means "treat me as two prior runs" — easily overruled. 50.0 means the
    /// arm needs a long losing streak to fall.
    pub strength: f64,
    /// USD per million input tokens. 0.0 for a free route.
    pub price_in: f64,
    /// USD per million output tokens. 0.0 for a free route.
    pub price_out: f64,
}

/// Beta posterior for one arm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Posterior { pub alpha: f64, pub beta: f64 }

impl Posterior {
    pub fn mean(&self) -> f64;
    /// Total evidence behind this posterior, prior included.
    pub fn observations(&self) -> f64;
}

/// Turn a belief into the posterior an arm starts life with.
///
/// Errors on a capability outside 0.0..=1.0, a non-finite or negative strength,
/// or a negative price.
pub fn seed(b: &PriorBelief) -> Result<Posterior, String>;

/// Fold observed outcomes into a seeded posterior.
pub fn update(p: Posterior, successes: u32, failures: u32) -> Posterior;

/// How far the posterior has moved from its prior, 0.0 at the seed and approaching
/// 1.0 as observations accumulate. This is what tells a planning agent whether it is
/// looking at a measurement or at someone's opinion.
pub fn evidence_weight(seeded: Posterior, current: Posterior) -> f64;

/// Arms on the cost/capability Pareto frontier: no other arm is both cheaper and at
/// least as capable. Cost is `price_in + price_out` per million tokens.
///
/// Returned cheapest first. This is the leaderboard the project exists to produce.
pub fn pareto_frontier(arms: &[(String, f64, Posterior)]) -> Vec<String>;
```

## Required behaviour

- `seed` maps capability and strength to `alpha = capability * strength` and
  `beta = (1 - capability) * strength`, then adds the uniform `Beta(1, 1)` so that a
  zero-strength belief degrades exactly to an unseeded arm rather than to an invalid one.
- A capability of exactly 0.0 or 1.0 is legal and must not produce a zero parameter: an
  arm believed certain must still be movable by evidence.
- `update` never lets either parameter fall, and saturates rather than overflowing.
- `evidence_weight` is 0.0 when `current == seeded`, strictly increasing in observations,
  and never exceeds 1.0.
- `pareto_frontier` breaks a cost tie in favour of the higher posterior mean, and includes
  a free arm (cost 0.0) whenever it is not strictly dominated.
- An arm with fewer observations than another is not dominated on capability alone —
  compare posterior means, and treat a difference smaller than `1e-9` as a tie rather than
  letting float noise decide the ranking.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No NaN may escape any function. Reject non-finite inputs at the boundary.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: zero strength degrades to `Beta(1,1)`; capability
1.0 still yields a finite movable beta; a long losing streak overrules a strong prior;
`evidence_weight` starts at zero and rises; a NaN capability is rejected; a free arm appears
on the frontier; a dominated arm does not; a cost tie is broken by mean.

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
