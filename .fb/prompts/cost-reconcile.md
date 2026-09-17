<!-- fb:modifies crates/farmerbob-core/src/cost.rs -->
# Task: make the cost axis report its disagreement with the provider

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/cost.rs`. Do not change any other file.

Read `cost.rs` first: `RunCost`, `ArmCost`, `aggregate`, `frontier`, `totals`, `fully_measured`,
and the `tokens_from_log` / `Launcher` pair merged today. You are adding a check over the result,
not changing how it is computed.

## The measurement

On 2026-09-17 the board and the provider were compared for the first time, over the same window
(the key was created the previous day, so there is no earlier history to explain it away):

    fb pareto        $15.9247 attributable + $0.9500 non-arm  = $16.8747
    OpenRouter       $20.0299

A residual of $3.1552, about 19%, with the board on the LOW side. Our figure is inferred from a
hand-entered price table in `sources.toml`, carrying a `price_checked` date and nothing that ever
compares it to a bill (farmerbob-xsa, farmerbob-wuu).

This matters because the whole project is a curve of cost against capability. An axis that is
systematically 19% light does not shift arms uniformly -- arms differ in how much of their spend
is priced -- so it reorders the very ranking the board exists to produce.

## What this task does NOT do

It does not fetch anything, and it does not correct our figure toward the provider's. Picking one
number and hiding the other is what a spreadsheet does. This reports the disagreement so a reader
can see it on every board.

## Exact API

```rust
/// Our figure against the provider's, over the same window.
#[derive(Debug, Clone, PartialEq)]
pub enum Reconciliation {
    /// The two agree within tolerance.
    Agrees { ours_usd: f64, theirs_usd: f64 },
    /// They differ by more than tolerance. `residual_usd` is theirs minus ours, so a
    /// POSITIVE residual means we under-counted.
    Differs { ours_usd: f64, theirs_usd: f64, residual_usd: f64, fraction: f64 },
    /// One side is unavailable, so no comparison was made. NOT agreement.
    Unchecked { missing: String },
}

/// Compare our attributable total with the provider's reported total.
///
/// `tolerance_fraction` is relative, e.g. 0.01 for one percent.
pub fn reconcile(
    ours_usd: Option<f64>,
    theirs_usd: Option<f64>,
    tolerance_fraction: f64,
) -> Reconciliation;
```

## Falsifiable clauses

1. `reconcile(Some(16.8747), Some(20.0299), 0.01)` is `Differs`, with `residual_usd` ≈ 3.1552 and
   `fraction` ≈ 0.1575 of the provider's figure. State in the doc which denominator you use --
   theirs or ours -- and pin it; they give different percentages and an unstated one is useless.
2. `reconcile(Some(20.0), Some(20.1), 0.01)` is `Agrees`: 0.5% is inside one percent.
3. **`theirs_usd: None` is `Unchecked`, never `Agrees`.** A provider we did not ask has not
   confirmed us. This is the clause that matters; say it in the doc in those words.
4. `ours_usd: None` is likewise `Unchecked`, with a reason naming which side was missing --
   the two cases are different facts and a reader must be able to tell them apart.
5. A POSITIVE residual means we under-counted. Pin the sign with the real numbers, because
   getting it backwards inverts the only conclusion this function exists to support.
6. Exactly at tolerance -- a 1.0% difference with `tolerance_fraction: 0.01` -- has a defined
   answer. State which side of the boundary it falls on and pin both it and one cent either way.

## Boundaries, at N and at zero

- Both zero: `Agrees`. Two parties agreeing that nothing was spent is agreement.
- `theirs_usd: Some(0.0)` with `ours_usd: Some(5.0)`: `Differs`, and `fraction` must not be
  `inf` or `NaN` from dividing by zero. State what you return for the fraction and pin it.
- `tolerance_fraction: 0.0`: any difference at all is `Differs`; identical values still `Agrees`.
- A negative `tolerance_fraction` is nonsense input: say what you do and pin it.
- Negative money -- a refund -- is possible in principle. Say whether you accept it and pin it.
- `f64::NAN` or infinity on either side: `Unchecked`, never `Agrees`. A value that is not a
  number has not been compared.

## Superset status on every enumerated list

`Reconciliation` is a CLOSED set of three. Do not add a variant.

But say in `reconcile`'s doc that the ways our figure can be wrong are OPEN -- a stale
`price_checked`, an unpriced launcher, a missing retry, reasoning tokens billed but never
counted -- and that this function detects only that the totals disagree, never which side is
wrong or why. It is an alarm, not a diagnosis.

## Composition of aggregate returns

If you add a function over several providers or windows, its doc must state what it returns for
an EMPTY input and whether the order is input order, both pinned. An empty result must not be
readable as "everything reconciled".

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No I/O and no network: both figures arrive as arguments.
- Do not change `aggregate`, `frontier`, `totals` or any existing signature.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here --
and on this task above all, since its entire purpose is to stop a comfortable number standing in
for a checked one.

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
