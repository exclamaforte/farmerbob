<!-- fb:creates crates/farmerbob-core/src/verify_plan.rs -->
# Task: fb-verify decides four things in shell and reports three of them as one

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/verify_plan.rs`. Declare it in `lib.rs` with one
`pub mod verify_plan;` line and change nothing else.

## Why this exists

`fb-verify.sh` is 107 lines of pure shell with nothing in Rust behind it. It decides whether a
candidate worktree is worth measuring at all, and the decision has four inputs that it collapses
into a pass/fail: whether the crate BUILDS, whether its tests RUN, whether the run was cut short
by a timeout, and whether the arm produced the declared deliverable at all.

Three of those four failures print the same thing. A candidate that never wrote the file, one
whose build broke, and one whose suite was killed at the timeout all surface as "did not pass
the gate" -- and the first is a no-op by the arm, the second is a defect in the arm's code, and
the third is a property of the MACHINE. Treating a harness timeout as a candidate's failure is
how an arm loses a run it did not lose.

`farmerbob_core::build_verdict` already decides build-versus-test from compiler output and is
merged. This module is the layer above it: given that verdict plus the other three facts, what
should happen to the candidate.

## Exact API

```rust
/// What the harness observed about one candidate worktree.
pub struct Observed {
    /// Whether the declared deliverable exists and is non-empty.
    pub deliverable: bool,
    /// The build-or-test verdict, already decided by `build_verdict`.
    /// `None` when the build was never attempted.
    pub build: Option<BuildVerdict>,
    /// Whether the run was cut short by the harness's own timeout.
    pub timed_out: bool,
}

/// What should happen to a candidate.
pub enum Fate {
    /// Measure it: it built, its suite ran, and it wrote the deliverable.
    Measure { tests_passed: u32 },
    /// The arm wrote nothing. This is a no-op BY THE ARM.
    NoOp,
    /// The arm's code does not build or its suite fails. A defect IN THE ARM.
    Failed(String),
    /// The harness cut the run short. NOT the arm's failure, and it must never
    /// be reported as one. Carries what was observed before the cut.
    Cut { had_deliverable: bool },
}

/// Decide one candidate's fate.
pub fn fate(o: &Observed) -> Fate;

/// How many candidates of each fate a field contains.
pub struct Field {
    pub measurable: usize,
    pub no_op: usize,
    pub failed: usize,
    pub cut: usize,
}

/// Tally a field.
pub fn field(fates: &[Fate]) -> Field;

/// Whether a field can be ranked at all.
///
/// Ranking needs at least two measurable candidates: one candidate is not a
/// comparison, and zero is not a field.
pub fn rankable(f: &Field) -> bool;
```

`BuildVerdict` is `farmerbob_core::build_verdict`'s, and it is exactly:

```rust
pub enum BuildVerdict {
    Passed { passed: u32 },   // built, at least one test ran and passed
    Failed { failed: u32 },   // built, tests ran, at least one failed
    NoTests,                  // built, no test executed
    BuildFailed,              // did not build
}
```

You do not define it and you do not re-derive it. The passing count comes from
`Passed { passed }`; there is no second count anywhere in this module.

## Falsifiable clauses

1. `timed_out: true` yields `Cut`, WHATEVER else is observed. Pin it against an observation that
   would otherwise be `Measure` and against one that would otherwise be `Failed`. A timeout is
   the machine's, and the two cases must not collapse.
2. `Cut { had_deliverable }` reports whether the deliverable existed at the moment of the cut.
   Pin both values.
3. `deliverable: false`, not timed out, no build attempted: `NoOp`.
4. `deliverable: false` while the build SUCCEEDED and tests passed: still `NoOp`. Writing tests
   without the deliverable is not a deliverable. Pin it — this is the case an implementation
   keyed on the build verdict alone gets wrong.
5. A build failure: `Failed`, with a non-empty reason. Assert the reason is non-empty and that it
   mentions building, case-insensitively; do NOT assert the sentence.
6. A test failure: `Failed`, non-empty reason mentioning tests, case-insensitively.
7. Clauses 5 and 6 must produce DIFFERENT reasons. Pin that they differ, not their wording:
   build-broken and tests-failing need different fixes and the shell prints one line for both.
8. `Measure` only when the deliverable exists and the build is `Passed { passed }`. It carries
   that same `passed` count, unchanged.
9. `BuildVerdict::NoTests` with an otherwise perfect observation is `Failed`, NOT
   `Measure { tests_passed: 0 }`. A suite that executed nothing is not a suite that passed
   nothing, and the rubric's gate requires "at least one test that actually executes". Pin
   clauses 8 and 9 as a PAIR, because an implementation mapping NoTests to a zero count passes
   clause 8 alone.
10. `field` counts each fate in exactly one bucket and the four fields sum to the input length.

## Boundaries, at N and at zero

- ZERO candidates: `field(&[])` is all-zero and `rankable` is FALSE.
- ONE measurable candidate: `rankable` is FALSE. One candidate is not a comparison.
- TWO measurable: `rankable` is TRUE. This is the boundary and both sides must be pinned.
- TWO candidates of which one is `Cut`: `rankable` is FALSE, because only one is measurable.
  Pin it — a field that looks like two is not two.
- `BuildVerdict::Passed { passed: 0 }` — built, the suite ran, and zero tests passed: NOT
  pinned by this spec. `build_verdict` documents `Passed` as "at least one test ran and
  passed", so the value may be unreachable. Say in your handoff which fate you return and do
  NOT assert on it.
- `BuildVerdict::Failed { failed: 0 }`: same instruction. Not pinned, not asserted.
- `timed_out: true` with `deliverable: false` and no build: `Cut { had_deliverable: false }`,
  not `NoOp`. Clause 1 is unconditional.

## Superset status on every enumerated list

`Fate` has EXACTLY four variants and `Field` exactly four counters. Closed sets.

This module does NOT decide build-versus-test from compiler output. `build_verdict` does that
and is already merged; re-deriving it here is a defect. This module consumes its answer.

## Composition of aggregate returns

`Field`'s four counters partition the input — clause 10 — and `rankable` reads ONLY
`measurable`. Pin that a field with many failures and two measurable candidates is rankable, so
that `rankable` is not accidentally implemented as "nothing went wrong".

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem, no `std::process`: every observation arrives as a value.
- Derive `Debug, Clone, PartialEq, Eq` on every type in the Exact API. This is pinned because a
  suite asserting with `assert_eq!`/`assert_ne!` fails to COMPILE against a rival that omits
  them, which forfeits the cross-examination cell. (bead farmerbob-9ci5)
- `cargo test -p farmerbob-core` and `cargo clippy -p farmerbob-core -- -D warnings` must pass.
  Both are clean on HEAD as of this task, so any failure is yours.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus `.fb/handoff.md`, which
  the Handoff section below REQUIRES you to write and which is exempt from this rule; plus,
  ONLY when the task
  CREATES a new file, the one `mod y;` line that declares it, in EITHER the `lib.rs` or the
  `main.rs` beside it. `farmerbob_core::scope::module_declarations_for` permits both, and a
  binary crate such as `fb` has no `lib.rs`, so `main.rs` is the only place the declaration can
  go. A task that MODIFIES an existing file touches one file and no other. Not "only that
  crate" --
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
4. **Test depth and generality** — and read this carefully, because it says something
   different from what it used to say.

   **Test COUNT is not a criterion and never was.** The adjudicating module refuses to rank on
   it, and it is right to: a table-driven test that pins nine behaviours scores one, and a
   suite of forty that restate one scores forty. This rubric claimed for weeks that count was
   scored fourth while the code scored it never, and every arm was told the wrong thing.
   (bead farmerbob-0ce)

   What IS scored is whether your tests would FAIL a wrong implementation. The instruments for
   that are defect injection and running your suite against a rival's code; when neither has
   been run, this criterion is `Missing` and ranks nobody. It is not silently replaced by a
   count.

   So write the tests that falsify the spec's clauses, however few that takes. Test the
   behaviour the specification requires, not your particular implementation's internals.
   Asserting on exact error strings, private field names, or an output format the spec does
   not fix makes a test worthless — and, when a rival's code is run against your suite, makes
   it worse than worthless, because it fails a correct implementation.
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
