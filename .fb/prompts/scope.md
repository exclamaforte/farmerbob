<!-- fb:creates crates/farmerbob-core/src/scope.rs -->
# Task: decide whether a run stayed inside its declared deliverable

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/scope.rs` and add `pub mod scope;` to `lib.rs`.
Change nothing else.

## Context

Every task in this harness declares exactly one deliverable — one file path. An arm that
edits other files has gone out of scope, and that matters for two reasons: the extra changes
are unreviewed, and they can make a rival's test suite fail for reasons unrelated to the task.

The harness currently measures scope by counting distinct *crates* touched. One arm modified
36 files across a crate it was never asked to touch, and that registered as "touched 2". The
count was not wrong; it was measuring the wrong thing.

This module decides the question properly, from paths alone. It is pure: no I/O, no process
execution, no filesystem access. Paths arrive as strings.

## What to implement

```rust
pub struct Declared {
    /// The one file the task told the arm to produce, repo-relative,
    /// forward slashes, no leading "./". Example: "crates/farmerbob-core/src/scope.rs"
    pub target: String,
}

pub enum Allowance {
    /// A path that is permitted even though it is not the target.
    ModuleDeclaration,
}
// A critic observed that this enum carries no information today -- it has one fieldless
// variant, `assess` discards it, and `Option<String>` would say the same thing. That is a
// fair reading and it is deliberate: the allowance list is where a future task-kind adds its
// own permitted paths (a generated file, a fixture directory), and a one-variant enum is the
// cheapest honest place to put the second variant later. Keep it as an enum.

pub enum Departure {
    /// Changed a file that is neither the target nor allowed.
    Foreign { path: String },
    /// Deleted a file it was not asked to touch.
    Deleted { path: String },
}

pub struct Scope {
    /// True when the target itself was changed.
    pub target_changed: bool,
    /// Permitted non-target paths that were changed, sorted ascending, no duplicates.
    pub allowed: Vec<String>,
    /// Violations, sorted by path ascending, no duplicates.
    pub departures: Vec<Departure>,
}

pub struct Change {
    pub path: String,
    pub deleted: bool,
}

pub fn assess(declared: &Declared, changes: &[Change]) -> Scope;
pub fn is_clean(scope: &Scope) -> bool;
pub fn module_declaration_for(target: &str) -> Option<String>;
```

## Rules, each of which is testable

1. `module_declaration_for(target)` returns the `lib.rs` that sits in the **same directory**
   as `target` — for `"crates/x/src/y.rs"` it is `Some("crates/x/src/lib.rs")`. It returns
   `None` when `target` is itself a `lib.rs`, when `target` has no `/`, and when `target` is
   empty. It does not check whether the file exists; this module performs no I/O.

2. A change whose `path` equals `declared.target` exactly, and has `deleted == false`, sets
   `target_changed = true` and appears in neither `allowed` nor `departures`.

   **If the target appears BOTH deleted and not deleted, the deletion wins:**
   `target_changed = false` and one `Departure::Deleted` is emitted. This matches the
   deleted-wins rule pinned for `departures` below. An earlier version of this spec stated
   rule 2 and rule 5 without saying which governs when both changes are present, and two
   implementations resolved the contradiction in opposite directions -- caught by
   cross-examination and independently by a critic, who noted the spec "pins deleted-wins
   dedup only for `departures`, so this is genuinely underdetermined".

3. A change whose `path` equals `module_declaration_for(&declared.target)` is
   `Allowance::ModuleDeclaration`: it goes in `allowed`, not in `departures`. Adding
   `pub mod y;` to the crate's own `lib.rs` is required to make a new module compile, so it
   is not a violation. **This is the only allowance.** The `Allowance` enum is exhaustive as
   listed — there are no other variants and you must not add any.

4. Every other changed path is a `Departure`. A change with `deleted == true` is
   `Departure::Deleted`; otherwise `Departure::Foreign`. This applies even to the target's own
   `lib.rs`: deleting it is a violation, not an allowance.

5. Deleting the **target itself** is `Departure::Deleted` and leaves `target_changed = false`.
   A task asks for a file to exist; removing it is not producing it.

6. `is_clean(scope)` is `true` if and only if `scope.departures` is empty. It does **not**
   require `target_changed`. A run that produced nothing at all is out of the gate's scope
   question, not this module's.

## Composition of the returned aggregate — pinned

`Scope.allowed` holds paths, sorted ascending by byte order, with duplicates removed.
`Scope.departures` is sorted ascending by the contained `path`, with duplicates removed,
where two departures are duplicates when their paths are equal — if the same path appears
both deleted and not deleted, keep the `Deleted` one and discard the other.

Sorting is over the whole list, not within variant groups; `Foreign` and `Deleted` interleave
by path.

## Boundaries — pinned, including the degenerate cases

- `changes` empty: `target_changed = false`, `allowed` and `departures` both empty, and
  `is_clean` is therefore `true`.
- `declared.target` empty: `module_declaration_for("")` is `None`, so there is no allowance,
  and a change whose path is the empty string **does** equal the target and therefore sets
  `target_changed = true` by rule 2 like any other match. An earlier version of this spec
  claimed "nothing can equal the target" here, which is simply false, and two implementations
  read it two different ways. Nothing is special about an empty target except that the
  allowance disappears.
- A path appearing twice in `changes` with identical `deleted` contributes one entry.
- Paths are compared as exact strings. Do **not** normalise, canonicalise, resolve `..`, or
  strip prefixes. Two spellings of the same file are two different paths here, by design:
  guessing at path equivalence is how a scope check silently permits something.

## Types

Use the concrete types above exactly. Do **not** make any parameter generic — no
`impl Into<String>`, no `impl AsRef<str>`, no lifetimes beyond what the signatures show. A
previous task in this harness produced three implementations that could not be compiled
against one another because each invented its own generic bounds, and the cross-examination
matrix measured nothing at all as a result.

Derive `Debug`, `Clone`, `PartialEq` and `Eq` on every public type.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none. This module needs none
  of them.
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
