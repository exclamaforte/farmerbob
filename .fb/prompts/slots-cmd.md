<!-- fb:creates crates/fb/src/slots_cmd.rs -->
<!-- fb:reads fb-admit.sh -->
# Task: give admission control its caller, and bound the SUM

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/slots_cmd.rs`, declare it from `crates/fb/src/main.rs` with
`mod slots_cmd;`, and wire an `fb slots` subcommand. Do not change any other file, and do not
delete the shell script.

Read `crates/farmerbob-core/src/slots.rs` first. `SlotTable::admit` already does the hard part:

    committed + requested <= available_mb - headroom_mb

It has NO CALLER. `fb-admit.sh` computes its own slot count in two lines of awk and never
consults it. Your job is the caller, not the arithmetic.

## The defect this closes

`fb-admit.sh` sizes the machine like this:

    SLOT_GB=1.5     # "measured p95 was 1.26G"
    HARD_GB=2       # per-run MemoryMax
    SLOTS = (avail - headroom) / SLOT_GB

Slots are counted against the MEASURED typical usage, and every run is then given a hard cap of
2G. Nine slots at 1.5G is 13.5G of expectation and 18G of permission. The sum of what the
machine may actually be asked for is never bounded, which is farmerbob-89j.

`SlotTable` bounds the sum because it charges each admission its BUDGET. Give it the hard cap as
the budget and the arithmetic is right by construction.

## Exact API

```rust
/// Report how many concurrent runs fit, and why.
///
/// `plan` prints the capacity line the shell prints. Exit code is 0 whenever the
/// question could be answered, including when the answer is one slot.
pub fn run_cmd(available_mb: u64, headroom_mb: u64, memory_mb: u64, plan: bool) -> i32;
```

Wire it as `fb slots --available-mb N --headroom-mb N --memory-mb N [--plan]`.

Use `farmerbob_core::slots::{SlotTable, Budget, Machine, Admission, RunRef}`. Do not reimplement
`admit`, `capacity` or the ceiling arithmetic, and do not define a parallel Budget or Machine.

## Falsifiable clauses

1. With `available_mb 17000`, `headroom_mb 3000`, `memory_mb 2048`: capacity is 6.
   (17000-3000)/2048 = 6.83, and a partial slot is not a slot.
2. Capacity is computed by admitting runs through `SlotTable` until one is `Queued`, not by
   dividing. The count and the table must agree by construction; say in the doc comment why you
   did not divide.
3. `available_mb 4000, headroom_mb 3000, memory_mb 2048`: the ceiling is 1000MB, one run needs
   2048MB, so NOTHING fits. Report capacity 0.
4. **Capacity 0 is reported as 0 and exits 0.** The shell clamps to a minimum of one slot
   (`[ "$SLOTS" -lt 1 ] && SLOTS=1`), which admits a run the machine cannot hold. Do not
   reproduce that clamp. Say in the doc comment that it is a deliberate divergence and why:
   a machine with no room should say so, and the caller decides whether to wait.
5. `available_mb == headroom_mb`: ceiling 0, capacity 0, no panic, no underflow.
6. `headroom_mb > available_mb`: ceiling 0, not a negative or wrapped number. Pin it.
7. `memory_mb == 0`: a slot that costs nothing would admit forever. Refuse it -- report the
   refusal and exit non-zero -- rather than looping. State which you chose.
8. Every `Queued` carries the reason `SlotTable` produced; do not replace it with your own text.

## Boundaries, at N and at zero

- Exactly divisible: `available 13288, headroom 3000, memory 2048` -> ceiling 10288 -> capacity 5
  exactly. Pin that a run fitting EXACTLY is admitted, not rejected.
- One MB short of a slot: capacity is one lower. Pin both sides.
- `u64::MAX` for any argument: no overflow, no panic.
- Capacity 1: the smallest working machine, distinct from capacity 0.

## Superset status on every enumerated list

`Admission` is a CLOSED set of two, `Admitted` and `Queued`. Do not add a variant and do not
collapse `Queued`'s reason into a boolean.

The command's FLAGS are a known subset: this task fixes four. `fb-admit.sh` also honours
`FB_PROVIDER_CAP`, and per-provider capping is deliberately NOT in scope here -- it is a
different constraint (the provider, not the machine) and mixing them is how one number ends up
serving two purposes. Say so in your handoff rather than adding it.

## Composition of aggregate returns

If you add a function returning the sequence of admissions, its doc must state what it returns
for a ceiling of zero and whether the order is admission order, both pinned by tests. An empty
sequence must be distinguishable from "not attempted" by the caller.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. Integer arithmetic only; saturating where it can underflow.
- `cargo build -p fb` and `cargo test -p fb` must pass. Run them.
- Tests take the numbers as arguments. Do not read `/proc/meminfo` or shell out to `free`.

## How this will be checked

There is NO differential oracle for this task, and that is stated rather than faked.
`fb-admit.sh` takes a matrix TSV and DISPATCHES runs; it has no reporting mode, so running it
with these flags would launch agents rather than answer a question. An earlier draft of this
spec declared one anyway, which would have made the harness dispatch a wave every time the
pipeline checked a port.

So this is checked the ordinary way: the clauses above, run against your implementation, plus
the fact that `SlotTable` is doing the arithmetic rather than a second copy of it.

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
