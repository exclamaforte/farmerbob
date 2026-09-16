<!-- fb:creates crates/farmerbob-core/src/vrouter.rs -->
# Task: implement the verification router

Create `crates/farmerbob-core/src/vrouter.rs` and declare it from `lib.rs` with
`pub mod vrouter;`. Change nothing else.

## Context

farmerbob runs many AI coding agents on the same task, then has to decide whether each
candidate is acceptable. Checks cost money and time, so something must decide **which check
to run next, and when to stop**. That is this module.

The hard constraint: a bad *implementer* wastes one run, but a bad *verifier* mislabels every
candidate it judges, and those labels train the router. So verifier statistics are tracked as
two separate error rates, never one accuracy number — a verifier that approves everything
scores 70% "accuracy" on our current workload while detecting nothing.

**This module is pure logic: no I/O, no clock, no randomness.** Everything it needs is passed
in. That is what makes it testable.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct VerifierId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TaskFamily(pub String);

/// What a check concluded. `SpecAmbiguous` is about the TASK; `UnableToAssess` is about the
/// checker's own capability. They must not be merged.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Finding {
    DemonstratedDefect { detail: String },
    SuspectedDefect { detail: String },
    NoDefectFound,
    UnableToAssess { reason: String },
    SpecAmbiguous { detail: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Check {
    ExecutedGate,
    CrossExamination,
    BehaviouralVerifier(VerifierId),
    CodeReview(VerifierId),
    TargetedReproduction { claim: String },
    DeputyDiscriminatingTest,
    OrchestratorAudit,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Decision { Accept, Reject, Repair, Undetermined }

/// Per (verifier, task family). Counts only, so it can be recomputed from an append-only log.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifierStats {
    pub rejected_defective: u32,
    pub missed_defective: u32,
    pub approved_acceptable: u32,
    pub rejected_acceptable: u32,
    pub abstained: u32,
    pub findings_reported: u32,
    pub findings_confirmed: u32,
}

impl VerifierStats {
    /// P(reject | defective). None when no defective cases have been seen.
    pub fn sensitivity(&self) -> Option<f64>;
    /// P(approve | acceptable). None when no acceptable cases have been seen.
    pub fn specificity(&self) -> Option<f64>;
    /// confirmed / reported. None when nothing was reported.
    pub fn precision(&self) -> Option<f64>;
    /// Total adjudicated observations.
    pub fn n(&self) -> u32;
    /// Calibrated once it has at least `min_n` adjudicated observations AND has seen at
    /// least one defective case (otherwise sensitivity is unmeasured).
    pub fn is_calibrated(&self, min_n: u32) -> bool;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Evidence {
    pub family: TaskFamily,
    /// Checks already run, in order, with what they found.
    pub completed: Vec<(Check, Finding)>,
    /// Arms that must not verify this candidate: the implementer, plus anyone already used.
    pub excluded: Vec<VerifierId>,
    /// Review budget already spent, in whatever unit the caller uses.
    pub spent: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RouterConfig {
    pub review_budget: u32,
    pub cost_per_check: u32,
    pub min_n_for_calibration: u32,
    /// Verifiers available, with their family and their stats for this task family.
    pub roster: Vec<(VerifierId, String, VerifierStats)>,
}

/// Pick the next check, or None to stop.
pub fn next_check(ev: &Evidence, cfg: &RouterConfig) -> Option<Check>;

/// The decision implied by the evidence so far.
pub fn decide(ev: &Evidence) -> Decision;

/// Verdicts from an uncalibrated verifier are recorded but must not label a candidate.
pub fn is_shadow(v: &VerifierId, cfg: &RouterConfig) -> bool;
```

## Required behaviour

`decide`:
- any `DemonstratedDefect` → `Reject`
- any `SpecAmbiguous` → `Undetermined` (the task is at fault; do not score the arm)
- `ExecutedGate` returning `NoDefectFound` plus at least one other `NoDefectFound` → `Accept`
- only `SuspectedDefect` and no demonstration → `Repair`
- nothing conclusive → `Undetermined`
- `SpecAmbiguous` outranks `DemonstratedDefect`: if the spec does not determine the answer,
  a "defect" against it is not established.

`next_check`:
- Stop (`None`) when the budget is exhausted, i.e. `spent + cost_per_check > review_budget`.
- Stop when `decide` already returns `Reject` — further review cannot change it.
- Stop when `decide` returns `Undetermined` because of `SpecAmbiguous` — escalating to more
  reviewers cannot fix an underspecified task.
- Otherwise follow this order, skipping any already in `completed`:
  `ExecutedGate` → `CrossExamination` → `BehaviouralVerifier` → `CodeReview`
  → `TargetedReproduction` (only when a `SuspectedDefect` exists, carrying its detail as
  `claim`) → `DeputyDiscriminatingTest` → `OrchestratorAudit`.
- Never pick a verifier in `excluded`.
- Prefer a verifier whose model family differs from every verifier already used on this
  candidate; fall back to any eligible one only if none differs.
- Among otherwise-equal candidates prefer the higher `sensitivity()`, treating `None` as
  worse than any measured value.

`is_shadow`: true when the verifier is absent from the roster, or its stats are not
`is_calibrated(min_n_for_calibration)`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`. Return `Option`/`Decision` values instead.
- Integer arithmetic must not overflow or divide by zero on any input, including all-zero
  stats and an empty roster.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule above, named after it. At minimum: the `SpecAmbiguous` precedence over
`DemonstratedDefect`; budget exhaustion; the exclusion list being respected; family-diversity
preference; `None` sensitivity ranking below any measured value; and every `VerifierStats`
accessor returning `None` rather than dividing by zero on default stats.

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
