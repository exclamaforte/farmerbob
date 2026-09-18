<!-- fb:creates crates/farmerbob-core/src/stage_outcome.rs -->
# Task: a stage that exits 0 and writes nothing looks exactly like a stage that worked

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/stage_outcome.rs` and add `pub mod stage_outcome;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`fb-pipeline.sh` runs the scoring stages in order and, until this morning, recorded each one as
done when its command exited 0. `promote` exited 0 for sixty-six tasks while writing no
artefact at all, and every one of those tasks carried a signature file saying the stage had
completed. The pipeline was not lying: it had asked the only question it knew how to ask.

The rule it was missing is one line long -- a stage that exits 0 and produces no artefact did
not run -- and it now lives in shell, untested, beside the three other places that will need
it. The shell also cannot express the case that matters most: the pipeline could not LOOK at
the artefact. `stat` failing and `stat` reporting zero bytes are different facts, and the
current code turns both into "produced nothing", which is the same coercion in a new place.

This module is the decision, in the crate, with the third answer available.

## Exact API

```rust
use crate::measurement::Measurement;

/// What a stage actually did, as distinct from what its exit status said.
/// Exactly these variants and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// Exited 0 and left a non-empty artefact.
    Ran,
    /// Exited 0 and left no artefact, or an empty one. The stage is not
    /// trustworthy and its signature must not be written.
    ProducedNothing,
    /// Exited non-zero.
    Failed {
        /// The exit status, as reported.
        rc: i32,
    },
    /// Exited 0, and the artefact could not be inspected at all, so
    /// neither `Ran` nor `ProducedNothing` is known to be true.
    Unknown {
        /// Why the artefact could not be inspected, in prose. Never empty.
        why: String,
    },
    /// Never attempted.
    Skipped {
        /// Why it was not attempted, in prose. Never empty.
        why: String,
    },
}

/// One stage of a pipeline, named, with what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// The stage's name, as the pipeline prints it.
    pub name: String,
    /// What it did.
    pub outcome: StageOutcome,
}

/// Classify one stage from its exit status and the size of the artefact it
/// was supposed to leave.
///
/// `bytes` is `Observed(n)` when the artefact's size was read -- `n` may be
/// zero -- and `Missing(..)` when it could not be. This module does no I/O:
/// the caller stats the file and says what happened.
pub fn classify(rc: i32, bytes: &Measurement<u64>) -> StageOutcome;

/// Whether this outcome permits the pipeline to record the stage as done.
/// True for exactly one variant.
pub fn may_sign(o: &StageOutcome) -> bool;

/// The first stage that stops the pipeline, if any: the earliest stage in
/// `stages` whose outcome does not permit a signature.
pub fn first_blocking(stages: &[Stage]) -> Option<&Stage>;

/// A one-line report for a human reading the pipeline log, for each stage
/// in the order given.
pub fn report(stages: &[Stage]) -> Vec<String>;
```

## Falsifiable clauses

1. `classify` returns `Failed { rc }` for every non-zero `rc`, whatever `bytes` says. A stage
   that fails has not produced a trustworthy artefact even when a file is sitting there, and
   the exit status is the fact that is known first.
2. `classify(0, Observed(n))` is `Ran` when `n > 0` and `ProducedNothing` when `n == 0`. Pin
   both sides of that boundary, at `n = 0` and at `n = 1`.
3. `classify(0, Missing(..))` is `Unknown`, and its `why` is non-empty and derived from the
   stated reason in the `Absent`. It is NEVER `ProducedNothing`: "I could not look" and "I
   looked and it was empty" are different facts, and collapsing them is the defect this module
   exists to close.
4. `may_sign` is true for `Ran` and false for every other variant, including `Unknown`.
5. `first_blocking` returns the EARLIEST non-signable stage, not the last and not the worst. A
   later failure does not outrank an earlier one, because the later stage ran against whatever
   the earlier one left.
6. `first_blocking` on a slice where every stage may sign returns `None`, and on an EMPTY slice
   returns `None`. An empty pipeline is not blocked; it is empty.
7. `report` returns exactly one line per stage, in the order given, and every line contains the
   stage's name. What else a line says is prose and is not pinned.

## Boundaries, at N and at zero

- `rc == 0` with `Observed(0)`: `ProducedNothing`. Clause 2.
- `rc == 0` with `Observed(1)`: `Ran`. Clause 2.
- A NEGATIVE `rc`, which is what a signal-killed process reports through some launchers:
  non-zero, so `Failed { rc }` with the negative value preserved verbatim. Say so; do not
  normalise it.
- `Skipped` is never produced by `classify`. It is constructed by a caller that decided not to
  run the stage at all, and `classify` has no argument that could express that. Say this in
  your handoff rather than inventing a third parameter.
- `first_blocking` on an empty slice: `None`. Clause 6.
- `report` on an empty slice: an empty vector, not a vector holding one line that says the
  pipeline was empty.
- A stage whose `name` is the empty string: it is degenerate, not invalid. It classifies,
  reports and blocks like any other. Say so rather than rejecting it.

## Superset status on every enumerated list

`StageOutcome`'s variants are a CLOSED set of exactly five: `Ran`, `ProducedNothing`,
`Failed`, `Unknown`, `Skipped`. Adding a sixth is a defect, and your tests may not assert on
any state outside these five.

The four `Absent` variants are likewise closed, and they are `measurement::Absent`'s, not
yours: `NotAttempted`, `InstrumentFailed`, `NothingToMeasure`, `Untrusted`. `classify` treats
ALL FOUR the same way -- any `Missing` is `Unknown` -- and you may not give one of them a
different answer from the others. Say why: the distinction between them is about the
INSTRUMENT, and this function's question is about the STAGE.

## Composition of aggregate returns

`report` returns one line per stage, in the order of the input slice, never reordered and never
filtered -- a stage that is fine still gets a line, because a report that lists only problems
cannot be read as a list of what ran.

`first_blocking` returns a borrow of an element of its input, so the caller can name the stage;
it does not return a copy, an index or a bool.

Say why `classify` and `may_sign` must be tested together rather than separately: `may_sign` is
only meaningful against the outcomes `classify` can produce, and a matched pair of bugs -- a
`classify` that never returns `ProducedNothing` and a `may_sign` that wrongly signs it -- would
pass two independent tests and reproduce the original defect exactly.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies and no I/O. The stage's exit status and the artefact's size arrive as
  arguments.
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
