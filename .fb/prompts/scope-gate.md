<!-- fb:modifies crates/farmerbob-core/src/gate.rs -->
# Task: a run that left its declared scope must not be able to read PASS

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/gate.rs`. Do not change any other file.

Read `crates/farmerbob-core/src/scope.rs` first. It already decides, from paths alone, whether a
run stayed inside its declared deliverable, and `crates/fb/src/score.rs` already calls it. None
of that is missing and none of it is your job. The gap is that `gate::judge` never receives the
answer.

## The incident

On 2026-09-17 an arm was given a task whose spec said, in those words, "Do not change any other
file." It changed 53, +2780/-1075, rewriting much of two crates. The harness measured this
correctly and recorded:

    scope_clean: false, scope_departures: 51

and the verdict was `PASS`, because `Observation` has no scope field and `judge` cannot see one.
The same arm did the same thing on the next task: +2941 lines. Both scored PASS. The objective
table has no scope column either, so a reader looking at the numbers sees only that the arm with
by far the most lines passed on both.

A gate that measures a violation and then returns Pass is worse than one that never measured it,
because the record now carries evidence that the run was fine.

## Exact API

Add one field to `Observation`, at the end, and one variant to `Verdict`:

```rust
pub struct Observation {
    // ... existing fields unchanged, in their current order ...
    /// Paths changed outside the declared deliverable, as counted by
    /// [`crate::scope::assess`]. `Some(0)` means measured and clean.
    /// `None` means scope was never assessed, which is NOT the same as clean.
    pub scope_departures: Option<u32>,
}

pub enum Verdict {
    // ... existing variants unchanged ...
    /// Built, tested and wrote code, but touched files outside its declared
    /// deliverable. A statement about what the run was PERMITTED to change,
    /// not about whether it can write working code.
    OutOfScope,
}
```

`judge` keeps its signature: `pub fn judge(o: &Observation) -> Verdict`.

Every existing caller constructs `Observation` with struct literal syntax, so adding a field
breaks them at compile time, which is intended and is the point of adding it there. Fixing those
callers is NOT your job -- they are in `crates/fb`, which this task may not touch. Add
`#[derive(Default)]` to `Observation` if and only if it does not already have one, so callers
outside this crate can be migrated with `..Default::default()` in a later task, and say in the
doc comment that `Default` gives `None` everywhere -- every field unmeasured -- and is a starting
point for construction, never a description of a run.

## The precedence rule, decided here so it is not decided by accident

`OutOfScope` displaces `Pass` and NOTHING ELSE.

A run that departed scope AND failed to build reports `NoCompile`. A run that departed scope and
whose tests failed reports `TestsFail`. Those verdicts describe the code the arm wrote; scope
describes what it was allowed to touch, and when the code is already unusable the more actionable
fact wins. But a run that built, ran tests, passed them, wrote lines, and departed scope reports
`OutOfScope` and must never report `Pass`.

State this in `judge`'s doc comment in those terms. It is a decision, not a derivation, and the
next person needs to know it was made on purpose.

## Falsifiable clauses

State each as a test.

1. Everything positive and `scope_departures: Some(0)` returns `Pass`.
2. Everything positive and `scope_departures: Some(1)` returns `OutOfScope`. One departure is a
   departure; there is no tolerance band.
3. Everything positive and `scope_departures: Some(51)` returns `OutOfScope` -- the real case.
4. **`scope_departures: None` with everything else positive returns `Indeterminate`, NOT `Pass`
   and NOT `OutOfScope`.** Scope that was never assessed is not scope that was clean. This is the
   clause that matters most; the whole incident is an instance of a measurement that was missing
   being treated as a measurement that was fine.
5. `built: Some(false)` with `scope_departures: Some(51)` returns `NoCompile`, not `OutOfScope`.
6. `tests_passed: Some(false)` with `scope_departures: Some(51)` returns `TestsFail`.
7. `lines_added: Some(0)` with `scope_departures: Some(51)` returns `NoOp`. An arm that wrote
   nothing to its target cannot have departed scope in any interesting sense, and `NoOp` is the
   more informative answer.
8. `tests_run: Some(0)` with `scope_departures: Some(51)` returns `NoTests`. The empty-suite trap
   still outranks scope.
9. `Verdict::OutOfScope.is_pass()` is `false`.
10. Every existing test in this file still passes unchanged. If one encodes the old behaviour,
    change it and say which and why -- do not delete it.

## Boundaries, at N and at zero

- `Some(0)`: measured, clean. Pass is reachable.
- `Some(1)`: the smallest departure. Still `OutOfScope`.
- `Some(u32::MAX)`: no overflow, no panic, same answer as `Some(1)`.
- `None`: `Indeterminate`. Pin it separately from clause 4 for the case where another field is
  ALSO `None`, and state which `Indeterminate` wins -- they are the same verdict, so the test is
  that it does not become `Pass` or `OutOfScope` by either route.
- An `Observation` with every field `None` returns `Indeterminate`, as it does today. Adding a
  field must not change that.

## Superset status on every enumerated list

`Verdict`'s doc says "Exactly these verdicts and no others", and that is CORRECT and must stay.
It is a closed set on purpose: a verdict is a decision the harness makes, not an observation of
an open world, and the whole value of the type is that every caller can match it exhaustively.
This is the opposite of the refusal-pattern lists, which are an open subset of how a provider
might behave. Do not weaken this doc comment into a "known subset" hedge; instead, say in one
line why this enum is closed where those lists are open, so the distinction survives.

`Departure` and `Allowance` in `scope.rs` are not yours to change.

## Composition of aggregate returns

If you add any function over several observations, its doc comment must state what it returns for
an EMPTY slice and whether the result is ordered, and both must be pinned by tests. An aggregate
whose empty case is unstated is how "nothing was out of scope" and "nothing was examined" become
the same answer -- which is the defect this entire task exists to close.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Add no dependencies.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. The `crates/fb`
  build WILL break because callers lack the new field; that is expected, it is not yours to fix,
  and you must say so plainly in your handoff rather than reaching outside the crate to silence
  it.
- Do not make parameters generic. Concrete types only.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment saying what you could not verify. Returning less with a stated reason is a correct answer
here and is scored as one. A previous submission in this series shipped a file that emitted its
script's exact output format with the numbers hardcoded, and passed build, scope, lint and its
own tests, because every other gate in this harness is a gate on form. This one is read.

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
