<!-- fb:creates crates/farmerbob-core/src/witness.rs -->
# Task: "fails 2 of 3" is two different things and the label picks the wrong one

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/witness.rs` and add `pub mod witness;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## The measurement

On marker-grammar, three candidates. gemini-38-flash's suite failed two of the three
implementations, which `matrix::suite_quality` reports as `OVER-FITTED` -- the N-1-of-N
signature of a suite fitted to its own author's code.

It was the opposite. Running both failing cells by hand:

- against glm-53-flash, the failing test was `tie_breaking_order_among_rejections`: a line with
  two faults reported the lower-ranked one, violating a precedence the spec fixes by name.
- against or-nemotron-ultra, the failing test was `tab_mixed_with_spaces_is_still_a_tab`: no
  rejection at all where the spec requires one.

**Different tests, different defects, both real.** The label read "discount this suite", and
taking it would have merged an implementation with a confirmed precedence violation.

The distinguishing evidence was in the cell output the whole time and was thrown away: an
over-fitted suite fails everywhere for the SAME reason, and a strict correct suite meeting a
field of flawed rivals fails for DIFFERENT ones. This module reads that.

## Exact API

```rust
/// Which of a suite's tests failed against one implementation.
///
/// Test names as the runner printed them. An empty set means the suite passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    /// The implementation the suite ran against.
    pub impl_arm: String,
    /// Names of the failing tests, sorted, without duplicates.
    pub failed: Vec<String>,
}

/// What a suite's failures across a field actually indicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The suite passed against every implementation it ran on.
    Clean,
    /// Every failing implementation failed on the SAME set of tests. One
    /// disagreement, repeated -- the suite encodes a reading its author's
    /// code satisfies and the field does not.
    OneDisagreement {
        /// The tests every failing implementation failed on, sorted.
        shared: Vec<String>,
    },
    /// Failing implementations failed on DIFFERENT tests. Not one
    /// disagreement but several, which is what a strict correct suite
    /// looks like against a flawed field.
    SeparateFaults,
    /// Fewer than two implementations failed, so "same or different" has no
    /// content. NOT a judgement: one failure is one data point.
    TooFewFailures,
    /// No witness carried any test name. The cells recorded pass/fail and
    /// nothing else, so nothing can be concluded. Distinct from [`Clean`]:
    /// a suite that passed and a suite nobody recorded are different.
    NoEvidence,
}

/// Read the witnesses for ONE suite across a field.
///
/// `witnesses` holds one entry per implementation the suite ran against,
/// including the suite's own author.
pub fn read(witnesses: &[Witness]) -> Verdict;

/// The tests that failed against exactly one implementation, with that
/// implementation's name. Sorted by test name, then by arm.
///
/// These are the strongest single-defect evidence a matrix carries: a test
/// that only one arm fails is pointing at that arm, not at the spec.
pub fn unique_failures(witnesses: &[Witness]) -> Vec<(String, String)>;
```

## Falsifiable clauses

1. Every witness with an empty `failed` yields `Clean`.
2. Exactly ONE witness failing, on any number of tests, yields `TooFewFailures`. Never
   `OneDisagreement` and never `SeparateFaults`. Say in the doc why: with one failure there is
   nothing to compare it against, and calling it either way is an assertion the data does not
   support.
3. Two or more failing witnesses with the IDENTICAL set of failing test names yields
   `OneDisagreement`, carrying that set.
4. Two or more failing witnesses whose failing sets are not all identical yields
   `SeparateFaults`. **Partial overlap is `SeparateFaults`, not `OneDisagreement`**: if A fails
   `{x, y}` and B fails `{x}`, the suite found something in A that it did not find in B, so
   there is more than one disagreement. Pin this case explicitly -- it is the one a reader
   guesses wrong.
5. An empty `witnesses` slice yields `NoEvidence`, not `Clean`.
6. Witnesses that all carry empty `failed` yield `Clean` -- clause 1 -- but a slice where every
   entry has an empty `failed` AND the caller had no test names to give is indistinguishable
   from it. Say so in the doc and say the caller must not pass empty sets to mean "unknown".
7. `unique_failures` returns a test name exactly when exactly one witness lists it. A test
   failed by two arms is not unique and is excluded.
8. `unique_failures` on `Clean` input returns empty.
9. `read` never panics on duplicate arm names, duplicate test names within a witness, or
   names that differ only in case. Comparison is EXACT, as everywhere in this crate.

## Boundaries, at N and at zero

- ZERO witnesses: `NoEvidence` from `read`, empty vector from `unique_failures`.
- ONE witness, failing: `TooFewFailures`.
- ONE witness, passing: `Clean`. Clause 1 has no minimum.
- TWO witnesses, both failing on the same single test: `OneDisagreement { shared: [that] }`.
- A witness whose `failed` contains the same name twice: treated as once. `Witness::failed` is
  documented sorted-and-deduplicated; state what `read` does when handed one that is not, and
  pin it. Do not panic.
- Every witness failing, on identical sets: still `OneDisagreement`. That a suite fails the
  WHOLE field, its own author included, is a different fact and is not this function's job;
  say so rather than adding a variant.

## Superset status on every enumerated list

`Verdict` is a CLOSED set of five. The set of TEST NAMES and ARM NAMES is open and nothing may
key off known values -- no matching on `tests::`, no stripping of a `xtests_` prefix, no
assumptions about the runner's format.

## Composition of aggregate returns

`Witness::failed` is sorted, deduplicated. `OneDisagreement::shared` is sorted and non-empty by
construction -- say why it cannot be empty. `unique_failures` returns `(test_name, arm)` pairs
sorted by test name then arm, without duplicates, and an empty vector is a real answer meaning
every failure was shared.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O.
- Do not change `matrix.rs` or any existing signature. This module is read BY the adjudicator
  alongside `matrix::suite_quality`, and replacing that is a separate decision.
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
