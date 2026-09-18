<!-- fb:creates crates/farmerbob-core/src/wave_plan.rs -->
# Task: the launch decision is in shell, and every bug in it cost a wave

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/wave_plan.rs` and add `pub mod wave_plan;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

Whether to launch a queued wave is currently decided by `fb-autopilot.sh`, in shell, untested.
Every defect in that decision this week cost real time:

- It launched only when NOTHING was running, so a three-arm wave against a six-slot machine left
  half the box idle by construction. Twenty-three consecutive ticks logged "queue empty" while
  one wave ran.
- A global lock meant a second wave was refused however many slots were free. The slot check it
  was paired with refused a launch zero times in its entire life, because it could never be
  reached.
- A bead-scoped lock, added to fix a different bug, was taken EXCLUSIVE and held for a whole
  run, so three arms of one wave ran strictly sequentially.
- A wave was launched against a target another wave was writing, and separately against a
  module that did not exist yet.

None of that was testable. This module makes the decision a function.

## Exact API

```rust
/// A wave waiting in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    /// The queue file's name, e.g. `"wave74"`. Ordering key.
    pub id: String,
    /// The task it dispatches.
    pub task: String,
    /// Repo-relative path the task declares it will write.
    pub target: String,
    /// How many arms it would dispatch.
    pub arms: u32,
}

/// A wave already running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// The task.
    pub task: String,
    /// The target it is writing.
    pub target: String,
    /// Arms currently holding a slot.
    pub arms: u32,
}

/// Why a queued wave was not launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Another running wave is writing the same target.
    TargetBusy,
    /// Not enough free slots for the whole wave.
    NotEnoughSlots,
    /// The concurrent-wave limit is already reached.
    TooManyWaves,
}

/// What the autopilot should do now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    /// Waves to launch, in queue order.
    pub launch: Vec<String>,
    /// Waves held, each with the first reason that applied, in queue order.
    pub held: Vec<(String, Held)>,
}

/// Decide which queued waves may start.
///
/// `slots` is the machine's total capacity; running arms are subtracted from
/// it. `max_waves` caps concurrent waves, counting those already running.
pub fn plan(
    queued: &[Queued],
    running: &[Running],
    slots: u32,
    max_waves: u32,
) -> Launch;
```

## Falsifiable clauses

1. A wave whose `target` equals the `target` of any running wave is held as `TargetBusy`,
   whatever the slot count. Two waves writing one file is how a merge ended up being done by
   hand as a graft.
2. A wave is launched only if its `arms` fit entirely in the free slots, where free is
   `slots` minus the sum of every running wave's `arms`. A PARTIAL wave is never launched:
   its remaining arms would queue behind a full machine and straggle for hours.
3. **Launching accumulates.** A wave admitted by this call occupies its arms and its wave slot
   for every later wave in the same call. Pin a case where two waves fit and a third does not
   only because the first two were admitted.
4. `max_waves` counts running waves plus those launched in this call. Pin that a full
   `max_waves` holds everything as `TooManyWaves` even with every slot free.
5. Reasons are checked in the order `Held` declares and the FIRST match is reported. A wave that
   is both target-busy and slot-starved reports `TargetBusy`.
6. Queue order is `queued`'s order and is preserved in both output vectors. This module does
   not sort; the caller supplies the order it wants honoured.
7. **Every queued wave appears exactly once across `launch` and `held`.** Pin the partition: a
   wave in neither is the failure this would be least likely to notice.
8. A wave whose `target` matches ANOTHER QUEUED WAVE launched earlier in this same call is
   held as `TargetBusy`. Clause 1 covers already-running waves; this covers waves this call is
   about to start, and omitting it is how two waves on one file would still slip through.

## Boundaries, at N and at zero

- An EMPTY queue: an empty `Launch`, equal to `Launch::default()`.
- `slots` of 0: everything held as `NotEnoughSlots`.
- A wave with `arms: 0`: it needs no slots, so slots never hold it. State what you do and pin
  it -- a zero-arm wave is a malformed queue file, and launching it is harmless while silently
  dropping it is not.
- Running arms exceeding `slots`: free slots are 0, not negative. No underflow.
- `max_waves` of 0: everything held as `TooManyWaves`, even an empty machine.
- Two queued waves with the SAME id: both are considered, in order, and the second is subject
  to clause 8 against the first. They are distinct entries.
- Comparison of targets and tasks is EXACT string matching, as everywhere in this crate.

## Superset status on every enumerated list

`Held` is a CLOSED set of three. There is deliberately no `Held::Unknown` and no "force"
launch: a wave this module cannot admit must not start, and a caller wanting to override passes
different inputs rather than asking for a bypass.

## Composition of aggregate returns

`launch` carries ids in queue order and `held` carries `(id, reason)` in queue order; their
lengths sum to `queued.len()`. Say why `launch` is not sorted -- the caller's order is the
queue's priority and re-sorting it would silently reorder work.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O, no clock.
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
