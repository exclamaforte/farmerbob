<!-- fb:creates crates/farmerbob-core/src/cost.rs -->
# Task: measured cost attribution and the cost/capability frontier

Create `crates/farmerbob-core/src/cost.rs`, declare it from `lib.rs` with
`pub mod cost;`. Change nothing else except that one `pub mod` line.

## Context

The number this project exists to produce: **given a task you actually need done, which
arms can do it, and what do they cost?** Everything else is instrumentation for this table.

Cost must be MEASURED, not estimated from a price list. A price list says what a token
costs; it cannot say how many tokens an arm spent failing. One arm on this project cost
$1.51 across three runs and completed none of them, while the free route to the SAME
underlying model completed its task — an estimate from published prices would have ranked
them identically.

Two traps the type system has to close:

- **A plan-based arm is genuinely free.** `Some(0.0)` is a measurement; `None` is an
  absent one. Collapsing them makes an unmeasured expensive arm look free.
- **Cost per success is undefined with no successes.** Dividing spend by zero completions
  and reporting infinity, or reporting the raw spend, both rank a total failure as if it
  were merely expensive.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// What one run cost and whether it counted.
#[derive(Debug, Clone, PartialEq)]
pub struct RunCost {
    pub arm: String,
    pub task: String,
    /// Measured USD. `Some(0.0)` for a plan-based arm is a real zero; `None` is unmeasured.
    pub usd: Option<f64>,
    pub tokens: Option<u64>,
    /// Did the run produce a passing deliverable?
    pub completed: bool,
    /// False when the outcome says nothing about the arm: quota block, harness bug,
    /// invalid task, orchestrator cancellation. Excluded from rates AND from spend.
    pub counts_for_arm: bool,
}

/// One arm's aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmCost {
    pub arm: String,
    pub runs: u32,
    pub completed: u32,
    /// Summed measured spend. `None` when NO run for this arm was measured.
    pub usd: Option<f64>,
    pub tokens: Option<u64>,
}

impl ArmCost {
    /// Completions over counted runs. `None` when no run counted.
    pub fn completion_rate(&self) -> Option<f64>;
    /// Spend per completion. `None` when spend is unmeasured OR completions are zero --
    /// never infinity, and never the raw spend.
    pub fn usd_per_completion(&self) -> Option<f64>;
}

/// Aggregate per arm. Runs with `counts_for_arm == false` are excluded entirely.
/// Returned sorted by arm name so the table is stable.
pub fn aggregate(runs: &[RunCost]) -> Vec<ArmCost>;

/// The cost/capability frontier: arms no other arm dominates.
///
/// A dominates B when A costs no more per completion AND completes at no lower a rate,
/// and is strictly better on at least one. An arm with no completions is never on the
/// frontier. Returned cheapest first, ties broken by higher completion rate then by name.
pub fn frontier(arms: &[ArmCost], epsilon: f64) -> Vec<String>;

/// Total measured spend, and what it bought. `None` spend contributes nothing.
pub fn totals(arms: &[ArmCost]) -> (f64, u32, u32);
```

## Required behaviour

- `Some(0.0)` and `None` are never conflated anywhere: a free arm has a measured spend of
  zero and appears on the frontier; an unmeasured arm has `usd: None` and cannot.
- An arm whose runs are all `counts_for_arm == false` appears with `runs: 0` and a
  `completion_rate` of `None`, not a rate of 0.0.
- `usd_per_completion` is `None` with zero completions. Ranking a total failure as merely
  expensive is the specific error this prevents.
- `aggregate` sums only measured costs; one unmeasured run among measured ones does not
  poison the sum to `None`.
- Differences below `epsilon` are ties; float noise must never decide the frontier.
- A free arm (0.0) is on the frontier unless another free arm strictly out-completes it.
- `totals` returns (spend, completions, counted runs) and ignores unmeasured spend.
- No NaN or infinity escapes any function.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a measured zero is on the frontier and an
unmeasured arm is not; zero completions gives `None` rather than infinity; an all-excluded
arm has rate `None` not 0.0; one unmeasured run does not poison a sum; two free arms are
ordered by completion rate; a dominated arm is absent; epsilon absorbs float noise.

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
