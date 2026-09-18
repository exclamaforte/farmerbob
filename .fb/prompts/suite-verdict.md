<!-- fb:creates crates/farmerbob-core/src/suite_verdict.rs -->
# Task: replace the heuristic that has pointed the wrong way three times

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/suite_verdict.rs` and add `pub mod suite_verdict;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`matrix::suite_quality` classifies a candidate's test suite from the SHAPE of its row of
results: a suite failing N-1 of N implementations is `OVER-FITTED`, one failing 1 of N is
`DISCRIMINATING`. It has misled the adjudicator three times in one week:

- `marker-grammar`: a suite marked OVER-FITTED for failing two of three. The two failures were
  DIFFERENT confirmed defects -- a precedence violation in one arm, a silently-accepted tab in
  the other. Taking the label would have merged an implementation with a confirmed bug.
- `availability`: six failing cells across a 4x4, every one of them the same unpinned clause.
  An arm scored `survival=0.00` with no defect anywhere in its code.
- `reap-plan`: OVER-FITTED again, on partial overlap -- one shared failing test and two unique
  to a single rival.

The shape alone cannot separate these. `witness::read` can, and `testout` now produces its
input. This module joins them so the classification is made from WHICH TESTS FAILED rather than
from how many implementations failed.

## Exact API

```rust
use crate::witness::{Verdict as WitnessVerdict, Witness};

/// What a suite's results across a field support saying about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suite {
    /// Passed against every implementation. Says nothing about quality:
    /// a suite that catches nothing and a field with no defects look
    /// identical from here.
    NoSignal,
    /// Failing implementations failed on DIFFERENT tests.
    ///
    /// This is the shape several real defects make, and it is the shape a
    /// suite with ONE over-fitted test makes when the field also has
    /// unrelated faults: a partially-overlapping set -- A fails `{x, y}`,
    /// B fails `{x}` -- lands here, and the shared `x` may be the
    /// over-fitted one. Like [`Suite::OneDisagreement`], this variant
    /// reports what the evidence shows and does NOT claim which cause
    /// produced it. `reap-plan` was exactly this shape.
    FoundSeparateFaults {
        /// How many implementations failed.
        implementations: u32,
    },
    /// Every failing implementation failed on the SAME tests. One
    /// disagreement repeated, which is what over-fitting looks like -- and
    /// also what a correct suite meeting a field with one shared defect
    /// looks like. The two are NOT distinguishable from this evidence and
    /// this variant must not claim otherwise.
    OneDisagreement {
        /// The shared failing tests, sorted.
        shared: Vec<String>,
        /// How many implementations failed.
        implementations: u32,
    },
    /// Not enough failures to compare, or no test names were recorded.
    /// Carries which, because "one arm failed" and "the instrument recorded
    /// nothing" are different states.
    Inconclusive {
        /// Why, in prose. Never empty.
        reason: String,
    },
}

/// Classify one suite from the witnesses of every implementation it ran
/// against, including its own author's.
pub fn classify(witnesses: &[Witness]) -> Suite;

/// Whether this verdict supports discounting the suite.
///
/// True ONLY for [`Suite::OneDisagreement`], and even then it is a
/// suggestion: see the doc on that variant.
pub fn suggests_overfit(s: &Suite) -> bool;
```

## Falsifiable clauses

1. `classify` delegates to `witness::read` and maps its result. It must NOT re-derive the
   same-or-different comparison; that logic is merged and owns the rule.
2. `WitnessVerdict::Clean` maps to `Suite::NoSignal`.
3. `WitnessVerdict::SeparateFaults` maps to `FoundSeparateFaults`, carrying the number of
   witnesses with a non-empty `failed`.
4. `WitnessVerdict::OneDisagreement { shared }` maps to `Suite::OneDisagreement`, carrying that
   exact `shared` list and the same count.
5. `WitnessVerdict::TooFewFailures` and `NoEvidence` both map to `Inconclusive`, with reasons
   that DIFFER from each other. Pin that the two strings are not equal: collapsing them loses
   the distinction `witness` was built to preserve.
6. `suggests_overfit` is true for `OneDisagreement` and false for all three others. Pin all
   four.
7. **Neither `OneDisagreement` nor `FoundSeparateFaults` may claim a CAUSE.** Each doc states
   what the evidence shows and names the other cause consistent with it:
   - `OneDisagreement` is consistent with over-fitting AND with a correct suite meeting a field
     that shares one defect.
   - `FoundSeparateFaults` is consistent with several real defects AND with one over-fitted
     test among unrelated faults, because PARTIAL overlap lands here -- A fails `{x, y}`,
     B fails `{x}` -- and the shared `x` may be the over-fitted one.

   This is the whole point, and an earlier draft of this spec got it half right: it demanded the
   hedge for `OneDisagreement` and wrote "Evidence of several real defects" unqualified for
   `FoundSeparateFaults`. A spec critic caught it and observed that the `reap-plan` case cited
   in this document as unresolved is itself a partial overlap and therefore lands in exactly
   the variant that was asserting certainty. The old heuristic asserted a cause from a shape
   that does not imply it; the replacement must not do the same in either direction.
8. `implementations` counts witnesses whose `failed` is non-empty, never the total number of
   witnesses. Pin a case where a suite ran against four implementations and two failed.

## Boundaries, at N and at zero

- An EMPTY witness slice: `Inconclusive`, with a reason naming the empty input. NOT `NoSignal`:
  a suite nobody ran and a suite that passed everywhere are different facts.
- ONE witness that passed: `NoSignal`. Clause 2 has no minimum count.
- ONE witness that failed: `Inconclusive` via `TooFewFailures`.
- TWO witnesses failing on identical single-element sets: `OneDisagreement` with
  `implementations: 2`.
- A witness list where every entry passed: `NoSignal`, whatever its length.
- `implementations` at zero is only reachable for `NoSignal` and `Inconclusive`, neither of
  which carries the field. Say so rather than adding a zero that cannot occur.

## Superset status on every enumerated list

`Suite` is a CLOSED set of four. It deliberately has no `OverFitted` variant: this module
cannot prove over-fitting from test names alone, and naming a variant `OverFitted` would
license exactly the overclaim that makes the current heuristic wrong. `suggests_overfit`
exists so a caller can act on the suspicion while the type keeps refusing to assert it.

`witness::Verdict` is CLOSED at five and defined elsewhere. **Use it, imported. Do not define
your own.**

## Composition of aggregate returns

`OneDisagreement::shared` carries `witness::read`'s list verbatim -- sorted, non-empty by
construction -- and you must say why it cannot be empty rather than guarding against it.
`Inconclusive::reason` is never empty and is prose for a human reading an adjudication, not a
machine-readable code.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O.
- Do not change `witness.rs`, `testout.rs` or `matrix.rs`. `matrix::suite_quality` keeps its
  current behaviour; replacing it is a separate decision that wants this module's evidence
  first.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the ONE file the task declares, and nothing else.** Not "only that crate" --
  that is what this line used to say, and it understated the rule by a wide margin. The
  measurement is `farmerbob_core::scope`, it compares paths as exact strings, and it permits
  exactly two things: the declared target, and adding `pub mod y;` to the `lib.rs` beside it
  when a NEW file needs that to compile. There is no tolerance band: ONE other changed file
  is a departure, and a departure now yields the verdict `OutOfScope`, which is not a pass.

  Read this as permission, not only as prohibition. If the task's own instructions make the
  wider workspace fail to build -- a new enum variant breaking a caller in another crate, say
  -- that breakage is EXPECTED and you must leave it. Reaching out to fix it is the departure.
  Say what you left broken in your handoff.

  This line was wrong until 2026-09-17, and three consecutive runs by one arm changed 51, 52
  and 51 files while being told "only the crate". They were measured against a rule they had
  not been given, which is the whole of farmerbob-vg0: disclose what is scored, or you are
  measuring house style rather than capability.

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
   it. Count is the fallback, not the target: ten tests that pin ten distinct behaviours beat
   forty that restate one. Your tests must be good enough to catch a bug in **any** correct-looking
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

Three tasks have now been decided by candidates disagreeing about exactly this -- an unpinned
boundary or an unstated list -- rather than about anything either of them got wrong, every time
because a test asserted a case the specification never fixed.

## A signature that cannot compute what the spec promises

If a clause in this spec describes a value that the API it also fixes makes **uncomputable**,
say so in your handoff and implement the closest honest thing. Do not silently return a
placeholder.

This is not hypothetical. A previous task's spec asked `status(resource)` to report "how long
the current holder has held the lease" while fixing a signature that takes no clock. Several
implementations returned `Duration::zero()` -- correct by necessity, indistinguishable from a
bug -- and the same spec's `holder_died(holder) -> Option<Grant>` could report only one grant
for a holder that may hold many, so its own "never left locked" invariant was unreportable.
Three critics found all of it, in three different implementations, which is how the fault was
traced to the spec rather than to any arm.

A defect that appears in nearly every implementation is evidence about the specification, not
about the field. Naming it in your handoff routes it where the fix belongs.

## Reuse the crate's existing types

If this spec names a type that already exists in `farmerbob-core` -- `Verdict`, `Grade`,
`RunState`, `Outcome`, `Measurement` and so on -- you **use that type**, imported, and you do
not define your own. A new public type whose name already exists in the crate is a defect,
scored as one, however good its internals are.

Forty-four modules have been merged, each written without sight of the others, and seven core
concepts now exist two or three times in mutually incompatible shapes. `Verdict` exists three
times. That happened one reasonable-looking local decision at a time. If you believe the
existing type genuinely cannot express what this task needs, say so in your handoff, name the
type and the clause it cannot express, and extend it rather than shadowing it.

## Run the tests quietly

Use `cargo test -q` and `cargo build -q`. This workspace has over nine hundred tests and a
plain `cargo test` prints a line for every one of them; three arms on a recent task spent
their whole run reading their own test output and produced nothing at all. `-q` prints the
summary and any failures, which is the entire signal.

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
