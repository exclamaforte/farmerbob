<!-- fb:creates crates/farmerbob-core/src/outcome.rs -->
# Task: the outcome record — what actually reaches the posterior

Create `crates/farmerbob-core/src/outcome.rs`, declare it from `lib.rs` with
`pub mod outcome;`. Change nothing else except that one `pub mod` line.

## Context

Every run this project has done produces one record, and the bandit learns from it. The
record therefore has one job above all others: **never let a failure that says nothing about
the arm reach the arm's posterior.**

This is not hypothetical. Runs have been lost to a provider quota block, a stale API key, a
launcher refusing access to the run's own worktree, a task whose deliverable already existed
at the base commit, and a dispatcher deleting another dispatcher's worktree. Every one of
those was recorded as "this model produced nothing", and every one silently defamed a model
for something the harness did.

A second, subtler quantity: **rescue effort**. An arm that passes unaided is not equal to one
that passed after the orchestrator repaired its build. Both are `PASS`. Only one is free.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// Why a run ended. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeClass {
    /// The arm was given a fair chance and this is what it did.
    ArmResult,
    /// Harness, launcher, credential or machine fault.
    Infrastructure,
    /// The provider refused on quota grounds.
    QuotaLimited,
    /// The task was unrunnable as specified, e.g. its deliverable already existed.
    TaskInvalid,
    /// The orchestrator stopped it.
    Cancelled,
    /// Not yet classified. Must be treated as unusable, never as a failure.
    Unknown,
}

impl OutcomeClass {
    /// Only ArmResult reaches a posterior. This is the whole point of the enum.
    pub fn counts_for_posterior(self) -> bool;
}

/// The gate verdict. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict { Pass, NoOp, NoCompile, TestsFail, NoTests }

/// How much orchestrator work a PASS required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rescue {
    /// Merged as delivered.
    None,
    /// Lints or formatting fixed.
    Cosmetic,
    /// Build or tests repaired.
    Repaired,
    /// Substantially rewritten. A PASS here is barely the arm's.
    Rewritten,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub arm: String,
    pub task: String,
    pub verdict: Verdict,
    pub class: OutcomeClass,
    pub rescue: Rescue,
    pub lines: u32,
    pub tests_run: u32,
    /// Measured USD. `Some(0.0)` is a real zero; `None` is unmeasured. Never conflate them.
    pub usd: Option<f64>,
}

impl Outcome {
    /// A success the posterior may learn from: ArmResult, Pass, and rescue at most
    /// `max_rescue`. An orchestrator-repaired pass is not evidence the arm can do it.
    pub fn is_clean_success(&self, max_rescue: Rescue) -> bool;
    /// Self-consistency. A record can be wrong about itself and must say so rather than
    /// be silently believed.
    pub fn validate(&self) -> Result<(), String>;
}

/// Beta update over records, skipping everything that does not count.
/// Returns (alpha_increment, beta_increment).
pub fn posterior_delta(rs: &[Outcome], max_rescue: Rescue) -> (u32, u32);

/// Records whose class is Unknown. These are a backlog to triage, never a failure count:
/// leaving them unclassified quietly deflates the arms that own them.
pub fn needs_triage(rs: &[Outcome]) -> Vec<&Outcome>;

/// Per-arm counts of every class, so a leaderboard can show WHY an arm has few
/// countable runs. Sorted by arm name.
pub fn class_census(rs: &[Outcome]) -> Vec<(String, Vec<(OutcomeClass, u32)>)>;
```

## Required behaviour

- `counts_for_posterior` is true for `ArmResult` and false for every other variant,
  including `Unknown`.
- `validate` rejects: a `Pass` with `tests_run == 0`; a `Pass` with `lines == 0`; a `NoOp`
  with `lines > 0`; a non-`ArmResult` class carrying a rescue above `None`; a negative or
  non-finite `usd`. Report the FIRST failure in that order, so the message is deterministic.
- `is_clean_success` requires all three of: class `ArmResult`, verdict `Pass`, and
  `rescue <= max_rescue`.
- `posterior_delta` counts a clean success as alpha and an `ArmResult` non-pass as beta.
  A record that does not count for the posterior increments neither.
- An `ArmResult` `Pass` whose rescue exceeds `max_rescue` increments NEITHER. It is not a
  failure of the arm and it is not a success the arm earned.
- `class_census` omits classes with a zero count and orders them as declared above.
- `needs_triage` preserves input order.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No NaN may escape, and counts must saturate rather than overflow.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: Unknown does not count for the posterior; a Pass
with zero tests fails validation; validation order is deterministic when two rules are
violated at once; a Repaired pass above max_rescue increments neither alpha nor beta; a
quota-limited run is absent from both; `Some(0.0)` validates and `None` validates; the
census omits zero counts; `needs_triage` keeps input order.

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
