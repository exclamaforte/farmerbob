<!-- fb:creates crates/farmerbob-core/src/reap_exec.rs -->
# Task: a plan that cannot be checked against the world before it runs

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/reap_exec.rs` and add `pub mod reap_exec;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`wtreap` decides which worktree directories are disposable. `reap_plan` turns that into an
ordered list of steps. Neither has ever been run against the 327 directories on disk, and the
reason is the same both times: the last thing standing between a decision and a deletion is
the gap between when the plan was BUILT and when it is EXECUTED.

A plan says "unregister then delete `foo--bar`". Between building it and running it, a wave can
start and `foo--bar` can become a live run's worktree. Executing a stale plan then deletes an
agent's working tree mid-run, which this project has already done once by other means: eight of
ten runs destroyed, and the survivors looked like arms producing nothing.

This module re-checks a plan against the world as it is NOW, immediately before each step, and
refuses the steps that no longer hold. It still performs nothing.

## Exact API

```rust
use crate::reap_plan::{Plan, Step};

/// Whether one step may still be performed, checked against current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Perform it.
    Go,
    /// Do not. The world changed since the plan was built.
    Stop {
        /// Which fact changed, for the log a caller writes.
        why: Stale,
    },
}

/// What invalidated a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// A live run now holds this directory. The strongest refusal.
    NowInUse,
    /// The plan says unregister, but git no longer registers it.
    NoLongerRegistered,
    /// The plan says delete an unregistered directory, but it is registered now.
    NowRegistered,
    /// The directory is gone already.
    Absent,
}

/// Re-check one step against the world.
///
/// `live` are directory keys with a running agent. `registered` are the
/// directories git currently lists. `present` are the directories that exist
/// on disk.
pub fn admit(step: &Step, live: &[&str], registered: &[&str], present: &[&str]) -> Admit;

/// What a caller should actually do, in order, having re-checked everything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Admitted {
    /// Steps that still hold, in the plan's original order.
    pub go: Vec<Step>,
    /// Steps refused, each with the reason, in the plan's original order.
    pub stopped: Vec<(Step, Stale)>,
}

/// Re-check a whole plan.
pub fn admit_plan(
    plan: &Plan,
    live: &[&str],
    registered: &[&str],
    present: &[&str],
) -> Admitted;
```

## Falsifiable clauses

1. A `Delete` whose directory is in `live` yields `Stop { why: NowInUse }`, **whatever** the
   other two slices say. Liveness outranks every other signal, as it does in `wtreap`.
2. An `Unregister` whose directory is in `live` also yields `NowInUse`. Removing the
   registration under a running agent is as damaging as deleting the tree.
3. An `Unregister` for a directory NOT in `registered` yields `NoLongerRegistered`. Something
   else already did it; performing it again is an error the caller should not be handed.
4. A `Delete` for a directory that IS in `registered` yields `NowRegistered` -- **even though
   the plan may contain an earlier `Unregister` for it.** Say in the doc why: `admit` sees one
   step and cannot know whether the earlier step ran, so it reports what it observes.
   `admit_plan` is where the sequence is understood (clause 7).
5. A `Delete` for a directory not in `present` yields `Absent`. Already gone is not an error
   and not a success; it is a step that need not run.
6. `Stale` is checked in the order the enum declares, so a directory that is both live and
   absent reports `NowInUse`. Pin the precedence explicitly for every pair you can construct.
7. **`admit_plan` accounts for its own earlier steps.** If it admits an `Unregister` for a
   directory, the following `Delete` for that same directory is judged against a `registered`
   set with that directory REMOVED. Otherwise every correctly-ordered plan refuses its own
   second half, which is clause 4 read without clause 7 and is the trap in this task.
8. Every step of the input plan appears exactly once across `go` and `stopped`, in the plan's
   original order within each. Pin the partition.

## Boundaries, at N and at zero

- An EMPTY plan: an empty `Admitted`, equal to `Admitted::default()`.
- All three slices empty against a non-empty plan: every `Unregister` is
  `NoLongerRegistered` and every `Delete` is `Absent`. Nothing is admitted and nothing is an
  error. Pin this; it is the "someone already cleaned up" case.
- A plan whose every step is refused: `go` empty, `stopped` carrying all of them. This is a
  SUCCESSFUL re-check, not a failure. Say so in the doc.
- A directory appearing in all three slices at once: `NowInUse` by clause 6.
- A name in `live`, `registered` or `present` that matches no step: ignored, not an error.
- Comparison is EXACT string matching, as everywhere in this crate.

## Superset status on every enumerated list

`Admit` is a CLOSED set of two and `Stale` a CLOSED set of four. There is deliberately no
`Stale::Unknown` and no "force" admission: a step this module cannot judge must not be
performed, and a caller wanting to override re-checks with different inputs rather than asking
for a bypass.

`Step` and `Plan` are defined in `reap_plan` and are CLOSED there. **Use them, imported. Do not
define your own.**

## Composition of aggregate returns

`go` and `stopped` each preserve the plan's original relative order, and their lengths sum to
the plan's step count -- state both and pin the sum. `go` is an ORDERED sequence, not a set: a
caller executes it top to bottom, so `Unregister` still precedes the matching `Delete` exactly
as `reap_plan` arranged it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. **No I/O, no `std::fs`, no `std::process`**: the three slices arrive as
  arguments. This module performs nothing and reads nothing.
- Do not change `reap_plan.rs` or `wtreap.rs`.
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
