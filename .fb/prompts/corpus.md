<!-- fb:creates crates/farmerbob-core/src/corpus.rs -->
# Task: a defect corpus harvested from the orchestrator's own history

Create `crates/farmerbob-core/src/corpus.rs`, declare it from `lib.rs` with
`pub mod corpus;`. Change nothing else except that one `pub mod` line.

## Context

Defect sensitivity needs defects, and synthetic mutations are weak: a hand-written mutant
tends to break something every suite already checks. The interesting defects are the ones
that actually happened — a scoring gate that passed an empty test suite, a confinement
wrapper silently dropped in a refactor, a memory metric that read another process's cgroup.
Each of those shipped, survived review, and was caught later by something other than tests.

So the corpus is harvested, not invented. Every entry carries the evidence that it was real
and the check that would have caught it, and an entry that cannot state both is not admitted.

The admission rule that matters: **a defect is only admitted if some suite detects it.** A
mutation nothing detects is inert, or the specification is silent about it. Neither is a
miss on any candidate's part, so both leave the denominator rather than counting against
everyone equally. A corpus that quietly inflates its denominator makes every arm look worse
and makes the differences between them smaller.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// How a defect was found, in descending order of how much it says about tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FoundBy {
    /// A test failed. The cheapest possible discovery.
    Test,
    /// Cross-examination: another candidate's suite caught it.
    CrossExamination,
    /// A reviewer reading the patch.
    Review,
    /// Only noticed in production behaviour. The most expensive, and the most valuable
    /// to add a test for.
    Observation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defect {
    pub id: String,
    /// What went wrong, in one line.
    pub summary: String,
    /// Where it lived.
    pub module: String,
    pub found_by: FoundBy,
    /// The assertion that would have caught it. Empty means the entry is not admissible.
    pub detecting_assertion: String,
    /// Evidence it really happened: a commit, a bead id, a log line.
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inadmissible {
    /// No assertion would catch it: inert, or the spec is silent.
    NoDetectingAssertion,
    /// No evidence it occurred. An invented defect is a mutation, not a harvest.
    NoEvidence,
    Duplicate { of: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Corpus { /* your fields */ }

impl Corpus {
    pub fn new() -> Self;
    /// Admit or reject. Rejection carries a reason; a rejected entry is never stored.
    pub fn admit(&mut self, d: Defect) -> Result<(), Inadmissible>;
    /// Admitted defects, ordered by id.
    pub fn defects(&self) -> Vec<&Defect>;
    /// Defects in one module.
    pub fn by_module(&self, module: &str) -> Vec<&Defect>;
    /// Count per discovery route. The shape of this histogram says whether the test
    /// suites are carrying their weight or whether review is doing their job.
    pub fn histogram(&self) -> Vec<(FoundBy, u32)>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

/// Two defects are the same when they name the same module and the same detecting
/// assertion, whatever their ids or wording. Duplicates inflate a denominator.
pub fn same_defect(a: &Defect, b: &Defect) -> bool;

/// Modules with defects that only `Observation` or `Review` found -- nothing tested them.
/// This is the list of places to write tests next, ranked by count descending.
pub fn untested_modules(c: &Corpus) -> Vec<(String, u32)>;
```

## Required behaviour

- An empty or whitespace-only `detecting_assertion` is `NoDetectingAssertion`; an empty
  `evidence` is `NoEvidence`. Check the assertion first when both are missing.
- `same_defect` compares module and detecting assertion, ignoring case and surrounding
  whitespace, and ignores `id`, `summary`, `evidence` and `found_by`.
- `admit` rejects a duplicate naming the id it duplicates, and keeps the FIRST.
- `histogram` includes only routes that occur, ordered by the `FoundBy` ordering, and its
  counts sum to `len()`.
- `untested_modules` counts only defects found by `Review` or `Observation`, and omits a
  module entirely once any of its defects was found by `Test` or `CrossExamination`.
- Ties in `untested_modules` are broken by module name, so the ranking is deterministic.
- `defects()` is ordered by id and is stable regardless of admission order.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a missing assertion is rejected before a missing
evidence; two differently-worded entries for one assertion are duplicates; the first is
kept and the rejection names it; the histogram sums to len; a module with any tested defect
is absent from `untested_modules`; ties are broken by name; `defects()` order does not
depend on admission order.

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
