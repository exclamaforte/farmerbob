<!-- fb:creates crates/farmerbob-core/src/adjudicate.rs -->
# Task: adjudication — turning evidence into a winner, or admitting there isn't one

Create `crates/farmerbob-core/src/adjudicate.rs`, declare it from `lib.rs` with
`pub mod adjudicate;`. Change nothing else except that one `pub mod` line.

## Context

When several agents attack one task, the harness collects objective evidence about each:
conformance against a hidden suite, defect sensitivity of their own tests, cross-examination
survival, clippy count, scope discipline, cost. Something has to turn that into a decision.

The hard-won constraint, from running this on real tasks: **most of the time the evidence
does not discriminate.** On one recent task two candidates tied on conformance (10/10 each),
defect sensitivity (6/6 each), clippy (0 each) and panic-freedom. An adjudicator that always
returns a winner will manufacture one from noise — and it will usually do so by reaching for
whatever metric happens to differ, which is the definition of overfitting to a sample of one.

So `Undecided` is a first-class outcome, and the module must say what further evidence would
break the tie. That is the signal the verification router consumes.

A second rule, learned the same way: **a proxy never overrides the thing it proxies for.**
Test COUNT is a proxy for test QUALITY; defect sensitivity measures test quality directly.
When both are present and they disagree, the direct measurement wins and the proxy is
discarded, not averaged in.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// One candidate's objective record. `None` means not measured, which is never
/// the same as a measured zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub arm: String,
    /// Fraction of a hidden conformance suite passed, 0.0..=1.0.
    pub conformance: Option<f64>,
    /// Fraction of injected defects this candidate's own tests caught.
    pub defect_sensitivity: Option<f64>,
    /// Fraction of other arms' suites this implementation survives.
    pub survival: Option<f64>,
    pub clippy: Option<u32>,
    /// Crates modified. The task names one; more is scope creep.
    pub crates_touched: Option<u32>,
    pub tests: Option<u32>,
    pub lines: Option<u32>,
    /// Measured USD. `Some(0.0)` for a plan-based arm is a real zero.
    pub cost_usd: Option<f64>,
}

/// Which measurement decided it, in the rubric's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterion { Conformance, ScopeDiscipline, DefectSensitivity, Survival, Clippy, Cost, Simplicity }

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Winner { arm: String, on: Criterion, margin: f64 },
    /// No criterion separated them. Names the candidates still in contention and the
    /// evidence that would.
    Undecided { tied: Vec<String>, next: Vec<Criterion> },
    /// Nothing passed the gate.
    NoCandidate,
}

/// Apply the criteria in order, stopping at the first that separates the field.
///
/// `epsilon` is the smallest difference treated as real; anything smaller is a tie.
/// Float noise must never decide a ranking.
pub fn adjudicate(candidates: &[Evidence], epsilon: f64) -> Verdict;

/// Candidates that clear the gate: conformance is known and equals 1.0, and at least
/// one test ran. A candidate that did not build never reaches adjudication.
pub fn eligible(candidates: &[Evidence]) -> Vec<&Evidence>;

/// Criteria that are measured for some candidates but not all. These are the cheapest
/// way to break a tie, because the measurement already exists for part of the field.
pub fn missing_evidence(candidates: &[Evidence]) -> Vec<Criterion>;
```

## Required behaviour

- Criteria apply in exactly this order: `Conformance`, `ScopeDiscipline`,
  `DefectSensitivity`, `Survival`, `Clippy`, `Cost`, `Simplicity`. The first that separates
  the field decides, and later criteria are never consulted.
- `ScopeDiscipline` prefers FEWER `crates_touched`; a candidate touching more crates than
  the minimum loses to one touching fewer, whatever else is true of it.
- `Clippy` and `Cost` prefer lower; `Simplicity` prefers fewer `lines`.
- A criterion where any contender's value is `None` is SKIPPED, not treated as zero, and it
  appears in the `next` list of an `Undecided` verdict.
- `tests` is never a criterion. It is a proxy for `defect_sensitivity`; when sensitivity is
  measured the count adds nothing, and when it is not, a count still cannot substitute for it.
- Differences below `epsilon` are ties. With every criterion tied or skipped the verdict is
  `Undecided`, listing every still-tied arm sorted by name.
- `margin` is the winner's advantage over the best other contender, always positive.
- One candidate alone is a `Winner` only if it clears `eligible`; otherwise `NoCandidate`.
- `NoCandidate` when the input is empty or nothing clears the gate.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No NaN may escape, and a NaN in the input must not be ranked as though it were a number.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a conformance difference decides before scope is
consulted; scope discipline beats a better defect sensitivity; a `None` in one contender
skips that criterion and lists it in `next`; a higher test count does NOT win; an
all-tied field is `Undecided` with names sorted; `margin` is positive; a NaN does not win;
an empty field is `NoCandidate`.

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
