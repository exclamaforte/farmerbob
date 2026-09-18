<!-- fb:modifies crates/fb/src/crossx.rs -->
# Task: four merged modules exist to explain a VOID, and the VOID still says nothing

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/crossx.rs`. Do not change any other file.

**A wiring task on a large, live file.** `crossx.rs` is the cross-examination engine and it is
about two thousand lines. You are changing how it records and reports cells, not rewriting it.

## Why this exists

Four modules were specified, critiqued and merged to make a VOIDed matrix explain itself, and
none of them is called:

- `cell_record::read_cell` turns a `cargo test` invocation's output into `Cell::{Pass, Fail,
  NoCompile}`, carrying failing test names or the first compiler error and a `Breakage`
  classification.
- `cell_record::is_instrument_fault` says whether a breakage indicts the harness rather than
  either candidate -- true for everything except `MissingItem`.
- `testout::failed_tests` extracts failing test names.
- `witness::read` and `suite_verdict::classify` tell one disagreement repeated across a field
  from several separate faults, which the N-1-of-N shape heuristic cannot.

Today `crossx.rs` reduces each cell to one word and discards the output. When the diagonal
fails it prints

    VOID: the diagonal is not all-pass -- gemini-38-flash(nocompile)

and nothing else. Four matrices were lost this month and each cause took a separate hand
investigation: a truncating strip, a doubled import, a full tmpfs, a half-copied `use`. Every
one was in the first compiler error line.

## What it must do

1. Where a cell's `cargo test` output is currently reduced to pass/fail/nocompile, ALSO call
   `cell_record::read_cell` and keep the `Cell`.
2. On VOID, print for each failing diagonal cell: the arm, the `Breakage`, whether it is an
   instrument fault, and the `first_error` line verbatim.
3. When every failing diagonal cell is an instrument fault, say so in one line: the matrix was
   lost to the harness, not to any candidate.
4. Build a `witness::Witness` per suite from the `Cell::Fail` names, and print
   `suite_verdict::classify`'s result beside the existing `SUITE` column rather than replacing
   it. Both readings are shown; the old one is not deleted.
5. Keep every existing output line that other code parses. `fb brief`, `fb-pipeline.sh` and the
   adjudicator read this output; additions are safe, removals are not.

## Falsifiable clauses

1. The existing `Matrix`, `Cell` classification into pass/fail/nocompile, `diagonal_bad`,
   `detect_partition`, `compute_stats` and `format_matrix` keep their current behaviour and
   signatures. Adding is permitted; changing what any of them returns is not.
2. A VOID now prints one line per failing diagonal cell naming the arm and the breakage.
3. That line carries `first_error` verbatim, not summarised.
4. **When every failing diagonal cell is an instrument fault, the output says the matrix was
   lost to the harness.** This is the sentence that would have saved four investigations.
5. When at least one failing diagonal cell is `MissingItem`, the output does NOT say that --
   a genuine API divergence is the candidate's, and blaming the harness for it is the mirror of
   the bug being fixed.
6. The `SUITE` column keeps its current values. The `suite_verdict` reading is printed
   ALONGSIDE, labelled, and never overwrites it.
7. A cell whose output cannot be classified yields `Breakage::Other` and is still reported. No
   cell is silently omitted from the VOID report.
8. `fb crossx --json` keeps every field it emits today. New fields may be added.

## Boundaries, at N and at zero

- A matrix with NO failing diagonal cells: no VOID, and none of this output appears.
- A field of ONE candidate: `crossx` already refuses below two; that refusal is unchanged.
- A failing diagonal cell whose output is EMPTY: `read_cell` returns `Environment` with the
  literal `(no output)`; report it like any other rather than skipping it.
- Every cell failing: the report has one line per arm and the harness-fault line appears only if
  every one is an instrument fault.
- `witness::read` over a field where no suite recorded any test names returns `NoEvidence`; print
  that, do not omit the row.

## Superset status on every enumerated list

`Breakage` is CLOSED at five and `suite_verdict::Suite` CLOSED at four; both are defined in
`farmerbob-core`. **Use them, imported. Do not define your own.** The OUTPUT LINES this command
prints are an OPEN set -- other code parses them -- so clause 5 forbids removal and permits
addition.

## Composition of aggregate returns

`run_cmd` keeps its exit code contract exactly. The VOID report is printed in arm order, one
line per failing diagonal cell, and the harness-fault sentence appears at most once, after them.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. Every module named above is already in `farmerbob-core`.
- Tests construct outputs and `Cell` values directly; they may not run `cargo`, spawn git, or
  require a worktree.
- `cargo build -p fb` and `cargo test -p fb` must pass. Run them, and say in your handoff which
  existing tests you touched. The correct answer is "none".
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
