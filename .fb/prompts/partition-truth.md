<!-- fb:modifies crates/fb/src/crossx.rs -->
# Task: the verdict that outranks every defect has never once been right

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/crossx.rs`. Do not change any other file.

## Why this exists

Cross-examination prints one verdict that outranks everything else it computes:

    SPEC-AMBIGUOUS: the field partitions into self-consistent camps -- {a,b} | {c} | {d}
    Each camp passes within itself and fails across ...
    Treat every cross-camp failure as vetoed and fix the specification.

Nine tasks have carried it. All nine were re-checked against the property the message itself
asserts, by testing every cross-camp cell in both directions. **All nine are false.** There is
no true positive on record, and six of the nine were merged with their cross-examination signal
suppressed by an instrument that could not fail.

`detect_partition` builds `camp(a)` as the set of SUITES that `a`'s implementation passes, then
prints the groups as camps of ARMS. Those are different objects and they are not disjoint: a
suite that passes everything sits inside every arm's camp-set, and the validity loop then skips
exactly the cells that would refute the partition --

```rust
for other in arms {
    if camp.contains(other) { continue; }   // <- skips the refuting cell
    if cells.get(m, other) != Some(Cell::Fail) { return None; }
}
```

The sparser the failures, the more is skipped. `window-state` has ONE failing cell in sixteen
and still gets the verdict.

`farmerbob_core::field_shape::shape` was merged for this and enforces the property directly,
in both directions, with `(Some(Fail), Some(Fail))`. It has no caller. (bead farmerbob-gzxg)

## Exact API

`detect_partition` keeps its signature and its output format, and stops computing the answer
itself:

```rust
use farmerbob_core::crossx::{Cell, Matrix};
use farmerbob_core::field_shape::{self, Shape};

/// The partition shape, or `None` when the field does not partition.
///
/// Delegates to [`farmerbob_core::field_shape::shape`]. This function formats;
/// it does not decide.
pub fn detect_partition(arms: &[String], cells: &Matrix) -> Option<String>;
```

It returns `Some` when and only when `field_shape::shape(cells)` is
`Measurement::Observed(Shape::Partition { camps })`, and the string is those camps rendered in
the format the caller already prints. Every other shape, and every `Measurement::Missing`,
is `None`.

**Delete the camp-building and validity code.** A second copy of this rule is how the first one
survived nine false verdicts, and leaving it behind as dead code is the same defect one step
removed.

## Falsifiable clauses

1. `detect_partition` returns `Some` exactly when `field_shape::shape` observes
   `Shape::Partition`, and `None` for `Unanimous`, `MutualRejection`, `Isolated`, `Mixed` and
   for every `Missing`.
2. The rendered string is unchanged in shape: camps joined by `" | "`, each camp `{` its
   members joined by `","` `}`. `field_shape` already pins members sorted by name and camps
   sorted by first member, so two runs over one matrix render identically.
3. **The nine historical matrices all return `None`.** Each is 4x4, row-major, rows are
   implementations and columns are suites, `P`=Pass `F`=Fail. Pin every one as a test:

       availability   PPFP PPFP FFPF PFFP
       budget         PFPF PPPF PFPF FFFP
       field-shape    PFPP PPPF PFPF PFPP
       gate           PPPP PPPF PPPP PPPP
       matrix         PPPP PPPP PFPF PPPP
       precondition   PFPP PPPP PFPP PPPP
       resume         PFFP FPFF PFPP PFFP
       scope          PPFP FPFF PPPP PPFP
       window-state   PPPP PPPF PPPP PPPP

   These are real tables this harness printed. A test that does not make them fail the old
   implementation is not testing the fix.
4. A GENUINE partition still returns `Some`: `PPFF PPFF FFPP FFPP` over `["a","b","c","d"]`
   renders `{a,b} | {c,d}`.
5. A matrix that differs from clause 4 by ONE cross-camp cell flipped to `Pass` returns `None`.
   That single cell is the whole difference between the two, and it is what nine verdicts
   missed.
6. The `arms.len() < 4` guard is REMOVED, and removing it changes nothing at three arms,
   because `field_shape` checks `Isolated` BEFORE `Partition` and a three-arm split is always
   two-plus-one. The lone arm fails every foreign suite in both directions, which is exactly
   `Isolated`, so `shape` never returns `Partition` at three arms and `detect_partition`
   returns `None` there. Pin that: a three-arm two-plus-one split returns `None`, and say in
   your handoff that it is `Isolated` rather than "not a partition", because those are
   different facts. A two-arm mutual rejection also returns `None`.

   The guard still goes. It was a proxy for the broken check, and leaving a floor that
   `field_shape` already enforces is a second copy of the rule -- which is the whole subject of
   this task.
7. `arms` is no longer used to compute the answer. If it is still needed to render, say what
   for; if it is not, say so in your handoff rather than silently keeping a parameter that does
   nothing.

## Boundaries, at N and at zero

- ZERO arms and ONE arm: `field_shape::shape` returns `Missing`, so `None`.
- TWO arms: never `Some`, whatever the cells say. Clause 6.
- THREE arms: never `Some` either, and for a reason worth stating -- `Isolated` claims the
  shape first. Clause 6.
- FOUR arms: the smallest field in which `Some` is reachable at all.
- Every off-diagonal cell `Cell::Error`: `Missing`, so `None`.
- SOME cells `Error`: `field_shape` excludes them from every denominator and its answer governs.
  Do not re-derive that rule here; assert the behaviour, not the mechanism.
- A matrix whose diagonal is not all-`Pass`: the VOID check upstream already returns before this
  function is reached. Do not add a second diagonal check; say in your handoff that you relied
  on the caller.

## Superset status on every enumerated list

`field_shape::Shape`'s variants are a CLOSED set of exactly five: `Unanimous`,
`MutualRejection`, `Partition`, `Isolated`, `Mixed`. You define none of them and your tests may
not assert on any shape outside them. `Cell`'s three and `Absent`'s four are likewise core's.

## Composition of aggregate returns

The returned `String` is a rendering of `Shape::Partition { camps }` and contains exactly the
camps that variant carries, in the order it carries them, with no camp dropped, merged or
re-sorted here. Ordering is `field_shape`'s to pin and this function must not re-establish it.

Say why clause 4 and clause 5 must be tested as a PAIR rather than separately: clause 4 alone
passes against an implementation that returns `Some` for everything, and clause 5 alone passes
against one that returns `None` for everything. The nine historical matrices in clause 3 are the
same argument at scale, and an implementation that satisfies all three cannot be either.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. `farmerbob-core` is already a dependency and `field_shape` is already in
  it; verify that for yourself rather than taking this line's word for it.
- `Matrix::new` returns `Result`; handle it. Inside `#[cfg(test)]` an `unwrap` is permitted.
- `cargo test -p fb` must pass.
- **The clippy criterion is NOT MEASURABLE on this crate and no candidate will be ranked by it.**
  `cargo clippy -p fb -- -D warnings` fails on seven errors that predate this task -- dead code
  in `critique.rs`, `prove.rs` and `import.rs`, and three empty-format-string literals in
  `select.rs` and `main.rs`. Fixing them is a departure from the declared scope and scores as
  one, so the bar is unreachable for everyone equally and is therefore not a discriminator. The
  scored list below still names clippy; read this line as the measurement that list cannot take
  here, exactly as it says under "A signature that cannot compute what the spec promises".
  Report that your own file adds none. (bead farmerbob-hm4j)

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus, ONLY when the task
  CREATES a new file, the one `pub mod y;` line in the `lib.rs` beside it. A task that MODIFIES
  an existing file touches one file and no other. Not "only that crate" --
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
