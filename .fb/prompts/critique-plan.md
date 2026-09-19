<!-- fb:creates crates/farmerbob-core/src/critique_plan.rs -->
# Task: who reviews whom, and what they are shown, is 90 lines of bash

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/critique_plan.rs`. Declare it in `lib.rs` with one
`pub mod critique_plan;` line and change nothing else.

## Why this exists

`fb-critique.sh` decides two things before it spends a single token, and both are pure:

1. **Assignment.** Arm `i` reviews arm `i+1 mod N`. A derangement -- nobody reviews themselves.
2. **What the critic is shown.** The deliverable, not "whatever git happens to call a change".

The second rule exists because it was once wrong. The script ran `git diff HEAD -- crates/$CRATE`
with a fallback to reading the target file if the diff came back EMPTY. For a task that CREATES
a file the deliverable is untracked, so the diff contained exactly one tracked line:

```
+pub mod matrix;
```

Not empty. The fallback never fired. On the `matrix` task that produced four reviews of which
three were worthless and two were fabricated -- one critic invented line numbers for a file it
had never seen. The guard was defeated by the very line that made the patch useless.
(bead farmerbob-4ur)

Both decisions are in shell, untested, and re-derived every run.

## Exact API

```rust
/// One arm that produced something reviewable.
pub struct Candidate {
    /// Arm name, e.g. "glm-53-flash".
    pub arm: String,
    /// The diff of the declared target in this arm's worktree, if the target is TRACKED.
    /// Empty string and `None` are different: see clauses 4 and 5.
    pub target_diff: Option<String>,
    /// The full contents of the declared target on disk, if it exists at all.
    pub target_contents: Option<String>,
    /// Paths this arm changed that are NOT the declared target and NOT the module
    /// declaration line. In the order given.
    pub strayed: Vec<String>,
}

/// What one critic is asked to review.
pub struct Assignment {
    pub critic: String,
    pub subject: String,
    pub patch: Patch,
    /// Copied from the subject's `strayed`, unchanged, same order.
    pub strayed: Vec<String>,
}

/// The artefact a critic is shown.
pub enum Patch {
    /// The target is tracked and has a non-empty diff. Carries that diff.
    Diff(String),
    /// The target is untracked or unchanged, and exists on disk. Carries its contents
    /// and its line count.
    NewFile { contents: String, lines: usize },
}

/// Why no plan could be made.
pub enum NoPlan {
    /// Fewer than two candidates have something reviewable.
    TooFew { reviewable: usize },
}

/// Assign every reviewable candidate exactly one subject.
///
/// Candidates with nothing to show are dropped BEFORE the assignment is computed,
/// so the derangement is over the reviewable set, not over the input.
pub fn plan(candidates: &[Candidate]) -> Result<Vec<Assignment>, NoPlan>;

/// What a single candidate would be shown as, or `None` if it has nothing to show.
pub fn patch_for(c: &Candidate) -> Option<Patch>;
```

## Falsifiable clauses

1. `patch_for` returns `Diff(d)` when `target_diff` is `Some(d)` and `d` is **not** empty and
   not entirely whitespace.
2. `patch_for` returns `NewFile` when the diff is absent, empty, or whitespace-only, AND
   `target_contents` is `Some(c)` with `c` non-empty. `lines` is the number of lines in `c`.
3. `patch_for` returns `None` when neither is available: no diff worth showing and no contents.
   A candidate that is `None` is NOT reviewable and does not appear in the plan, as critic or as
   subject.
4. `Some("")` and `None` for `target_diff` must reach the SAME branch -- clause 2. Pin them in
   one test that constructs both and asserts both yield `NewFile`, not in two tests that each
   pass alone. This is the matrix defect: a check that distinguished empty-from-absent when the
   distinction it needed was one-worthless-line-from-real.
5. `Some("+pub mod matrix;\n")` yields `Diff`, not `NewFile`. This clause is here to state that
   the fix is NOT "treat a short diff as empty". The shell's error was reading the WRONG PATH
   (the whole crate); a one-line diff of the declared TARGET is a real diff. Pin it.
6. In a returned plan, no assignment has `critic == subject`.
7. Every reviewable candidate appears exactly once as a critic and exactly once as a subject.
8. `Assignment::strayed` is the SUBJECT's strayed list, not the critic's, in the order given.
9. The order of the returned vector follows the order of the reviewable candidates in the input.

## Boundaries, at N and at zero

- ZERO candidates: `Err(TooFew { reviewable: 0 })`.
- ONE reviewable candidate: `Err(TooFew { reviewable: 1 })`. One arm cannot review itself and
  there is nobody else; this is the boundary the shell guards with `[ "$N" -lt 2 ]`.
- TWO reviewable: a plan of exactly two, each reviewing the other. At N=2 the wrap-around
  derangement is forced and is its own inverse; assert both assignments, because this is the
  smallest case where clause 6 can fail silently.
- THREE reviewable: a 3-cycle. Assert it is a cycle and not two arms swapping with a third
  reviewing itself.
- THREE candidates of which ONE is unreviewable: the plan has exactly two assignments and the
  unreviewable arm appears nowhere. Not three assignments with an empty patch.
- `target_contents: Some("")`: treated as nothing to show -- `None`. An empty file is not a
  deliverable.

## Superset status on every enumerated list

`Patch` has EXACTLY these two variants and no others. `NoPlan` has exactly one. Adding a third
`Patch` variant -- a "both" or an "empty" -- is a defect, because the caller's job is to put
one artefact in front of one critic.

The `strayed` list is given to you; you do not compute it and you do not filter it. The rule
that the module-declaration line is excluded from it is the CALLER's, already implemented, and
re-deriving it here is a defect.

## Composition of aggregate returns

`plan` returns a vector whose length is the number of REVIEWABLE candidates, whose order is
clause 9, and each of whose elements is fully determined by the input. State in your handoff
whether two candidates with the SAME arm name are possible and what your implementation does;
the spec does not pin it, so your tests may not assert on it.

`Patch::NewFile.lines` counts lines in `contents`. Whether a trailing newline makes the last
line count is NOT pinned -- say which you chose in your handoff and do not assert on a case that
turns on it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem access and no `std::process` -- this module decides, it
  does not read worktrees. A function here that shells out to git is a defect.
- `cargo test -p farmerbob-core` and `cargo clippy -p farmerbob-core -- -D warnings` must pass.
  Both are clean on HEAD as of this task, so any failure is yours.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus, ONLY when the task
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
