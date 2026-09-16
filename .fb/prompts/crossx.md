<!-- fb:creates crates/farmerbob-core/src/crossx.rs -->
# Task: cross-examination matrix

Create `crates/farmerbob-core/src/crossx.rs`, declare it from `lib.rs` with
`pub mod crossx;`. Change nothing else except that one `pub mod` line.

## Context

When N agents attack one task you get N implementations **and N test suites**. Run every
suite against every implementation: N² cheap local executions, no tokens, no model opinions.
The resulting matrix carries a consensus correctness signal that needs no judge.

Read a column (one implementation, all suites) and a row (one suite, all implementations):

- a test that passes on 9/10 implementations and fails on 1 → strong evidence of a real
  **bug in that one**
- a test that fails on 9/10 → strong evidence the **test is wrong**, or encodes an
  assumption the spec never stated
- a test that passes on 10/10 → carries no discriminating information

The hard part is that both readings explain any single failure. Only the distribution
separates them, so this module's job is to turn a matrix into calibrated evidence and to
**refuse to conclude** when the matrix is too small to support a conclusion.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// Outcome of running one suite against one implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell { Pass, Fail, Error }

/// A square-ish matrix: suites (rows) by implementations (columns).
/// Suites and implementations are named; they need not be the same set.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    suites: Vec<String>,
    impls: Vec<String>,
    cells: Vec<Cell>,
}

impl Matrix {
    /// `cells` is row-major: suite 0 against every impl, then suite 1, ...
    /// Errors if the length does not equal suites.len() * impls.len(), or if either
    /// axis contains a duplicate name.
    pub fn new(suites: Vec<String>, impls: Vec<String>, cells: Vec<Cell>) -> Result<Self, String>;
    pub fn get(&self, suite: &str, imp: &str) -> Option<Cell>;
    pub fn suites(&self) -> &[String];
    pub fn impls(&self) -> &[String];
}

/// What the matrix says about one (suite, implementation) failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// This suite fails here and passes nearly everywhere else.
    LikelyDefect { suite: String, imp: String, passed_elsewhere: usize, total_elsewhere: usize },
    /// This suite fails nearly everywhere.
    LikelySuiteFault { suite: String, failed: usize, total: usize },
    /// Too few implementations, or a split too even, to distinguish the two.
    Inconclusive { suite: String, imp: String, reason: String },
}

/// Read every failing cell. A cell is only reported once, under its strongest reading.
///
/// `min_impls` is the smallest number of implementations for which any conclusion is
/// allowed; below it every failure is `Inconclusive`. A caller passing 1 or 2 is asking
/// for noise, and this function must not oblige.
pub fn read(m: &Matrix, min_impls: usize) -> Vec<Reading>;

/// Fraction of cells that are `Pass`, over cells that are not `Error`.
/// `None` when every cell errored: no denominator, and 0.0 would be a lie.
pub fn agreement(m: &Matrix) -> Option<f64>;

/// Suites whose row is identical across every implementation — they discriminate nothing.
/// These are the tests worth deleting, and the ones worth telling an author about.
pub fn non_discriminating(m: &Matrix) -> Vec<String>;
```

## Required behaviour

- `Error` cells are excluded from every denominator, never counted as `Fail`. A suite that
  failed to compile against an implementation is a missing measurement, not a defect.
- A failure is `LikelyDefect` when the suite passes on a strict majority of the *other*
  implementations it was measured against; `LikelySuiteFault` when it fails on a strict
  majority; `Inconclusive` on an exact tie or when fewer than `min_impls` implementations
  are present.
- `LikelySuiteFault` is reported once per suite, not once per failing cell.
- A suite excluded from a comparison because every other cell errored is `Inconclusive` with
  a reason saying so.
- `non_discriminating` includes all-pass and all-fail rows alike; both are uninformative.
- Self-pairs — a suite run against the implementation it shipped with — are included in the
  matrix but must be **excluded** from the `passed_elsewhere` / `failed` counts. An author's
  own suite passing on their own code is not evidence about anyone.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.
- Treat a suite and an implementation as the same author when their names are equal.

## Tests you must write

One per rule, named after it. At minimum: a 1-of-10 failure reads as `LikelyDefect`; a
9-of-10 failure reads as `LikelySuiteFault` and appears once; an even split is
`Inconclusive`; `min_impls` suppresses conclusions on a 2-column matrix; `Error` cells do
not count as failures; `agreement` is `None` on an all-error matrix; a self-pair does not
inflate `passed_elsewhere`; an all-pass row is non-discriminating.

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
