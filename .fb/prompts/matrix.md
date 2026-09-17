<!-- fb:creates crates/farmerbob-core/src/matrix.rs -->
# Task: reading a cross-examination matrix

Create `crates/farmerbob-core/src/matrix.rs`, declare it from `lib.rs` with
`pub mod matrix;`. Change nothing else except that one `pub mod` line.

## Context

N candidates attack one task, so you get N implementations **and** N test suites. Run every
suite against every implementation and the N×N result is the cheapest objective signal this
project has — no tokens, no model opinions, just executions.

Reading it is the hard part, and four distinct shapes have been observed in practice. The
harness must name them, because three of them look like the fourth if you only count:

- **A defect.** One suite fails on one implementation and passes on the rest. Strong
  evidence of a real bug in that one.
- **An over-fitted suite.** One suite fails nearly everywhere. Evidence about the suite, not
  its targets: it encodes its author's implementation rather than the spec. Its failures
  carry no information and its test count is worthless as a measure of depth.
- **A specification ambiguity.** The field partitions into camps that pass within themselves
  and fail across. Neither camp is wrong; the spec admitted two readings. Reported as
  four discriminating suites, this looks like every candidate finding two defects when none
  found any.
- **Consensus.** Everything passes. Nothing discriminates, and that is real information: the
  authors agree about the contract.

The diagonal is the invariant that makes the rest trustworthy. Every arm's own suite passed
against its own implementation in its own worktree, so a diagonal failure is a transplant
error and the whole matrix is void. Twice this harness reported a confident "no signal" from
a matrix whose diagonal was broken.

**Pure logic: no process execution.** The caller runs the cells.

## Exact API — implement these signatures verbatim

```rust
/// Outcome of running one suite against one implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell { Pass, Fail, NoCompile }

/// Square matrix over one candidate set. Row = implementation, column = suite.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    arms: Vec<String>,
    cells: Vec<Cell>,
}

impl Matrix {
    /// `cells` is row-major: implementation 0 against every suite, then implementation 1...
    /// Errors on a length mismatch or a duplicate arm name.
    pub fn new(arms: Vec<String>, cells: Vec<Cell>) -> Result<Self, String>;
    pub fn get(&self, implementation: &str, suite: &str) -> Option<Cell>;
    pub fn arms(&self) -> &[String];
}

/// What the matrix as a whole says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// The diagonal is not all-Pass. Nothing else may be read.
    Void { broken: Vec<String> },
    /// Every cell passes.
    Consensus,
    /// Camps that pass within themselves and fail across. Names each camp, sorted.
    SpecAmbiguous { camps: Vec<Vec<String>> },
    /// Ordinary: some suites discriminate, some are over-fitted.
    Discriminating,
}

/// Classify. `Void` is checked first and suppresses every other reading.
pub fn shape(m: &Matrix) -> Shape;

/// How one arm's SUITE behaved, judged across the whole matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuiteQuality { Discriminating, OverFitted, Uninformative }

/// Classify one arm's suite. Self-pairs are excluded from every count: an author's suite
/// passing on its own code says nothing about anyone.
///
/// OverFitted when it fails on a strict majority of the others; Uninformative when it fails
/// on none; Discriminating otherwise.
pub fn suite_quality(m: &Matrix, arm: &str) -> Option<SuiteQuality>;

/// Fraction of OTHER arms' discriminating suites this implementation passes.
/// `None` when no other arm has a discriminating suite -- there is nothing to survive.
pub fn survival(m: &Matrix, arm: &str) -> Option<f64>;

/// Implementations this arm's suite breaks, excluding itself, sorted.
pub fn discoveries(m: &Matrix, arm: &str) -> Vec<String>;
```

## Required behaviour

- `shape` returns `Void` when any arm's self-cell is not `Pass`, naming every such arm.
  Checked before anything else.
- `SpecAmbiguous` requires at least 4 arms and at least 2 camps, where every member of a
  camp passes every suite in its own camp and fails every suite outside it. With 2 or 3 arms
  a partition is indistinguishable from ordinary disagreement and must NOT be reported.
- `NoCompile` is excluded from every numerator and denominator. A suite that could not be
  built against an implementation is a missing measurement, not a failure.
- `suite_quality` and `survival` return `None` for an arm not in the matrix.
- `survival` counts only suites whose `suite_quality` is `Discriminating`.
- Self-pairs are excluded everywhere.
- `Consensus` requires every non-self cell to be `Pass`; a single `NoCompile` makes it
  `Discriminating`, not `Consensus`, because something went unmeasured.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No NaN may escape.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a broken diagonal voids everything; a 2x2
partition is NOT reported as SpecAmbiguous; a 4-arm partition IS; a suite failing 1 of 4 is
Discriminating and failing 3 of 4 is OverFitted; NoCompile is not a failure; a self-pair
does not inflate discoveries; survival is None when no rival suite discriminates; one
NoCompile prevents Consensus.

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
4. **Test depth and generality** — measured directly where possible, by injecting known
   defects and by running your suite against rival implementations. Where neither
   measurement could be taken, the number of distinct behaviours you covered stands in for
   it. Count is the fallback, not the target:
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
