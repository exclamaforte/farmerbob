<!-- fb:creates crates/fb/src/compare_gather.rs -->
# Task: compare_cmd renders a table nobody can feed it

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/compare_gather.rs`. Declare it in `main.rs` with one
`mod compare_gather;` line and change nothing else. `fb` is a binary crate with no `lib.rs`, so
`main.rs` is the only place that line can go.

## Why this exists

`compare_cmd::table` renders a comparison of a task's candidates and takes `&[Row]`. Nothing
builds a `Row`. The figures live in `<task>.score.json`, written by the scorer, and the only
thing that reads them for comparison is `fb-compare.sh`, 47 lines of shell.

The scorer records each figure alongside a flag saying whether it was MEASURED:
`lines`/`lines_measured`, `clippy`/`clippy_measured`, and `tests_run`. A figure whose
`*_measured` flag is false was not read, and rendering it as its numeric placeholder is how a
candidate that wrote code gets recorded as having written none — twelve of them did, which is
`farmerbob-qv6u`.

## Exact API

```rust
/// Read a task's score file into comparison rows.
///
/// Every figure the scorer did not measure becomes `Missing`, never the
/// number sitting beside the flag.
pub fn rows_for(score_path: &Path) -> Result<Vec<Row>, GatherError>;

/// Why a score file could not be turned into rows.
pub enum GatherError {
    /// The file does not exist or could not be read. Carries a non-empty reason.
    Unreadable(String),
    /// The file is not a JSON array of objects. Carries a non-empty reason.
    Malformed(String),
}

/// Read and render. Returns the exit code the caller should use.
pub fn run(score_path: &Path, out: &mut dyn Write) -> i32;
```

`Row` is `crate::compare_cmd::Row`. You do not define it, you do not render anything yourself,
and you do not reimplement the table: `compare_cmd::table` owns it.

## The score file's shape, pinned

Each element is an object. The fields this module reads are EXACTLY these six and no others:

- `source` — the arm name, a string.
- `tests_run` — a number.
- `lines` — a number, and `lines_measured` — a boolean.
- `clippy` — a number, and `clippy_measured` — a boolean.

A `*_measured` flag that is `false`, or absent, makes its figure `Missing`. `tests_run` has no
flag; absent or non-numeric makes it `Missing`.

## Falsifiable clauses

1. An element with `source`, `tests_run`, `lines` + `lines_measured: true`, `clippy` +
   `clippy_measured: true` yields a `Row` with all three figures `Observed` and the arm name
   carried unchanged.
2. `lines_measured: false` makes `lines` `Missing` EVEN WHEN a `lines` number is present beside
   it. Pin it with a non-zero `lines` value — an implementation that reads the number and
   ignores the flag passes clause 1 and fails only here, and that is the defect.
3. The same for `clippy_measured: false`.
4. An ABSENT `lines_measured` is treated as false. Pin it separately from clause 2: absent and
   `false` reach the same answer, and an implementation defaulting a missing flag to true is
   wrong in the dangerous direction.
5. A missing or non-numeric `tests_run` is `Missing`, not zero.
6. `tests_run: 0` with the field present is `Observed(0)`. Pin clauses 5 and 6 as a PAIR: a
   suite that ran nothing and a count that was never read are different facts.
7. Rows come back in the order the array lists them, one per element, none dropped — including
   an element whose every figure is `Missing`.
8. A path that does not exist yields `Unreadable`; a file whose top level is not an array yields
   `Malformed`. Pin that the two differ.
9. `run` returns `compare_cmd::run`'s code for a readable file, and a DIFFERENT, non-zero code
   for each `GatherError`. State which codes you chose in your handoff; the spec pins only that
   the three are distinct from one another.
10. `run` writes nothing but the table on success, and on failure writes a message that mentions
    the path. Assert it CONTAINS the path; do not assert the wording.

## Boundaries, at N and at zero

- An EMPTY array `[]`: `Ok(vec![])`, not an error. A task with no candidates is a readable score
  file, and `compare_cmd::run` already decides what an empty table does.
- An element missing `source` entirely: NOT pinned. Say in your handoff whether you dropped it,
  named it empty, or errored, and do not assert on it.
- An element that is not an object, inside an otherwise valid array: `Malformed`.
- A `lines` value too large for the width `Row` uses: not pinned; say what you did.
- A file containing only whitespace: `Malformed`, not `Unreadable`. It was read.

## Superset status on every enumerated list

The six fields above are EXACTLY what this module reads. The scorer writes twenty-one; reading a
seventh here means this table and `fb brief` can disagree about what a comparison contains.

`GatherError` has EXACTLY two variants.

This module does not measure anything, does not run cargo, and does not render. It reads JSON
the scorer already wrote and builds `Row`s.

## Composition of aggregate returns

`rows_for` returns one `Row` per array element in array order — clause 7 — and each `Row`'s three
figures are independent: one being `Missing` says nothing about the others. Pin a row with
`lines` observed and `clippy` missing.

`run` composes `rows_for` with `compare_cmd::run`: on success its code is that function's,
unchanged, so a caller sees the same answer whether it goes through this or builds rows itself.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. This crate already reads JSON; use what it already uses.
- Tests must write their own fixture files under a temporary path. Do NOT read this repository's
  logs: they are mutable state shared with a running harness.
- Do NOT wire the subcommand into the argument parser. Wiring is a separate task; say so in your
  handoff and use `#![allow(dead_code)]` with a comment if `-D warnings` needs it.
- Derive `Debug, Clone, PartialEq, Eq` on `GatherError`.
- RUN `cargo clippy -p fb --all-targets -- -D warnings` BEFORE you finish, including over your
  tests.
- `cargo test -p fb` and that clippy command must pass. Both are clean on HEAD as of this task,
  so any failure is yours.

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
