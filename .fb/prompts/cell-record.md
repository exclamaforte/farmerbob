<!-- fb:creates crates/farmerbob-core/src/cell_record.rs -->
# Task: a VOID says the diagonal failed and never says why

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/cell_record.rs` and add `pub mod cell_record;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

Cross-examination runs every candidate's suite against every candidate's code and records one
word per cell: pass, fail, or nocompile. When the diagonal fails -- a suite that cannot run
against the code it shipped with -- the whole matrix is VOIDed, correctly, because the transplant
is broken. The message is:

    VOID: the diagonal is not all-pass -- gemini-38-flash(nocompile)

and that is everything the harness retains. The compiler's own explanation is discarded with the
temporary directory.

Four matrices have been lost this month and each cause took a separate hand investigation:

- a strip that truncated a file on a QUOTED `"#[cfg(test)]"`;
- an import injected twice, `BTreeMap` from a braced group and again from the suite (E0252);
- a full tmpfs, where "Disk quota exceeded" reads as "could not compile";
- a wrapped `use` injected half-written, leaving an unclosed delimiter.

Every one was diagnosable from the first compiler error, and every one cost an hour because the
error was thrown away. This module keeps it, and classifies it well enough that the next VOID
names its own cause.

## Exact API

```rust
/// What one cell of the matrix did, with the evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// The suite ran and every test passed.
    Pass,
    /// The suite ran and some tests failed.
    Fail {
        /// Failing test names, sorted and deduplicated.
        failed: Vec<String>,
    },
    /// The suite did not build.
    NoCompile {
        /// What went wrong, classified.
        why: Breakage,
        /// The first compiler error line, verbatim. Never empty.
        first_error: String,
    },
}

/// Why a cell failed to build. A KNOWN SUBSET: every variant here was a real
/// matrix loss, and an unrecognised error is [`Breakage::Other`], never a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breakage {
    /// Unbalanced delimiters: the graft truncated or half-copied something.
    UnclosedDelimiter,
    /// The same name imported twice (E0252): the injection duplicated an import.
    DuplicateImport,
    /// A name the suite calls does not exist on this implementation (E0599,
    /// E0425, E0433). The one breakage that is a REAL API divergence rather
    /// than an instrument fault.
    MissingItem,
    /// The machine, not the code: no space, quota exceeded, killed.
    Environment,
    /// Recognised as a build failure, cause not classified.
    Other,
}

/// Classify one cell from a `cargo test` invocation's combined output.
pub fn read_cell(output: &str) -> Cell;

/// Whether this breakage indicts the HARNESS rather than either candidate.
///
/// True for everything except [`Breakage::MissingItem`], which is the only
/// variant that says something about the code.
pub fn is_instrument_fault(b: Breakage) -> bool;
```

## Falsifiable clauses

1. Output containing `test result: ok.` and no failures is `Cell::Pass`.
2. Output with failing tests is `Cell::Fail`, carrying the names sorted and deduplicated.
3. Output containing `this file contains an unclosed delimiter` is
   `NoCompile { why: UnclosedDelimiter, .. }`.
4. Output containing `error[E0252]` is `DuplicateImport`.
5. Output containing `error[E0599]`, `error[E0425]` or `error[E0433]` is `MissingItem`.
6. Output containing `Disk quota exceeded`, `No space left on device`, or `Killed` is
   `Environment` -- **even when it also contains `could not compile`**, which it will. Say in
   the doc which wins and why: a build that failed because the disk filled says nothing about
   either candidate, and reporting it as a code fault is how a full tmpfs was once read as an
   arm producing nothing.
7. `first_error` is the FIRST line beginning `error` in the output, verbatim, whitespace
   preserved after trimming the line's own trailing newline. Never empty for `NoCompile`; say
   why it cannot be.
8. `is_instrument_fault` is false ONLY for `MissingItem`. Pin all five variants.
9. Precedence when several markers appear: `Environment` first, then `UnclosedDelimiter`, then
   `DuplicateImport`, then `MissingItem`, then `Other`. Pin at least one pair from each
   adjacent rank.

## Boundaries, at N and at zero

- EMPTY output: `NoCompile { why: Environment, first_error: "(no output)" }`. Pinned, not
  delegated. A cell that produced no output at all did not compile and did not run; the most
  likely cause is the machine (killed, out of memory, no disk) and `Environment` is the variant
  that says "this indicts the harness". `first_error` carries the literal string `(no output)`
  so clause 7's never-empty guarantee holds without inventing a compiler message.
- Output with `error` appearing only inside a test NAME (`fn error_is_reported()`): not a
  compile failure. Clause 7 anchors on the line start; pin that a test name cannot trip it.
- Output with both a `failures:` block and `could not compile`: `NoCompile`, because the build
  is the earlier fact and the failures came from a different crate in the same invocation.
- A `Fail` with zero parseable names: return `NoCompile { why: Other, first_error: .. }` when an
  error line exists, and otherwise `NoCompile { why: Environment, first_error: "(no output)" }`.
  Pinned, not delegated: `Fail { failed: vec![] }` asserts that named tests failed while naming
  none, which is the failure-indistinguishable-from-success shape this crate refuses everywhere
  else.
- Several `error[E....]` lines of different classes: clause 9 decides, not source order.
- `first_error` still comes from the FIRST error line in source order even when clause 9 picks a
  breakage from a later one. The classification and the quoted line answer different questions
  and are allowed to disagree; say so in the doc.

## Superset status on every enumerated list

`Cell` is CLOSED at three and `Breakage` CLOSED at five. The ERROR MARKERS are an OPEN subset --
rustc's text is not a stability guarantee -- so an unrecognised build failure is `Other` and
never `Pass`. Say both halves.

## Composition of aggregate returns

`Fail::failed` is sorted and deduplicated. `NoCompile::first_error` is a single line, never
empty. Neither carries the whole output: this module summarises what a caller should keep, and
the caller decides whether to keep more.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O.
- **Use `testout::failed_tests`, imported**, for clause 2. It is merged and owns that rule.
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
