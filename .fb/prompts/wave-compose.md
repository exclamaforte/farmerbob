<!-- fb:creates crates/farmerbob-core/src/wave_compose.rs -->
# Task: a wave manifest is validated by four separate scripts, each partially

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/wave_compose.rs`. Declare it in `lib.rs` with one
`pub mod wave_compose;` line and change nothing else.

## Why this exists

A wave manifest is a TSV: task, crate, comma-separated arms. Four things must be true of it
before dispatch, and today each is checked somewhere different, or not at all:

- **Distinct tasks.** Two rows naming the same task race for the same worktree prefix.
- **Distinct targets.** `park-decision` and `park-scope` both declared `quota.rs`; the second
  branched from a base without the first, and the merge had to be done by hand as a graft.
  Serial dispatch hid it; parallel dispatch makes it routine.
- **An arm appears at most once per task.** The same arm twice on one task is two runs writing
  one worktree.
- **The manifest is not empty.**

`fb-autopilot.sh` checks target collision against LIVE waves — and until this evening it read
only row 1, so six of seven rows went unchecked. Nothing checks a wave against ITSELF.

## Exact API

```rust
/// One row of a wave manifest.
pub struct Row {
    pub task: String,
    pub crate_name: String,
    /// In manifest order. May contain repeats; that is what clause 4 is about.
    pub arms: Vec<String>,
    /// The file this task declares, as the spec's `fb:creates` or `fb:modifies`
    /// marker gives it. `None` when the spec declares none.
    pub target: Option<String>,
}

/// A reason a manifest may not be dispatched.
pub enum Fault {
    /// The manifest has no rows.
    Empty,
    /// Two rows name the same task. Carries the task and the two row indexes,
    /// lower first.
    DuplicateTask { task: String, rows: (usize, usize) },
    /// Two rows declare the same target. Carries the target and the two row
    /// indexes, lower first.
    TargetCollision { target: String, rows: (usize, usize) },
    /// One row lists an arm twice. Carries the row index and the arm.
    RepeatedArm { row: usize, arm: String },
    /// A row lists no arms at all.
    NoArms { row: usize },
}

/// Every fault in a manifest, in the order defined below.
///
/// Returns an empty vector when the manifest may be dispatched.
pub fn faults(rows: &[Row]) -> Vec<Fault>;

/// Whether the manifest may be dispatched.
pub fn dispatchable(rows: &[Row]) -> bool;

/// How many runs a manifest will produce: the total number of arms across
/// every row, counting repeats.
pub fn run_count(rows: &[Row]) -> usize;
```

## The order faults are reported, pinned

`Empty` first and alone: an empty manifest has no rows to report anything else about.
Otherwise, faults are reported in order of the LOWEST row index they mention; two faults
mentioning the same lowest row are ordered `DuplicateTask`, `TargetCollision`, `RepeatedArm`,
`NoArms`. This is pinned because it is not the arm's to choose.

## Falsifiable clauses

1. A manifest of three rows with distinct tasks, distinct targets and one arm each yields NO
   faults, and `dispatchable` is true.
2. Two rows naming the same task yield `DuplicateTask` carrying that task and both indexes,
   lower first.
3. Two rows declaring the same target yield `TargetCollision`, carrying the target and both
   indexes, lower first.
4. A row listing the same arm twice yields `RepeatedArm` naming that row and that arm — ONCE,
   not once per extra occurrence. An arm listed three times is still one fault.
5. `target: None` NEVER collides, with anything, including another `None`. Pin two rows both
   carrying `None`: no fault. A task that declares no target is not a task that declares the
   same target as everything else, and that confusion would refuse every legitimate wave.
6. A row with an empty `arms` vector yields `NoArms` naming that row.
7. `dispatchable` is true exactly when `faults` is empty. Pin the agreement on one clean
   manifest and one faulty one, not each function alone.
8. `run_count` counts every arm in every row, INCLUDING repeats, and is defined even for a
   manifest that is not dispatchable. Pin it on a manifest with a repeated arm.
9. Three rows sharing one target produce faults for the pairs (0,1), (0,2) and (1,2) — every
   pair, not just the first. Pin the count and the pairs, because reporting only the first
   collision hides the third row from whoever has to fix the manifest.
10. Faults are reported in the pinned order above. Pin one manifest carrying at least three
    different fault kinds and assert the whole sequence.

## Boundaries, at N and at zero

- ZERO rows: exactly one fault, `Empty`, and nothing else. `run_count` is 0 and `dispatchable`
  is false.
- ONE row, well formed: no faults, dispatchable. A wave of one is a wave.
- ONE row with one arm listed twice: one `RepeatedArm`. Not dispatchable.
- TWO rows, identical in every field including task and target: `DuplicateTask` AND
  `TargetCollision`, both for rows (0,1), in that order. Pin both — an implementation that
  returns after the first fault it finds passes clauses 2 and 3 separately and fails here.
- An empty task name, or an empty target string: NOT pinned. Two rows with `target:
  Some("")` — say in your handoff whether you call that a collision and do not assert on it.

## Superset status on every enumerated list

`Fault` has EXACTLY five variants. Closed set.

This module does NOT read the filesystem, parse TSV, or resolve a task name to its target —
the `target` field arrives already resolved. Parsing a manifest here is a defect.
`farmerbob_core::admit_queue` already flattens a manifest into runs and applies registry
eligibility; this module runs BEFORE it and answers a different question. Do not re-derive its
rules.

## Composition of aggregate returns

`faults` returns EVERY fault, not the first: clause 9 is the statement, and clause 10 fixes the
order. The vector is not deduplicated — three rows sharing a target give three pair faults.

`run_count` is independent of `faults`: pin that a manifest with faults still reports its run
count, because a caller reporting "this wave would have launched N runs" needs it precisely
when the wave is refused.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem, no `std::process`.
- Derive `Debug, Clone, PartialEq, Eq` on every type in the Exact API. This is pinned because a
  suite asserting with `assert_eq!` fails to COMPILE against a rival that omits them, which
  forfeits the cross-examination cell. (bead farmerbob-9ci5)
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
