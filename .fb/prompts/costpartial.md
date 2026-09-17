<!-- fb:modifies crates/farmerbob-core/src/cost.rs -->
# Task: an arm that was only partly measured cannot be placed on a cost frontier

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
`crates/farmerbob-core/src/cost.rs` ALREADY EXISTS. Read it first. Modify it in place and
change nothing else.

## Context

This harness ranks agents on a curve of cost against ability to complete tasks. Each run's
spend is read from that run's own store, and sometimes there is no store — an agent whose
launcher writes none, or a run whose telemetry was lost. `RunCost::usd` is `Option<f64>` for
exactly that reason: `Some(0.0)` is a measured zero, `None` is unmeasured.

`aggregate` then sums the measured runs into `ArmCost::usd`, documented "summed measured
spend, or `None` when NO counted run was measured". So an arm with six of seven runs priced
yields `Some(partial)` — and `usd_per_completion` divides that partial sum by **all**
completions, understating the arm's cost by exactly the unmeasured fraction, silently.

`ArmCost` has no way to say "this total is a lower bound". The binary that calls this module
currently counts priced runs itself and filters the field before asking for a frontier, which
is a caller compensating for a type that cannot express what it knows.

## What to change

Add one field to `ArmCost` and make two existing methods respect it. Everything else in the
file stays as it is.

```rust
pub struct ArmCost {
    pub arm: String,
    pub runs: u32,
    pub completed: u32,
    pub usd: Option<f64>,
    pub tokens: Option<u64>,
    /// Counted runs whose spend was not measured. Zero means the total is complete.
    pub unmeasured_runs: u32,          // NEW, and it goes last
}
```

New function, alongside the existing ones:

```rust
/// Whether this arm's spend is a complete total rather than a lower bound.
pub fn fully_measured(arm: &ArmCost) -> bool;
```

## Rules, each of which is testable

1. `aggregate` sets `unmeasured_runs` to the number of **counted** runs (`counts_for_arm ==
   true`) whose `usd` is `None`. Runs with `counts_for_arm == false` are excluded from this
   count exactly as they are excluded from `runs`, `completed`, `usd` and `tokens` today.

2. `fully_measured(arm)` is `true` if and only if `arm.unmeasured_runs == 0` **and**
   `arm.runs > 0`. It reports that no counted run was left unmeasured; it does **not** promise
   that `usd` is `Some`. Those differ, and two critics found the gap: an `ArmCost` with
   `usd: None, unmeasured_runs: 0, runs: 3` is not constructible by `aggregate` but is
   constructible by hand, and an arm whose every run is `Some(f64::NAN)` reaches
   `unmeasured_runs == 0` with `usd` dropped to `None` by the summing helper. In both cases
   `fully_measured` returns `true` about an arm with no known spend. Callers needing "spend is
   known" must check `usd` as well, and the doc comment must say so. An arm with no counted runs at all is not "fully measured"; it is
   unmeasured, and returning `true` for it would put an empty arm on a frontier.

3. `ArmCost::usd_per_completion` returns `None` when `unmeasured_runs > 0`, whatever `usd` and
   `completed` hold. A partial sum over all completions is not a cost per completion, and
   there is no correct way to scale it without knowing what the missing runs cost. Its
   existing conditions — unmeasured spend, or zero completions — still return `None` too.

4. `frontier` considers only arms for which `fully_measured` is `true`.

   **Be aware this is deliberately redundant with rule 3 and say so in a comment.** Rule 3
   already makes `usd_per_completion` return `None` for a partial arm, and `frontier` already
   drops arms with no comparable figure, so an implementation that omits this filter is
   indistinguishable by any test. Three critics on the first round of this task found exactly
   that and were right to. Keep the filter as a backstop against rule 3 being relaxed later,
   and mark it as one — an undocumented redundancy is how a rule quietly stops being enforced
   by the thing that appears to enforce it. An arm with a partial
   total has a spend that is a lower bound, and a lower bound cannot be shown not to dominate.
   Arms excluded this way never appear in the returned `Vec`, whatever their completion rate.

5. `totals` is unchanged in behaviour and keeps its current signature,
   `(Option<f64>, u32, u32)`. It sums every arm's measured spend including partial ones,
   because a partial total is still real money that was really spent. **`totals` and
   `frontier` deliberately disagree about partial arms and that is correct:** one is
   accounting, the other is comparison.

## Composition of the returned aggregate — pinned

`aggregate` still returns one `ArmCost` per distinct arm that has at least one run in the
input, ordered by arm name ascending, exactly as it does today. Adding the field changes no
ordering and drops no arm.

`frontier` still returns arm names cheapest-first by `usd_per_completion`, and ties keep the
order `aggregate` produced. An arm excluded by rule 4 is simply absent; it is not reported
with a sentinel.

## Boundaries — pinned, including the degenerate cases

- An arm whose counted runs are **all** unmeasured: `usd` is `None`, `unmeasured_runs` equals
  its counted run count, `fully_measured` is `false`, `usd_per_completion` is `None`, and it
  is absent from `frontier`.
- An arm with `runs == 0` — every run excluded by `counts_for_arm` — has `unmeasured_runs ==
  0`, and `fully_measured` is nevertheless `false` by rule 2.
- An empty `runs` slice yields an empty `Vec` from `aggregate` and an empty `Vec` from
  `frontier`.
- An arm with `completed == 0` but full measurement: `fully_measured` is `true`,
  `usd_per_completion` is `None` (no completions to divide by), and `frontier` skips it for
  lack of a comparable figure, as it does today.

## Types

Use the concrete types above exactly. Do **not** make any parameter generic, and do not add a
new error type, trait or wrapper — this is a change to an existing struct and two existing
functions plus one new predicate. Do not alter `RunCost`.

Keep the existing derives on `ArmCost`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- Every test already in `cost.rs` must still pass. If one of them constructs an `ArmCost`
  literal, update it for the new field; do not delete or weaken an existing assertion.
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
