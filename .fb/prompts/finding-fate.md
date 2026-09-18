<!-- fb:creates crates/farmerbob-core/src/finding_fate.rs -->
# Task: a confirmed defect in the merged winner leaves no trace

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/finding_fate.rs` and add `pub mod finding_fate;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

Escalation turns a confirmed critique finding into a permanent test. It works, and it has one
hole that swallows the most valuable findings there are.

When a critic finds a real defect in the arm that goes on to WIN, escalation writes the test,
runs it against the merged reference, watches it fail -- because the defect is real and unfixed
-- and retracts it:

    glm-53-flash: escalated 1 finding(s) from gemini-38-flash
    glm-53-flash: VETOED against the merged reference -- retracting

The veto is right to refuse a failing test into a green suite. But the finding was ACCEPTED by
the adjudicator, is real, and now exists nowhere except a bead somebody wrote by hand. The
better the critic, the more likely its finding is about the winner, and the more certain it is
to vanish.

This module decides what should happen to a finding instead of leaving it to whoever is reading.

## Exact API

```rust
/// Where a confirmed finding should end up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// The subject lost. Its code is not merged, so a test has nothing to
    /// run against and the finding is recorded against the critic's credit
    /// and nothing more.
    NoSubject,
    /// The subject won and the test PASSES against the merged code: the
    /// defect was fixed on the way in, or never applied to the merged
    /// version. Escalate it -- it is a regression guard.
    Escalate,
    /// The subject won and the test FAILS against the merged code: the
    /// defect is real and shipped. Do not put a failing test in the suite;
    /// file it as known-broken so it survives.
    FileAsKnownDefect {
        /// Why it cannot be escalated, for the bead body. Never empty.
        reason: String,
    },
    /// The test could not be run at all, so nothing is known. NOT the same
    /// as a test that failed.
    Unverifiable {
        /// What stopped it, for a human. Never empty.
        reason: String,
    },
}

/// What happened when the escalated test ran against the merged reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VetoRun {
    /// It ran and passed.
    Passed,
    /// It ran and failed.
    Failed,
    /// It did not run: no tests matched, the build broke, nothing executed.
    DidNotRun,
}

/// Decide a finding's fate.
///
/// `subject_merged` is whether the arm the finding is ABOUT is the one that
/// was merged.
pub fn decide(subject_merged: bool, veto: VetoRun) -> Fate;

/// Whether this fate means the finding survives somewhere a later reader
/// will see it.
///
/// True for [`Fate::Escalate`] and [`Fate::FileAsKnownDefect`]. False for
/// [`Fate::NoSubject`] and [`Fate::Unverifiable`] -- say in the doc that
/// those two are where findings are lost today, and that the module names
/// them so the loss is visible rather than silent.
pub fn survives(f: &Fate) -> bool;
```

## Falsifiable clauses

1. `subject_merged == false` is `NoSubject`, whatever `veto` says. A test against code that is
   not in the tree establishes nothing about the tree.
2. `subject_merged == true` with `VetoRun::Passed` is `Escalate`.
3. **`subject_merged == true` with `VetoRun::Failed` is `FileAsKnownDefect`, never `Escalate`
   and never silently dropped.** This is the clause the module exists for. Its `reason` is
   non-empty and says the test fails against merged code.
4. `VetoRun::DidNotRun` is `Unverifiable` when the subject was merged. Pin that it is NOT
   `Escalate` and NOT `FileAsKnownDefect`: a test that did not run has established nothing, and
   both of those assert something.
5. `VetoRun::DidNotRun` with `subject_merged == false` is `NoSubject` -- clause 1 has no
   exceptions, and which of the two reasons applies does not change the outcome. Say so.
6. `survives` is true for exactly `Escalate` and `FileAsKnownDefect`. Pin all four variants.
7. Every `reason` this module produces is non-empty. Say why it cannot be empty rather than
   guarding against it.

## Boundaries, at N and at zero

- The full input space is six combinations (two booleans by three `VetoRun` variants). Pin ALL
  SIX explicitly; there is no boundary to interpolate and a table this small should be complete
  in the tests rather than sampled.
- No input is invalid: every combination has a defined answer, so there is no error case and no
  `Option` in the return. State that.

## Superset status on every enumerated list

`Fate` is CLOSED at four and `VetoRun` CLOSED at three. There is deliberately no `Fate::Ignore`:
a confirmed finding that an adjudicator accepted must land somewhere, and a variant meaning
"drop it" would restore exactly the silent loss being fixed.

## Composition of aggregate returns

`decide` is total: six inputs, four outcomes, no failure mode. Say in the doc which of the six
inputs map to each outcome, as a table, because a reader checking the module against the record
should not have to run it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O. This module decides; the caller escalates or files.
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
