<!-- fb:creates crates/fb/src/verify_cmd.rs -->
# Task: verify_plan is merged, tested, and no shell script can reach it

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/verify_cmd.rs`. Declare it in `main.rs` with one `mod verify_cmd;` line
and change nothing else. `fb` is a binary crate with no `lib.rs`, so `main.rs` is the only place
that line can go.

## Why this exists

`farmerbob_core::verify_plan` decides what should happen to a candidate worktree, and it exists
because `fb-verify.sh` collapsed four different facts into one pass/fail. A candidate that never
wrote the deliverable, one whose build broke, and one whose suite the harness killed at the
timeout all printed the same thing -- and the first is a no-op by the ARM, the second a defect in
the arm's CODE, and the third a property of the MACHINE. Treating a harness timeout as a
candidate's failure is how an arm loses a run it did not lose.

It is merged, tested, and reachable from nothing. `fb-verify.sh` is still 107 lines of shell
making the decision itself, because there is no subcommand to call.

This task builds the callable layer. It does NOT change `fb-verify.sh`; that is a separate task,
and it is the one that finally deletes the shell copy.

## Exact API

```rust
/// What the caller observed about one candidate, as strings from the shell.
pub struct Raw {
    /// Arm name.
    pub arm: String,
    /// Whether the declared deliverable exists and is non-empty.
    pub deliverable: bool,
    /// Whether the build command exited 0. `None` when no build was attempted.
    pub built: Option<bool>,
    /// The test log, read verbatim. Empty when no suite ran.
    pub test_log: String,
    /// Whether the harness cut the run short.
    pub timed_out: bool,
}

/// Render one candidate's fate as a line for an operator, and a code for a script.
pub struct Line {
    /// The rendered line. Never empty, never ends in a newline.
    pub text: String,
    /// 0 measurable, 1 failed, 2 no-op, 3 cut.
    pub code: i32,
}

/// Decide and render one candidate.
pub fn assess(r: &Raw) -> Line;

/// Assess a field, in the order given.
///
/// The returned code is the WORST of the field by the ordering in `Line::code`,
/// or 0 when the field is rankable.
pub fn assess_field(raws: &[Raw]) -> (Vec<Line>, i32);
```

## Falsifiable clauses

1. `built: Some(true)` with a test log the core reads as passing, `deliverable: true`, not timed
   out: code 0, and `text` names the arm and mentions the passing count.
2. `timed_out: true` yields code 3 WHATEVER else is observed. Pin it against a `Raw` that would
   otherwise be code 0 and one that would otherwise be code 1. This is the clause the task
   exists for, and an implementation that checks the build first fails it.
3. `deliverable: false` with `built: None`: code 2.
4. `built: Some(false)`: code 1.
5. Every `text` is non-empty and ends with no newline, for all four codes. Pin as ONE test over
   all four, not a remark on each.
6. `text` mentions the arm name, for all four codes. Assert it CONTAINS the arm name; do not
   assert the sentence or the column layout, neither of which this spec fixes.
7. The build-versus-test decision is `farmerbob_core::build_verdict::read(built, test_log)`, and
   the fate is `farmerbob_core::verify_plan::fate`. Assert that `assess`'s code agrees with
   `verify_plan::fate` on every input, rather than re-deriving the rule. A second copy of the
   rule in this file is the defect this task exists to remove.
8. `assess_field` returns one `Line` per input, in input order, same length.
9. Its code is 0 when at least TWO candidates are code 0, because that is what
   `verify_plan::rankable` means, and non-zero otherwise. Pin one-measurable (non-zero) against
   two-measurable (0) -- a field that looks like two is not two.
10. A field of all-cut candidates returns a non-zero code and every line is code 3. Pin that the
    field code is not silently 0 because nothing "failed".

## Boundaries, at N and at zero

- ZERO candidates: `assess_field(&[])` returns an empty vector and a NON-ZERO code. An empty
  field is not a rankable field. Pin the emptiness and the code together.
- ONE measurable candidate: lines has one entry with code 0, and the field code is non-zero.
- An empty `test_log` with `built: Some(true)`: whatever `build_verdict::read` makes of it,
  `assess` agrees. Say in your handoff what that turned out to be and do not assert a value you
  did not first confirm against `build_verdict`.
- An empty arm name: not pinned. Say what you did and do not assert on it.

## Superset status on every enumerated list

The codes are EXACTLY 0, 1, 2 and 3, matching the four `verify_plan::Fate` variants one to one.
Adding a fifth is a defect.

This module does NOT decide anything. `build_verdict` reads the log, `verify_plan` decides the
fate, and this renders. A `match` on test-log text, or on an exit code, is the re-derivation
clause 7 forbids.

## Composition of aggregate returns

`assess_field` returns a PAIR: one line per candidate in input order, and one field code. The
lines are independent of the field code -- pin that a field whose code is non-zero still returns
every candidate's line, including the code-0 ones, because an operator reading a refused field
needs to see which candidates were fine.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies.
- Do NOT wire the subcommand into the argument parser: that would change `main.rs` beyond the
  one `mod` line. Wiring is a separate task. Say in your handoff that you left it unwired, and
  use `#![allow(dead_code)]` with a comment if the crate's `-D warnings` build needs it -- an
  uncalled `pub fn` in a BINARY crate trips `dead_code`, which has already cost one arm a clean
  clippy score.
- Derive `Debug, Clone, PartialEq, Eq` on every type in the Exact API. This is pinned because a
  suite asserting with `assert_eq!` fails to COMPILE against a rival that omits them.
- `cargo test -p fb` and `cargo clippy -p fb -- -D warnings` must pass. Both are clean on HEAD
  as of this task, so any failure is yours.

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
