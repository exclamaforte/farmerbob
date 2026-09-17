<!-- fb:creates crates/farmerbob-core/src/mutate.rs -->
# Task: generate defects mechanically, so suite sensitivity can finally be measured

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/mutate.rs` and add `pub mod mutate;` to `lib.rs`.
Change nothing else.

## Context

This harness ranks test suites by how many known defects they catch. That measure —
defect sensitivity — is the only DIRECT evidence of suite quality; everything else is a proxy.
It ranks third of seven criteria, above survival.

It has been measurable for exactly one task out of forty-eight, because the defects are
hand-written patch files and nobody writes them. The measurement the adjudicator wants most
has almost never been taken.

This module removes the bottleneck: given a Rust source file as a string, produce a
deterministic set of single-edit mutations. A suite that catches many of them is sensitive; a
suite that catches few is shallow, whatever its test count says.

It is pure: no I/O, no filesystem, no compilation. Source arrives as a `&str`.

## What to implement

```rust
/// A kind of single-token edit. These five and no others — the list is EXHAUSTIVE and you
/// must not add a variant.
pub enum MutationKind {
    /// `<` <-> `<=`, and `>` <-> `>=`. Off-by-one at a boundary.
    ComparisonBoundary,
    /// `==` <-> `!=`. Inverted equality.
    ComparisonNegate,
    /// `&&` <-> `||`. Wrong connective.
    BooleanConnective,
    /// `true` <-> `false`. Inverted constant.
    BooleanLiteral,
    /// A standalone integer literal `0` becomes `1`, and any other standalone
    /// integer literal becomes `0`.
    IntegerLiteral,
}

pub struct Mutant {
    /// Stable identifier, exactly `format!("{kind:?}@{byte_offset}")`.
    pub id: String,
    pub kind: MutationKind,
    /// Byte offset into the source where `before` begins.
    pub byte_offset: usize,
    /// The exact text being replaced.
    pub before: String,
    /// The exact text replacing it.
    pub after: String,
}

pub fn mutants(source: &str) -> Vec<Mutant>;
pub fn apply(source: &str, m: &Mutant) -> Option<String>;
```

## What is skipped, and exactly how — this is a PINNED HEURISTIC, not a lexer

Do **not** write a Rust lexer, and do not try to be clever. Implement precisely this, line by
line, so that four independent implementations produce byte-identical output:

1. Split the source into lines on `'\n'`. Track the byte offset at which each line starts.
2. **Skip an entire line** if, after trimming leading whitespace, it starts with `//` or `#[`
   or `*` or `/*`.
3. Within a surviving line, find the first occurrence of `//` **not** preceded on that line by
   a `"` character. Everything from there to the end of the line is ignored.
4. Within the remaining text of the line, ignore any region between an unescaped `"` and the
   next unescaped `"`. A `"` preceded by an odd number of consecutive `\` is escaped. An
   unterminated `"` ignores the rest of the line.
5. **Skip every line from a line whose trimmed form is exactly `#[cfg(test)]` to the end of
   the source.** Mutating a candidate's own tests measures nothing. This is a simple
   truncation, not brace matching: once that marker is seen, stop.

Sites are then found in what survives, by plain substring scanning.

## Site rules, each of which is testable

1. `ComparisonBoundary`: `<=` becomes `<`, `>=` becomes `>`, and a `<` or `>` that is **not**
   part of `<=`, `>=`, `<<`, `>>`, `->`, or `=>` becomes `<=` or `>=` respectively.
2. `ComparisonNegate`: `==` becomes `!=`, and `!=` becomes `==`. A `==` that is part of `===`
   does not occur in Rust and needs no special handling.
3. `BooleanConnective`: `&&` becomes `||`, and `||` becomes `&&`. A `|` or `&` alone is left
   alone.
4. `BooleanLiteral`: the word `true` becomes `false` and `false` becomes `true`, only when
   **delimited** — the characters immediately before and after are absent or are not
   alphanumeric and not `_`.
5. `IntegerLiteral`: a maximal run of ASCII digits, delimited as in rule 4 and additionally
   **not** immediately preceded by `.` and not immediately followed by `.` or by an ASCII
   letter or `_`. A run equal to `"0"` becomes `"1"`; every other run becomes `"0"`. This
   deliberately skips floats, suffixed literals like `3u32`, and tuple accessors like `x.0`.

Longer matches win: at a given offset, `<=` is one `ComparisonBoundary` site, never a `<` site
followed by an `=`. Scanning proceeds past the whole matched token.

## Composition of the returned aggregate — pinned

`mutants` returns every site in **ascending `byte_offset`** order. Two mutants never share a
`byte_offset`, because scanning consumes each matched token. The `Vec` is the complete set of
sites in the surviving text, with no sampling, no cap and no deduplication by kind.

`apply(source, m)` returns `Some(mutated)` when `source[m.byte_offset..]` starts with
`m.before`, replacing exactly that one occurrence at that offset. It returns `None` otherwise
— a mutant that no longer matches its source is not an error to paper over, it means the
source changed under it.

## Boundaries — pinned, including the degenerate cases

- Empty source: `mutants("")` is empty, and so is a source with no sites.
- A source that is entirely a `#[cfg(test)]` module from line one: no mutants.
- `"a < b"` yields one mutant; `"a <= b"` yields one mutant; `"a << b"` yields **none**.
- `"x.0"` and `"3u32"` and `"1.5"` yield no `IntegerLiteral` mutant.
- `"let s = \"a < b\";"` yields no mutant: the `<` is inside a string.
- `"a < b // c > d"` yields exactly one mutant, for the `<`.
- Non-ASCII text is passed through untouched and must not panic or split a character; all
  offsets are byte offsets and must always land on character boundaries.

## Types

Use the concrete types above exactly. Do not make any parameter generic, do not add a trait,
and do not add a dependency. Derive `Debug`, `Clone`, `PartialEq` and `Eq` on both public
types.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`. Slicing a `&str` at a non-character boundary panics; this is
  the main place this task can violate that, so index carefully.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none. This module needs none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them
  yourself and fix any failures before finishing.
- Write a unit test per numbered site rule, per skip rule, and per pinned boundary, named
  after it.

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
