<!-- fb:creates crates/farmerbob-core/src/sensitivity.rs -->
# Task: defect sensitivity — measuring whether a candidate's TESTS are good

Create `crates/farmerbob-core/src/sensitivity.rs`, declare it from `lib.rs` with
`pub mod sensitivity;`. Change nothing else except that one `pub mod` line.

## Context

An implementation's test suite is part of the deliverable, and its quality is measurable
without any judge: inject known defects into a reference implementation, run the candidate's
frozen suite against each mutant, and count how many it catches.

The subtlety is the **denominator**. A mutation that nothing detects is either inert — it
changed no observable behaviour — or it changed behaviour the specification is silent about.
Neither is a miss on the candidate's part, so both must be excluded from the denominator
rather than counted against every suite equally. A defect is admitted to the corpus only if
the reference conformance suite catches it; that is what makes it a defect rather than a
refactor.

The second subtlety: a suite that fails on the *unmutated* reference is broken, and its
"detections" are meaningless — it would report every mutant as caught while detecting
nothing. Such a suite must be disqualified, not credited with perfect sensitivity.

**Pure logic: no I/O.** Execution happens elsewhere; this module folds the results.

## Exact API — implement these signatures verbatim

```rust
/// A candidate defect, injected into the reference implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defect { pub id: String, pub description: String }

/// Whether one suite caught one defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detection { Caught, Missed, DidNotRun }

/// One suite's complete result set.
#[derive(Debug, Clone, PartialEq)]
pub struct SuiteRun {
    pub suite: String,
    /// Did this suite pass against the UNMUTATED reference? If not, it is disqualified.
    pub baseline_pass: bool,
    /// One entry per defect, by defect id.
    pub detections: Vec<(String, Detection)>,
}

/// A defect no admitted suite caught is excluded from every denominator.
#[derive(Debug, Clone, PartialEq)]
pub struct Corpus { /* your fields */ }

impl Corpus {
    /// Errors on a duplicate defect id, or a `SuiteRun` naming a defect not in `defects`.
    pub fn new(defects: Vec<Defect>, runs: Vec<SuiteRun>) -> Result<Self, String>;
    /// Defects at least one admitted suite caught. These form the denominator.
    pub fn live_defects(&self) -> Vec<String>;
    /// Defects no admitted suite caught: inert, or the spec is silent. Excluded, and
    /// reported so a human can decide which.
    pub fn inert_defects(&self) -> Vec<String>;
    /// Suites disqualified for failing the baseline.
    pub fn disqualified(&self) -> Vec<String>;
}

/// Fraction of live defects this suite caught. `None` for a disqualified suite, and
/// `None` when there are no live defects — a corpus that discriminates nothing cannot
/// rank anyone, and 0.0 or 1.0 would both be fabrications.
pub fn sensitivity(c: &Corpus, suite: &str) -> Option<f64>;

/// Defects caught by exactly one suite. These are the most valuable rows in the corpus:
/// the places where one author tested something no one else thought of.
pub fn unique_catches(c: &Corpus) -> Vec<(String, String)>;

/// Suites ranked by sensitivity, highest first, disqualified suites omitted.
/// Ties are broken by suite name so the ranking is deterministic.
pub fn rank(c: &Corpus) -> Vec<(String, f64)>;
```

## Required behaviour

- A suite with `baseline_pass == false` is disqualified: it contributes to no numerator and
  no denominator, cannot make a defect live, and never appears in `rank`.
- `DidNotRun` is neither a catch nor a miss. It shrinks that suite's denominator only —
  a suite that could not be executed against one mutant is unmeasured there, not wrong.
- A defect is live if **any admitted** suite caught it. A defect only "caught" by a
  disqualified suite is inert.
- `sensitivity` returns `None`, never 0.0, when the suite is disqualified or no live
  defects exist. An absent measurement and a measured zero must be distinguishable.
- `unique_catches` counts only admitted suites, and returns `(defect_id, suite)` pairs
  sorted by defect id.
- `rank` is deterministic under any input ordering.
- `Corpus::new` rejects a run referring to an unknown defect id rather than ignoring it:
  silently dropping it would quietly shrink a denominator.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No division by zero, and no NaN may escape any function.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a baseline-failing suite is disqualified and
scores `None`; a defect only caught by a disqualified suite is inert; `DidNotRun` shrinks
only its own denominator; an empty live set gives `None` not 0.0; a genuine zero-catch
admitted suite gives `Some(0.0)`; `unique_catches` finds the sole catcher; `rank` is
deterministic when two suites tie; an unknown defect id is rejected.

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
