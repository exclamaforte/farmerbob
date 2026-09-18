<!-- fb:creates crates/farmerbob-core/src/lib_diff.rs -->
# Task: the scope gate permits a file, and an arm rewrote three lines in it

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/lib_diff.rs` and add `pub mod lib_diff;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`scope::assess` compares PATHS. The only allowance is `Allowance::ModuleDeclaration`: the
`lib.rs` beside the target, because adding `pub mod y;` is required to make a new module
compile. The disclosed rubric says the gate "permits exactly two things: the declared target,
and adding `pub mod y;` to the `lib.rs` beside it."

It cannot check the second half of that sentence. `lib.rs` is permitted as a path, so anything
done inside it passes.

On `store-batch`, an arm changed three existing declarations in `lib.rs`:

    -    conn: Connection,
    +    pub(crate) conn: Connection,
    -fn run_state_data_json(...)   ->  +pub(crate) fn run_state_data_json(...)
    -fn run_state_name(...)        ->  +pub(crate) fn run_state_name(...)

and scored clean. A rival found it by reading the diff and noted the edits were not even
needed -- default visibility on root-defined items already reaches every descendant module.
The arm had the best test suite in the field and was ruled out of scope by hand, which is
exactly the judgement a gate exists to make without an adjudicator squinting at a diff.

## Exact API

```rust
/// One line of a unified diff of `lib.rs`, already stripped of its leading
/// `+` or `-`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// The line's text, without the diff marker and without a trailing newline.
    pub text: String,
    /// True for an added line (`+`), false for a removed line (`-`).
    pub added: bool,
}

/// What a `lib.rs` diff did, beyond what the gate permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibChange {
    /// Only module declarations were added, and nothing was removed.
    DeclarationsOnly {
        /// The modules declared, in the order they appear, sorted and
        /// deduplicated.
        modules: Vec<String>,
    },
    /// Something else happened. Every offending line, in diff order.
    Beyond {
        /// Lines that are not permitted, in the order given.
        lines: Vec<DiffLine>,
    },
    /// The diff is empty: `lib.rs` was not touched.
    Untouched,
}

/// Classify a `lib.rs` diff.
pub fn classify(diff: &[DiffLine]) -> LibChange;

/// Whether this change is within what the gate permits.
pub fn permitted(c: &LibChange) -> bool;

/// The module name a `pub mod y;` line declares, if the line is exactly that.
///
/// Returns `None` for anything else, including `mod y;` without `pub`,
/// `pub mod y { .. }` with a body, and a line with trailing code.
pub fn declared_module(line: &str) -> Option<&str>;
```

## Falsifiable clauses

1. An EMPTY diff is `Untouched`, and `permitted` is true for it.
2. A diff of only added `pub mod y;` lines is `DeclarationsOnly`, carrying those module names
   sorted and deduplicated, and `permitted` is true.
3. **Any REMOVED line makes it `Beyond`, whatever the line is** -- including a removed
   `pub mod y;`. The allowance is for ADDING a declaration; deleting one is a change to what
   the crate exposes. Pin a diff whose only entry is a removal.
4. An added line that is not a module declaration makes it `Beyond`, carrying that line. Pin
   the three shapes from `store-batch`: `pub(crate) conn: Connection,`,
   `pub(crate) fn run_state_data_json(...)` and a visibility change on `fn run_state_name`.
5. `Beyond::lines` carries EVERY offending line, not the first. An arm that made four
   unpermitted edits should see four.
6. `declared_module("pub mod y;")` is `Some("y")`. It is `None` for `mod y;`,
   `pub mod y {`, `pub mod y;  // note`, `pub  mod y;` with a double space, and
   `pub mod y ;`. The gate is exact because the rubric quotes the exact form; say so, and say
   that a rejected shape is reported as `Beyond` rather than silently permitted.
7. Whitespace-only and empty added lines are permitted and do NOT appear in `modules`. A diff
   that adds a blank line beside a declaration is still `DeclarationsOnly`.
8. **A comment-only added line is permitted.** `// declare the module` beside a declaration is
   not a change to what the crate exposes. Pin it, and pin that a line with code BEFORE a
   comment is not.

## Boundaries, at N and at zero

- Zero lines: clause 1.
- One added declaration: `DeclarationsOnly` with one module.
- The SAME module declared twice in one diff: `modules` carries it once; the diff is still
  `DeclarationsOnly`.
- One added declaration and one removed blank line: `Beyond`, per clause 3, and the removed
  blank line is the offending entry. State this plainly -- it is the case a reader expects to
  be forgiven and clause 3 does not forgive it.
- A diff of only blank added lines: `DeclarationsOnly` with an EMPTY `modules` vector, not
  `Untouched`. Untouched means no diff at all; say why the two differ.
- A line whose text is exactly `pub mod ;`: not a declaration, so `Beyond`.

## Superset status on every enumerated list

`LibChange` is a CLOSED set of three. The PERMITTED LINE SHAPES are a CLOSED set of exactly
three -- an exact `pub mod y;`, a whitespace-only line, and a comment-only line -- and anything
else is `Beyond`. This list is closed deliberately: an open one would let the next unanticipated
edit through, which is the defect being fixed.

## Composition of aggregate returns

`DeclarationsOnly::modules` is sorted and deduplicated and MAY be empty (clause 7's blank-line
case). `Beyond::lines` is in the order the diff supplied and is never empty -- say why it cannot
be, rather than guarding against it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O. The diff arrives as a slice.
- Do not change `scope.rs`. This module is read ALONGSIDE it; replacing `Allowance` is a
  separate decision that wants this module's evidence first.
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
