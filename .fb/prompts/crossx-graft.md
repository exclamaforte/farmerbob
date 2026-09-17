<!-- fb:modifies crates/fb/src/crossx.rs -->
<!-- fb:reads fb-crossx.sh -->
# Task: a grafted suite must not collide with the modules already in the file

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/crossx.rs`. Do not change any other file.

## The incident

Cross-examination has VOIDED on four consecutive tasks -- refusal-norm, liveness-cut, scope-gate,
claim-verify -- contributing nothing to any adjudication. Every cell reads `nocompile`, the
diagonal invariant fires, and the whole matrix is discarded. The cause is one function.

`test_body` grafts a candidate's `#[cfg(test)]` section into the reference and renames the test
module so it cannot collide. It renames **only** lines beginning `mod tests`:

```rust
if let Some(rest) = line.trim_start().strip_prefix("mod tests") {
```

A real file's test section contains more than `mod tests`. When a finding is escalated, the
proving test is promoted into the permanent suite as its own module, so
`crates/farmerbob-core/src/limit_signal.rs` now holds:

    mod conformance_limit_detect
    mod escalated_limit_detect_glm_53_flash

Those are in the REFERENCE because escalation put them there, and they are in the candidate's
frozen suite because the candidate's file contains them too. Grafting redeclares both:

    error[E0428]: the name `conformance_limit_detect` is defined multiple times
    error[E0428]: the name `escalated_limit_detect_glm_53_flash` is defined multiple times

So the instrument fails **more reliably the better a module is tested**: every escalated finding
makes the next cross-examination of that file more certain to void. That is exactly backwards,
and it has been silently true for four tasks.

Verified by hand on liveness-cut: renaming every module in the grafted block instead of only
`mod tests` produced 119 passed, 0 failed, in both directions.

## Exact API

`test_body`'s signature and visibility do not change. Its contract does:

> The `#[cfg(test)]` section of `src`, with **every module declared in that section** renamed
> to a form that cannot collide with a module already present in the host file.

Keep the existing `xtests_<idx>` scheme so the rename stays traceable to the arm index: a module
`foo` in the section for index 3 becomes `xtests_3_foo`, and `tests` becomes `xtests_3_tests`.
State the scheme in the doc comment; other code greps these names.

## Falsifiable clauses

1. `mod tests` in the section is renamed, exactly as today. The existing test
   `graft_text_pipeline_renames_mod_tests_and_injects_uses` must still pass unchanged.
2. `mod conformance_limit_detect` in the section is renamed. This is the regression; it is not
   renamed today.
3. `pub mod x` is renamed and KEEPS its `pub`. Visibility is not the renamer's business.
4. Two different modules in one section get two different names, and neither collides with the
   other after renaming.
5. Grafting a section that declares `mod a` into a host that already declares `mod a` produces
   a body containing no `mod a` at the section's top level.
6. A `mod` appearing in a STRING or a comment inside the test section is not renamed. Pin one
   test with `let s = "mod tests";` inside the suite and assert it survives byte-for-byte.
7. Text before the first `#[cfg(test)]` is still excluded entirely, as today.

## Boundaries, at N and at zero

- A section with ZERO module declarations is returned unchanged apart from the existing
  behaviour. It must not panic and must not emit a stray rename.
- A file with no `#[cfg(test)]` at all yields an empty body, as today.
- N modules, all at the section's top level: all N renamed.
- A module nested INSIDE another module in the section: state in the doc comment whether you
  rename it, and pin your choice in a test. Renaming only top-level declarations is acceptable
  and is probably right -- a nested `mod` cannot collide with a host top-level module -- but
  unstated behaviour here is what produced this bug, so it must be decided in writing.
- `mod x;` with a semicolon -- a declaration, not a definition -- cannot appear inside a test
  section that compiles, since there is no file to point at. Say what you do with it rather
  than leaving it to chance.

## Superset status on every enumerated list

Any list of module-name shapes you recognise is a **known subset**: module names you have not
anticipated must be renamed by the same rule, not skipped. The failure being fixed is precisely
a renamer that handled one name and passed everything else through untouched. Do not enumerate
module names; recognise a declaration structurally.

## Composition of aggregate returns

If you add any function returning a collection of renamed names or collisions, its doc comment
must state what it returns for an EMPTY section and whether the order is source order, and both
must be pinned by tests.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No regex crate; a line scan is enough and matches the existing code.
- `cargo build -p fb` and `cargo test -p fb` must pass. Run them.
- Do not weaken or delete an existing test to make a new one pass.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment saying what you could not verify. Returning less with a stated reason is correct here
and is scored as one.

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
