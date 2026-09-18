<!-- fb:creates crates/farmerbob-core/src/reap_plan.rs -->
# Task: deciding what may be deleted is not the same as deciding what to do

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/reap_plan.rs` and add `pub mod reap_plan;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`wtreap` decides which worktree directories may be deleted. It is careful: an unregistered
directory with no settled result is `Indeterminate`, a listing that contradicts itself is
excluded, and `safe_to_reap` is a strict subset of `reapable`.

Nothing calls it. There are 327 directories on disk against 124 registrations, and the module
that says which are disposable has never been asked.

The gap is not courage, it is that "may be deleted" and "here is what to do" are different
values. A directory that git still registers needs its registration removed BEFORE the
directory goes, or the admin entry is left dangling -- which `wtreap`'s own doc calls strictly
worse than the disk it would free. An ordered plan is what a caller can execute; a set of names
is not.

## Exact API

```rust
use crate::wtreap::Worktree;

/// One step a caller may perform. Ordered within a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `git worktree remove` this path. Always precedes the [`Step::Delete`]
    /// of the same directory.
    Unregister {
        /// Directory basename.
        dir: String,
    },
    /// Remove this directory from disk.
    Delete {
        /// Directory basename.
        dir: String,
    },
}

/// What a caller should do, and what it must not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Steps in execution order.
    pub steps: Vec<Step>,
    /// Directories deliberately left alone, each with the reason, sorted by
    /// directory name. Present so a plan that does almost nothing can say why
    /// rather than looking like a plan that found nothing.
    pub skipped: Vec<(String, Skip)>,
}

/// Why a directory is not in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// A live run holds it.
    InUse,
    /// Unregistered, not live, and no settled result: the two causes are
    /// indistinguishable, so neither is acted on.
    Indeterminate,
    /// Observations disagree about whether git registers it.
    Conflicted,
    /// Registered with git, and this plan was asked not to unregister.
    RegisteredAndUnregisterNotPermitted,
}

/// Build a plan.
///
/// `may_unregister` gates whether the plan is allowed to touch git's
/// administrative records. When false, a registered directory is skipped
/// entirely rather than half-processed.
pub fn plan(
    wts: &[Worktree<'_>],
    live: &[&str],
    settled: &[&str],
    may_unregister: bool,
) -> Plan;
```

## Falsifiable clauses

1. For any directory that appears as a `Delete`, if that directory is registered then an
   `Unregister` for the SAME directory appears EARLIER in `steps`. Pin this as a property over
   a mixed listing, not only as a single example.
2. An unregistered, reapable directory yields a `Delete` and **no `Unregister`** -- there is no
   registration to remove and issuing one would fail.
3. With `may_unregister: false`, a registered directory produces NO steps and appears in
   `skipped` as `RegisteredAndUnregisterNotPermitted`. Not a `Delete`, not a bare `Unregister`.
4. Every directory in the listing appears EXACTLY ONCE across `steps` and `skipped` -- counted
   by directory name, where a name in `steps` may carry two steps. Pin the partition: no
   directory is silently absent from both.
5. A conflicted directory -- observed both registered and unregistered -- is skipped as
   `Conflicted` and never appears in `steps`, whatever `may_unregister` says. This defers to
   `wtreap::conflicts` and must not re-derive the rule.
6. A directory a live run holds is skipped as `InUse`.
7. **The plan never contains a `Delete` for a directory `wtreap::safe_to_reap` would not
   return.** Pin this as a property: build a plan and a `safe_to_reap` over the same input and
   assert every deleted name is in the latter. This module adds ordering and refusals; it must
   never add permission.

## Boundaries, at N and at zero

- An EMPTY listing: an empty `Plan`, and `Plan::default()` equals it. Pin the equality.
- A listing where NOTHING may be deleted: `steps` empty, `skipped` carrying every directory
  with a reason. Say in the doc that this is a successful plan, not a failure.
- A listing where EVERYTHING may be deleted and all are unregistered: `steps` is exactly one
  `Delete` per directory, in sorted order, and `skipped` is empty.
- The SAME directory name appearing twice with identical entries: it is one directory. It
  appears once in the plan. State what happens and pin it -- `wtreap::census` counts entries
  while `reapable` returns a set, and a plan is about directories, not observations.
- A directory whose name does not split on `--`: `wtreap` calls it `Indeterminate` when
  unregistered, so it is skipped as `Indeterminate`. It is NOT a separate skip reason.

## Superset status on every enumerated list

`Step` is a CLOSED set of two and `Skip` a CLOSED set of four. There is deliberately no
`Step::Force` and no `Skip::Unknown`: a directory this module cannot classify is a defect in
`wtreap`, not a fifth outcome here, and inventing a bucket for it would hide exactly the
ambiguity `wtreap` exists to preserve.

## Composition of aggregate returns

`steps` is in EXECUTION ORDER, which is the whole point: within a directory `Unregister`
precedes `Delete`, and directories are visited in sorted name order so two runs over one
listing produce identical plans. `skipped` is sorted by directory name with one entry per
directory. Say in the doc that `steps` is not sorted as a whole -- it is grouped by directory
and ordered within -- because a reader who sorts it breaks clause 1.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. **No I/O, no `std::fs`, no `std::process`**: this module names steps and
  never performs them. A planner that executes cannot be tested by a test that is wrong about
  what it would execute.
- **Use `wtreap`'s existing functions.** `disposal`, `safe_to_reap` and `conflicts` are merged
  and own their rules. Re-deriving "is this reapable" here is a defect even if it agrees today,
  because the two copies will diverge.
- Do not change `wtreap.rs`.
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
