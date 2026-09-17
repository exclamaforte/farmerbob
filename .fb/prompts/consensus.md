<!-- fb:modifies crates/farmerbob-core/src/matrix.rs -->
# Task: a matrix where everything passes is not a result

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
`crates/farmerbob-core/src/matrix.rs` ALREADY EXISTS and classifies a cross-examination
matrix. Read it first. Modify it in place and change nothing else.

## Context

N candidate implementations of one specification are cross-examined: every candidate's test
suite is run against every other candidate's implementation. The existing `shape` function
reads the resulting grid and names what it sees — a defect, an over-fitted suite, a
spec-ambiguous partition, or consensus.

`Consensus` is currently reported like any other shape, and that is wrong in a way that has
already cost this project a real defect. When every cell passes, the only instruments in the
matrix are the candidates' own suites, and a fault shared by *all* of them is invisible to
every one of them. The field agrees with itself for exactly the wrong reason.

It happened: four implementations of a route-identifier canonicaliser all matched their
markers case-sensitively, all sixteen cells passed, and a mixed-case identifier would have
silently defeated the deduplication the module existed to perform. Two human-directed critics
found it; the matrix could not.

So a consensus matrix carries **no information about correctness**. It says the arms agree. It
cannot say whether they are right. This task makes the type say that.

## What to change

Add one variant to the existing `Shape` enum, add one function, and change nothing else about
how the other shapes are classified.

```rust
pub enum Shape {
    // ... every existing variant stays exactly as it is, with its existing fields ...

    /// Every off-diagonal cell passed. The arms agree; whether they are correct is
    /// UNKNOWN and no cell in this matrix bears on it.
    Consensus {
        /// Arms in the matrix, sorted ascending. Never empty when this variant is returned.
        arms: Vec<String>,
    },
}

/// Whether a shape's classification rests on evidence that could have contradicted it.
///
/// `false` for `Consensus` and for `Void`, `true` for every other variant.
pub fn is_discriminating(shape: &Shape) -> bool;
```

If `Shape::Consensus` already exists in the file, extend it with the `arms` field rather than
adding a second variant; if it carries no fields today, add exactly that one.

## Rules, each of which is testable

1. `shape` returns `Consensus { arms }` when the matrix is non-empty, the diagonal is entirely
   `Pass`, and every off-diagonal cell is `Pass`. `arms` holds every arm name in the matrix,
   sorted ascending by byte order, with no duplicates.

2. The `Void` check still runs FIRST and still wins. A matrix whose diagonal is not entirely
   `Pass` is `Void` even if every off-diagonal cell passed — a suite that cannot run against
   the code it shipped with means the transplant is broken, and a broken instrument's
   agreement is worth less than nothing.

3. A single off-diagonal `Fail` anywhere prevents `Consensus`, and the matrix is classified by
   the existing rules exactly as it is today. This task changes no other classification.

4. An off-diagonal `NoCompile` prevents `Consensus`. Arms that could not be compiled against
   one another did not agree; they failed to be comparable, which the existing shapes already
   express. `Consensus` requires actual passing cells.

5. `is_discriminating(shape)` is `false` for `Consensus` and for `Void`, and `true` for every
   other variant of `Shape`. **The `Shape` enum is exhaustive as it exists after your change —
   do not add variants beyond the one specified, and write this function with no wildcard
   `_ =>` arm, so that a future variant is a compile error here rather than a silent `true`.**

## Composition of the returned aggregate — pinned

`Consensus.arms` is sorted ascending by byte order and contains each arm exactly once,
regardless of the order the arms appear in the matrix. It is the same set the matrix was built
over, neither filtered nor reordered by pass/fail status — every cell passed, so there is no
status to sort by.

## Boundaries — pinned, including the degenerate cases

- A 1x1 matrix with a passing diagonal and no off-diagonal cells at all: this is
  `Consensus { arms }` with one arm. One arm agreeing with itself is vacuous agreement, and
  `is_discriminating` returning `false` is exactly the right reading of it.
- A 0-arm matrix: whatever the file returns today for an empty matrix, it must keep returning.
  Do not change the empty case; if it is `Consensus` today it stays `Consensus`, with `arms`
  empty, and rule 1's "never empty" applies only to the non-empty matrices it describes.
- A 2x2 matrix where both off-diagonal cells pass: `Consensus` with two arms.
- A matrix that is entirely `NoCompile` off the diagonal, diagonal passing: NOT `Consensus`,
  by rule 4.

## Types

Use the concrete types above exactly. Do not make any parameter generic, do not introduce a
new error type or trait, and do not alter the `Matrix` or `Cell` types. Keep every existing
derive on `Shape`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- Every test already in `matrix.rs` must still pass. If one matches on `Shape` exhaustively or
  constructs a `Consensus` literal, update it for the new field; do not delete or weaken an
  existing assertion.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them
  yourself and fix any failures before finishing.
- Write a unit test per numbered rule above, named after it, plus one per pinned boundary.

When done, briefly state what you implemented.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- only the crate named in the task is modified

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
   it. Count is the fallback, not the target:
   assertions. Your tests must be good enough to catch a bug in **any** correct-looking
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

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
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
