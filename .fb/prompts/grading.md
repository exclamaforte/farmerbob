<!-- fb:creates crates/farmerbob-core/src/grading.rs -->
# Task: recording a grade, and knowing when a grade is worth nothing

Create `crates/farmerbob-core/src/grading.rs`, declare it from `lib.rs` with
`pub mod grading;`. Change nothing else except that one `pub mod` line.

## Context

A grade is the orchestrator's judgement about one candidate, recorded so the bandit can
learn from it. The danger is that a grade looks authoritative whatever produced it, so this
module's real job is to keep the *provenance* attached and to refuse to average things that
should not be averaged.

Two facts this project learned the hard way:

- A grade derived from a broken instrument is worse than no grade. A conformance suite once
  indicted eleven candidates at once; a cross-examination matrix has twice reported a
  confident tie while measuring nothing. So a grade records which evidence it rests on, and
  evidence can be *retracted*, which must retract the grades built on it.
- Most fields tie. A grading scheme that always produces an ordering manufactures one from
  noise.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// Where a grade's authority comes from. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// A frozen conformance suite executed against the candidate.
    Conformance,
    /// Cross-examination against rival suites.
    CrossExam,
    /// A confirmed, veto-surviving claim from a critic.
    ProvenClaim,
    /// The orchestrator's own reading. Weakest, and never sufficient alone.
    Judgement,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grade {
    pub arm: String,
    pub task: String,
    /// 0.0..=1.0. Rejected outside that range.
    pub score: f64,
    pub basis: Basis,
    /// Identifier of the evidence this rests on, e.g. a suite name or claim id.
    /// Empty is rejected: a grade with no traceable evidence cannot be retracted.
    pub evidence: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    ScoreOutOfRange,
    NotFinite,
    NoEvidence,
    /// A Judgement grade submitted with no corroborating grade of a stronger basis.
    UnsupportedJudgement,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger { /* your fields */ }

impl Ledger {
    pub fn new() -> Self;
    /// Record a grade. A Judgement is accepted only when this ledger already holds a
    /// grade for the same (arm, task) whose basis is stronger than Judgement.
    pub fn record(&mut self, g: Grade) -> Result<(), Invalid>;
    /// Retract every grade resting on this evidence id, returning how many went.
    pub fn retract(&mut self, evidence: &str) -> usize;
    /// All live grades for one candidate, newest first.
    pub fn grades(&self, arm: &str, task: &str) -> Vec<&Grade>;
    /// The grade that should be believed: the one with the strongest basis, and among
    /// equals the newest. `None` when there are none.
    pub fn authoritative(&self, arm: &str, task: &str) -> Option<&Grade>;
}

/// Mean score weighted by basis strength, where Conformance counts 4, CrossExam 3,
/// ProvenClaim 2 and Judgement 1. `None` for an empty set -- not 0.0.
pub fn weighted(grades: &[&Grade]) -> Option<f64>;

/// True when no grade separates the candidates: every arm's authoritative score is within
/// `epsilon`, or some arm has none at all. An orchestrator must be able to learn that its
/// evidence does not decide.
pub fn is_undecided(l: &Ledger, task: &str, arms: &[String], epsilon: f64) -> bool;
```

## Required behaviour

- `Basis` orders Conformance > CrossExam > ProvenClaim > Judgement. The derived `Ord` must
  reflect that; if the declaration order gives the opposite, do not silently rely on it.
- `record` rejects a score outside `0.0..=1.0`, a non-finite score, and empty or
  whitespace-only evidence. Report in that order so the error is deterministic.
- An `UnsupportedJudgement` is rejected and NOT stored. Check it last, after the field
  validations.
- `retract` removes grades by exact evidence id, returns the count, and leaves everything
  else untouched. Retracting evidence that supported the only non-Judgement grade may leave
  a stored Judgement unsupported; that is allowed — retraction does not cascade — but
  `authoritative` must then return the Judgement, since it is what remains.
- `authoritative` breaks a basis tie by the newest `at_ms`, and an `at_ms` tie by keeping
  the first recorded.
- `weighted` returns `None` for an empty slice, never 0.0, and no NaN escapes.
- `is_undecided` is true when any named arm has no authoritative grade.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a bare Judgement is rejected; a Judgement after a
Conformance grade is accepted; validation order is deterministic when two rules break at
once; retract returns the count and does not cascade; authoritative prefers basis over
recency; weighted is None on empty; an arm with no grade makes the field undecided.

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
