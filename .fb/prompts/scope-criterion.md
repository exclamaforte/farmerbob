<!-- fb:modifies crates/farmerbob-core/src/adjudicate.rs -->
# Task: rank scope discipline on departures, not on crates touched

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/adjudicate.rs`. Do not change any other file.

Read `crates/farmerbob-core/src/scope.rs` first. It decides, from paths alone, whether a run
stayed inside its declared deliverable, and it is already what `fb score` and `gate::judge` use.

## Why

`Criterion::ScopeDiscipline` is documented as "Number of crates touched" and ranks arms on
`Evidence::crates_touched`, whose own doc says "The task names one; more is scope creep."

That measures the wrong thing, and it is the last survivor of the three proxy metrics retired on
2026-09-16 (farmerbob-2jc). The other two are gone: no threshold on line count, no doc-comment
density anywhere.

Counting crates penalises work the task ASKED for. A spec whose deliverable is
`crates/fb/src/promote.rs` and which requires a new enum variant in `farmerbob-core` forces two
crates; the arm that does as it was told scores worse on "discipline" than one that quietly
skipped half the job. And it says nothing at all about the rule the harness actually states and
now enforces, which is about the declared FILE:

> you change the ONE file the task declares, and nothing else

`scope::assess` already produces that answer, and `gate::judge` already gates on it via
`Observation::scope_departures`. The adjudicator alone still ranks on the proxy.

## Exact API

Add one field to `Evidence`, keep every existing field, and do not remove `crates_touched` --
it is still emitted in the score record and read by reports:

```rust
pub struct Evidence {
    // ... existing fields unchanged, in their current order ...
    /// Paths changed outside the declared deliverable, from [`crate::scope::assess`].
    /// `Some(0)` means measured and clean. `None` means scope was never assessed, which
    /// is NOT the same as clean.
    pub scope_departures: Option<u32>,
}
```

`Criterion::ScopeDiscipline` now ranks on `scope_departures`, lower is better, exactly as it
ranked on `crates_touched` before. Update its doc comment from "Number of crates touched" to what
it now measures. `ORDERED_CRITERIA` keeps all seven criteria in their current order -- this task
changes what one criterion reads, not which criteria exist or when they are consulted.

## Falsifiable clauses

1. Two candidates identical but for `scope_departures: Some(0)` and `Some(3)`: the criterion
   prefers the one with 0.
2. `crates_touched` no longer influences the ruling. Two candidates identical but for
   `crates_touched: Some(1)` and `Some(9)` produce no winner on ScopeDiscipline, and the
   adjudication falls through to the next criterion. This is the regression the task exists for;
   pin it.
3. A candidate with `scope_departures: None` is treated exactly as every other criterion treats
   an unmeasured value in this module -- do not invent a new rule. Read how `Conformance` and
   `Clippy` handle `None` and match it, and say in your handoff which behaviour you found and
   followed.
4. `Some(0)` and `None` must not compare equal. Measured-clean and never-measured are different
   facts, and this module's whole purpose is keeping them apart.
5. Every existing test in this file still passes, unchanged, except any that asserts a ruling
   decided by `crates_touched`. Change those, and say in your handoff which and why -- do not
   delete them.

## Boundaries, at N and at zero

- All candidates at `Some(0)`: no leader on this criterion, fall through to the next.
- One candidate measured and the rest `None`: whatever clause 3's rule gives, stated and pinned.
- `Some(u32::MAX)`: no overflow when converted for comparison.
- A single candidate: the existing single-candidate behaviour is unchanged.
- An empty candidate list: the existing behaviour is unchanged. Pin it if it is not already.

## Superset status on every enumerated list

`Criterion` is a CLOSED set of seven, and `ORDERED_CRITERIA` exists because two copies of that
list once drifted apart when `Cost` was removed from one of them. Do not add, remove or reorder a
criterion. If you believe the order is wrong, say so in your handoff and leave it alone.

## Composition of aggregate returns

`adjudicate` returns a `Ruling`. If you add any helper returning a collection of candidates still
in contention, its doc must state what it returns for an EMPTY input and whether the order is the
input order, both pinned by tests. "No candidate was eliminated" and "no candidate was examined"
must not be the same value.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them. The
  `crates/fb` build WILL break because callers construct `Evidence` without the new field; that
  is expected, it is not yours to fix, and reaching into another crate to silence it is the
  scope departure this very criterion measures. Say what you left broken in your handoff.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here and
is scored as one.

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
