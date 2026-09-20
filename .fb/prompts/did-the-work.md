<!-- fb:creates crates/farmerbob-core/src/surface.rs -->
<!-- fb:modifies crates/fb/src/doctor.rs -->
# Task: what an arm did not touch must still work, and dead code must not hide

Rust workspace, already builds. Create `crates/farmerbob-core/src/surface.rs` and declare it
in `lib.rs`; modify `crates/fb/src/doctor.rs`. Change no other file.

## Why this exists

Two T16 beads, one question: **did the arm actually do the work, and is anything checking?**

`farmerbob-wu1m` — an arm once stubbed a shipped command to `-> i32 { 1 }` and scored PASS.
Nothing verifies that public items the task never mentioned still exist. The gate asks
whether the crate builds and its tests pass; a stub does both.

`farmerbob-649f` — measured on 2026-09-19: **20 of the `fb` crate's modules carry
`#[allow(dead_code)]`, and removing them surfaces 21 dead items** — `function run is never
used` twice, `struct StageArgs is never constructed`, `struct Invocation`, `struct Flags`,
`method exit_code`, `function suggest`, and more. These sit inside REACHABLE modules, so
they are invisible twice: the compiler is told not to mention them, and `fb doctor`'s
unreachable-modules check works at module granularity and cannot see inside one.

The two meet at the same place. A public item that vanishes is the first bug; a public item
that was never reachable is the second; and in both cases the tree compiles, the tests pass,
and nothing says a word.

## The exact API

In `crates/farmerbob-core/src/surface.rs`:

```rust
/// One public item a module exposes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Item {
    /// The item's name, e.g. `run`.
    pub name: String,
    /// What kind it is, lowercased as written: `fn`, `struct`, `enum`, `trait`, `type`,
    /// `union`, `const`, `static`.
    pub kind: String,
}

/// Every public item a source file declares, sorted and deduplicated.
///
/// `name_collision::public_types` answers a NARROWER question -- type names only, for
/// finding two things with one name. This needs functions and consts too, because a stubbed
/// `pub fn run` is exactly the case wu1m is about. Do not change that module; this one is
/// its own question.
pub fn public_items(source: &str) -> Vec<Item>;

/// What happened to a file's public surface between two revisions.
///
/// This list is CLOSED: a fourth case is a change to this enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceChange {
    /// Nothing public was removed. Carries anything ADDED, which is normal and reported
    /// only so a reader can see it.
    Intact { added: Vec<Item> },
    /// Public items present on the base are gone. Carries them, sorted.
    Removed { gone: Vec<Item> },
    /// One side could not be read. Carries which and why.
    Unmeasured { reason: String },
}

/// Compare one file's public surface. `base` and `candidate` are the file's full text, or
/// `None` when it is absent on that side.
pub fn compare(base: Option<&str>, candidate: Option<&str>) -> SurfaceChange;
```

## Falsifiable clauses

1. `public_items` finds `pub fn`, `pub struct`, `pub enum`, `pub trait`, `pub type`,
   `pub union`, `pub const` and `pub static`, each with its kind. This list is a SUPERSET:
   Rust may gain another public item form, and one not listed is simply not reported, never
   guessed at. (bead farmerbob-2ve)

2. A PRIVATE item is not public surface and is absent from the result. Removing a private
   helper is refactoring, not a broken contract.

3. An item inside a `#[cfg(test)]` module is not public surface. Test scaffolding is not
   the crate's API, and counting it would make every fixture a contract.

4. `compare` returns `Removed` naming every public item on the base and missing from the
   candidate. Pin it with a `pub fn run` stubbed to a private `fn run`: the item is GONE
   from the public surface even though the name still appears in the file. **This is the
   wu1m case and is the point of the task.**

5. `compare` returns `Intact { added }` when nothing was removed, carrying additions. Adding
   public API is normal work and must never read as a fault.

6. `base: None, candidate: Some(_)` is `Intact` with every item added: a new file removed
   nothing. `base: Some(_), candidate: None` is `Removed` with every item gone: deleting a
   file removes its whole surface.

7. `base: None, candidate: None` is `Unmeasured`, not `Intact`. Neither side was seen, and
   "nothing was removed" is a claim about a comparison that did not happen.

8. A new `fb doctor` check reports every `#[allow(dead_code)]` in the `fb` and
   `farmerbob-core` crates, naming the files. `Warn`, not `Fail` -- it compiles and runs.
   An unreadable crate directory WARNS rather than reporting none, because reporting a clean
   tree when the instrument failed is the defect `fb doctor` exists to surface.

## Boundaries, at N and at zero

- An EMPTY source: no items, and not an error.
- A source with no public items at all: an empty vec. `compare` of two such is
  `Intact { added: [] }`.
- The SAME name with two kinds (`pub struct Foo` and `pub fn Foo` cannot coexist in Rust,
  but `pub const X` and `pub fn X` can): they are two Items and both must be tracked.
  Removing one while keeping the other is `Removed`.
- `public_items` is sorted and deduplicated, so two runs over one file agree.

## Composition of the aggregate return

`compare` returns exactly one `SurfaceChange`. `Removed` wins over `Intact`: a file that
both added and removed public items is `Removed`, because the removal is the fault and an
addition must not mask it. `Unmeasured` wins over both.

## Rules

`public_items` and `compare` are pure: no file system, no clock, no environment. The caller
reads the files. Do NOT change `name_collision`; it answers a narrower question and has its
own tests. Every clause above gets a test named for what it pins. Keep the workspace
rustfmt-clean and clippy-clean and every existing test passing.

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
  measurement is `farmerbob_core::scope`, and it compares paths as exact strings.

  **A task declares a SET of files.** Most declare one. A task that legitimately spans
  several lists them all, and touching any of them is in scope; so is adding `pub mod y;` to
  the `lib.rs` beside each new file. A file outside the declared set is a departure, and a
  departure yields the verdict `OutOfScope`, which is not a pass.

  The set was a single file until 2026-09-19, and that was never a decision -- it was
  inferred from an incident. One arm changed 51, 52 and 51 files across three runs and was
  disqualified for wandering. It had not wandered: it had run `cargo fmt` on a
  rustfmt-dirty repository, and 51 is exactly the number of dirty files its changes
  intersected. The gate was tightened on the strength of a formatter artefact, and the
  formatter bug was fixed separately the same day.

  What that cost was not theoretical. A task adding one field to a struct could not pass if
  any other file in the same crate constructed it: fixing those callers was OutOfScope and
  leaving them was TESTS-FAIL. No legal move, and an arm lost a run to it.

  **`.fb/handoff.md` is the one exception, and it OVERRIDES the task's own Rules section.**
  Every spec's Rules says "change <target> and NOTHING else"; the Handoff section below then
  requires you to write `.fb/handoff.md`. Read literally those contradict, and a critic caught
  it: "one correct implementation must leave it untouched to obey the Rules, while another must
  write it to satisfy the Gate". Write the handoff. It is exempt, it has always been exempt,
  and the scope measurement already excludes it. Nothing else is.

  **`cargo fmt` is safe, and this line used to say the opposite.** The base you are given is
  rustfmt-clean -- `cargo fmt --all -- --check` exits 0 on it -- so running the formatter
  rewrites your file and nothing else. Run it if you want it.

  It was not always so, and the history is why this paragraph exists rather than a bare
  permission. The workspace drifted dirty because every merge takes ONE file from ONE arm in
  that arm's style and nothing normalised it afterwards. An arm that then ran a formatter had
  every dirty file its build touched rewritten, and the scope gate counted each one as a
  departure. One arm lost four runs that way, at 51, 52, 51 and 17 departures; the 51 was
  exactly the number of rustfmt-dirty files its changes intersected. The arms were doing
  ordinary Rust and the repository was wrong.

  So if `cargo fmt` DOES touch a file you did not change, the base has drifted again. Revert
  that file, keep your own, and say so in your handoff -- that sentence routes a harness bug
  back where it belongs instead of costing you the run.

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

## A "derive these traits" instruction applies to types YOU define

If a spec says to derive `Debug, Clone, PartialEq, Eq` "on every type in the Exact API", it
means every type the task DEFINES. An Exact API block also NAMES types it imports --
`Measurement`, `Absent`, `Verdict`, `BuildVerdict` -- and you can neither define nor change
those. Read literally the two instructions contradict, and codex-luna reported exactly that
before implementing: "the derive rule requires the implementation to derive those traits on
`Measurement` and `Absent`, but the adjacent rule says those existing types must not be
defined".

Derive on what you write. If an imported type lacks a trait your tests need, say so in your
handoff and test around it; do not edit the other file.

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
