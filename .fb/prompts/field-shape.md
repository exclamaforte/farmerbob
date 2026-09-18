<!-- fb:creates crates/farmerbob-core/src/field_shape.rs -->
# Task: the matrix has a shape and the harness can only read one cell at a time

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/field_shape.rs` and add `pub mod field_shape;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

Cross-examination runs every candidate's suite against every candidate's implementation and
then reads the result one suite at a time: `crossx::read` asks, for each failing cell, whether
that suite is discriminating or over-fitted. That question is unanswerable below three
implementations, and `crossx::detect_partition` refuses outright below four.

The two most recent waves both fell in the gap, and both carried a loud message the harness
printed as "no signal":

- wave85, three implementations, EVERY off-diagonal cell a failure. Three self-consistent
  implementations each rejecting both others is not three over-fitted suites. It is a
  specification that did not pin what all three suites test. The table said
  `OVER-FITTED / FoundSeparateFaults(2)` three times and named no cause.
- wave86, two implementations, EVERY off-diagonal cell a pass. The table said
  `Inconclusive (NoEvidence)` twice. "No evidence" and "the field agrees completely" are
  different facts and the harness cannot tell them apart.

Both readings were made by hand, twice, from the same table. The shape of the WHOLE matrix is
a fact about the FIELD, it is available at two implementations, and nothing computes it.
(beads farmerbob-gm4, farmerbob-ltm1)

## Exact API

```rust
use crate::crossx::Matrix;
use crate::measurement::Measurement;

/// What the matrix as a whole says about the field. Exactly these variants and
/// no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// Every off-diagonal cell passed. The field agrees; no suite separates
    /// any pair.
    Unanimous {
        /// How many implementations agreed.
        arms: usize,
    },
    /// Every off-diagonal cell failed. Each implementation is self-consistent
    /// and rejects every other.
    MutualRejection {
        /// How many implementations mutually rejected.
        arms: usize,
    },
    /// The field splits into camps that pass within themselves and fail
    /// across. Requires at least two camps and at least two arms in one of
    /// them.
    Partition {
        /// The camps. See the composition section for the exact ordering.
        camps: Vec<Vec<String>>,
    },
    /// Exactly one implementation fails every foreign suite while every other
    /// pair passes. The odd one out.
    Isolated {
        /// The arm that stands alone.
        arm: String,
    },
    /// None of the above.
    Mixed,
}

/// Read the shape of a whole cross-examination matrix.
///
/// `Missing(..)` when there is nothing to read rather than when the answer is
/// dull: see the boundaries below for exactly which cases are which.
pub fn shape(m: &Matrix) -> Measurement<Shape>;

/// Whether this shape is evidence about the SPECIFICATION rather than about
/// any implementation. True for exactly two variants.
pub fn points_at_spec(s: &Shape) -> bool;

/// One sentence a human can read in the pipeline log. Non-empty for every
/// shape; its exact wording is not pinned.
pub fn narrate(s: &Shape) -> String;
```

## Falsifiable clauses

1. A matrix whose every off-diagonal cell is `Cell::Pass` is `Unanimous { arms }`, where
   `arms` is the number of implementations. Pin it at three arms and at two.
2. A matrix whose every off-diagonal cell is `Cell::Fail` is `MutualRejection { arms }`. Pin
   it at three arms (wave85's shape) and at two.
3. `points_at_spec` is true for `MutualRejection` and for `Partition`, and false for
   `Unanimous`, `Isolated` and `Mixed`. A field that all disagrees, or that splits into
   internally-consistent camps, is evidence about the document all of them read; one arm
   standing alone is evidence about that arm.
4. **`Partition` is never reported at two arms.** At two arms "camp {a} and camp {b}" and
   "a and b reject each other" are the SAME table, and the partition reading adds a structure
   the data does not contain. Two arms that fail each other are `MutualRejection { arms: 2 }`.
5. `Isolated { arm }` requires that every pair NOT involving `arm` passes both ways, and that
   `arm` fails every foreign suite in both directions. Fewer than three arms cannot exhibit
   it, because there is no surviving pair; pin that a two-arm matrix is never `Isolated`.
6. The DIAGONAL is not read. A cell where the suite and the implementation are the same arm
   says nothing about the field, and `shape` ignores it entirely. Pin that changing a
   diagonal cell does not change the answer.
7. `shape` returns `Missing` rather than a `Shape` when the two axes are not the same set of
   names. `crossx::Matrix` permits suites and implementations to differ, and "off-diagonal"
   has no meaning when they do.

## Boundaries, at N and at zero

- ZERO implementations: `Missing(NothingToMeasure { .. })`. An empty field has no shape.
- ONE implementation: `Missing(NothingToMeasure { .. })`. There are no off-diagonal cells at
  all, so nothing was measured. This is NOT `Unanimous { arms: 1 }`; a field of one agrees
  with nobody.
- TWO implementations: `Unanimous` and `MutualRejection` are both reachable and both correct.
  `Partition` and `Isolated` are not; clauses 4 and 5.
- THREE implementations: every variant is reachable.
- Every off-diagonal cell is `Cell::Error`: `Missing(InstrumentFailed { .. })`. The suites did
  not run; that is a fact about the harness, not about the field.
- SOME off-diagonal cells are `Cell::Error` and some are not: `Error` cells are excluded from
  every denominator, exactly as `crossx` already excludes them, and the shape is read from
  what remains. Say so; do not treat an `Error` as a `Fail`.
- A matrix in which one ARM's every off-diagonal cell is `Error`: that arm contributes no
  usable cells. State what you do with it and pin the answer you chose -- the one rule is
  that it must not silently become `Isolated`, because "its suite never compiled" and "its
  suite failed everywhere" are different facts.

## Superset status on every enumerated list

`Shape`'s variants are a CLOSED set of exactly five: `Unanimous`, `MutualRejection`,
`Partition`, `Isolated`, `Mixed`. Adding a sixth is a defect, and your tests may not assert on
any shape outside these five.

`Cell`'s variants are likewise closed and they are `crossx::Cell`'s, not yours: `Pass`, `Fail`,
`Error`. `Absent`'s four are `measurement::Absent`'s. You define none of these.

`Mixed` is the catch-all and it is REQUIRED to be reachable: a matrix that is none of the
other four is `Mixed`, never a `Missing`. Absence is for nothing measured, not for an answer
that is uninteresting.

## Composition of aggregate returns

`Partition.camps`: every arm appears in EXACTLY ONE camp; each camp's members are sorted by
name; the camps themselves are sorted by their first member. Two runs over the same matrix
produce identical output, including identical ordering. There are at least two camps.

`narrate` returns one non-empty line for any shape, and what it says is prose: the specification
fixes that it is non-empty and mentions the shape, and fixes nothing about its wording.

Say why `shape` and `points_at_spec` must be tested together rather than separately:
`points_at_spec` is only meaningful against the shapes `shape` can produce, and a matched pair
of bugs -- a `shape` that never returns `MutualRejection` and a `points_at_spec` that gets it
wrong -- would pass two independent tests while the pipeline kept printing "no signal" at the
exact table this module was written to read.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies and no I/O. The matrix arrives as a `&Matrix`.
- Build your test matrices with `crossx::Matrix::new`, which is the pinned constructor. It
  returns `Result`; handle it, in tests, without `unwrap()` outside `#[cfg(test)]` -- inside
  `#[cfg(test)]` it is permitted.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.

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

## A field the spec calls prose is not a field your tests may quote

If the specification describes a string by what it should SAY -- a reason, a grounds, a
diagnostic, a message "for a human reading an adjudication" -- then its exact wording is NOT
pinned, and a test asserting the exact text fails a rival that says the same thing differently.

This has cost a cross-examination cell in three consecutive tasks:

- a test asserting the exact text of `Inconclusive::reason`;
- a test asserting `"fewer than two implementations failed"` and `"no test names were recorded"`;
- a test asserting `reason.contains("fails against merged code")`.

In each case the spec pinned that the string was NON-EMPTY and said what it should convey, and
in each case a rival conveying it in other words was marked wrong.

Test the requirement, not the sentence. If the spec says the reason must say the test failed
against merged code, assert that it mentions failing and mentions merged -- separately,
case-insensitively -- or assert only that it is non-empty. One arm put it exactly right while
reviewing another: check "the semantic requirement ... without asserting on fragile, exact
string formatting that would fail against rival implementations".

The exception is a string the spec quotes verbatim as a value, such as a sentinel like
`"(no output)"`. A quoted literal is pinned; a described one is not.

## Your suite is run against OTHER implementations

This is the rule that has cost the most signal, four times, and it is stated here in terms of
what is MEASURED rather than of what is intended.

Every candidate's test suite is extracted and run against every other candidate's code. A suite
that calls anything the specification does not pin **fails to compile against every rival**, and
those cells are recorded as API-incompatible: you forfeit the cross-examination signal you would
otherwise have earned, however good your tests are.

Four arms have lost it this way, each for an addition that was reasonable on its own:

- a `Registry::new()` / `insert()` pair, used to build test fixtures;
- five tests asserting cases the spec never pinned;
- an inherent method beside the pinned free function, called five times in tests;
- a `DiffLine::added(..)` constructor, used to build test inputs.

None of those are bad code. Add them if they help a caller. **Your tests must go through the
pinned surface anyway** -- construct values from their public fields, call the free function the
Exact API names, and assert only on behaviour the specification fixes. If you would have to call
your own addition to write the test, write the test the longer way.

The rule is not "do not add API". It is "do not make your suite depend on API a rival has no
reason to have".

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
