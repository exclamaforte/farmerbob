<!-- fb:creates crates/farmerbob-core/src/park_decision.rs -->
# Task: two halves of a park decision exist and nothing joins them

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/park_decision.rs` and add `pub mod park_decision;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

Three modules already in this crate each answer part of one question and none of them talk:

- `quota::park_after(class, log_head, now_ms, default_backoff_ms) -> Park` decides WHETHER a
  refused run should park and until when. Its own doc says it decides about ONE ARM FROM ONE
  RUN and that a caller parking a whole provider is doing something it never authorised.
- `quota::blast_of(log_head) -> Blast` and `quota::arms_in_blast(arm, blast, registry)` decide
  HOW WIDE a refusal reaches: the arm, its vendor bucket, or the whole credential.
- `availability::availability(arm, now_ms)` reads a park back out and distinguishes a live park
  from one whose instant has passed and which nothing revisited.

Nothing calls any of them. Meanwhile, measured on this machine: thirteen dispatch slots in one
day spent on arms that refused before they started, and five free arms sat eligible and
undispatched for 28.8 hours behind a park whose reset instant was invented.

This module is the join. It is still pure -- it writes nothing and reads no clock -- but it
produces the single value a dispatcher can act on.

## Exact API

```rust
use crate::outcome::OutcomeClass;
use crate::quota::{Blast, Park, Registry};

/// What a dispatcher should do to the registry after one run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Change nothing. The run says nothing about availability.
    Leave,
    /// Park these arms until this instant. Arms sorted, deduplicated, and
    /// always containing the refused arm itself.
    ParkUntil {
        /// Epoch milliseconds.
        at_ms: u64,
        /// Every arm the refusal reaches.
        arms: Vec<String>,
        /// How wide, and why this width was chosen.
        blast: Blast,
    },
    /// Park these arms for this long from now. Used when the provider
    /// refused without stating a reset: the instant is not known and must
    /// not be invented.
    ParkFor {
        /// Milliseconds from `now_ms`.
        backoff_ms: u64,
        /// Every arm the refusal reaches.
        arms: Vec<String>,
        /// How wide, and why this width was chosen.
        blast: Blast,
    },
}

/// Join the three halves into one actionable decision.
///
/// `arm` is the arm that ran. `class` and `log_head` come from the finished
/// run. `registry` supplies the provider and bucket of every arm.
pub fn decide(
    arm: &str,
    class: OutcomeClass,
    log_head: &str,
    registry: &Registry,
    now_ms: u64,
    default_backoff_ms: u64,
) -> Decision;
```

## Falsifiable clauses

1. Any `class` for which `park_after` returns `Park::No` yields `Decision::Leave`, whatever
   `log_head` says and whatever the registry contains. `park_after` owns that question
   entirely and this module must not second-guess it.
2. `Park::Until { at_ms }` yields `ParkUntil` carrying that exact `at_ms`, unmodified. No
   rounding, no clamping, no minimum.
3. `Park::Backoff { ms }` yields `ParkFor` carrying that exact `ms`.
4. The `arms` in either park variant are exactly `arms_in_blast(arm, blast_of(log_head),
   registry)`. Not a subset, not a superset, and computed with the SAME `log_head` that
   `park_after` was given.
5. **`Leave` carries no arms and no blast.** A run that teaches nothing about availability
   must not name a blast radius: naming one invites a caller to use it.
6. An arm absent from the registry still parks ITSELF, by `arms_in_blast`'s own rule. Pin that
   the result is a one-element vector containing `arm`, never empty.
7. The `blast` reported is the one used. Pin a case per `Blast` variant where the reported
   value and the membership of `arms` agree.
8. **This module never widens beyond what `blast_of` returned.** An unrecognised refusal is
   `Blast::Arm` and the decision parks exactly one arm. Say in the doc why: widening on a guess
   benches arms that would have worked, and a day of this project's dispatch has already been
   spent proving a wrong confident answer costs more than an admitted narrow one.

## Boundaries, at N and at zero

- `default_backoff_ms == 0` with a refusal that states no reset: `park_after` returns `No` for
  this, so the decision is `Leave`. Pin it here too rather than assuming -- a zero backoff
  parks nothing, and returning `ParkFor { backoff_ms: 0 }` would park until now.
- A reset instant EXACTLY equal to `now_ms`: `park_after` returns `No`, so `Leave`.
- An EMPTY registry: a park still names the refused arm and nothing else.
- An empty `log_head` with a `QuotaLimited` class: `blast_of` gives `Arm` and `park_after`
  gives `Backoff`, so the decision is `ParkFor` over one arm. Pin the whole shape.
- `now_ms == 0`: no arithmetic underflows. Say what you return.

## Superset status on every enumerated list

`Decision` is a CLOSED set of three. It deliberately has no `ParkIndefinitely`: a park with no
end is a decision to retire an arm, which is a human's call and not a dispatcher's.

`Blast`, `Park` and `OutcomeClass` are defined elsewhere in this crate and are CLOSED there.
**Use them, imported. Do not define your own.** Seven core concepts already exist two or three
times in this crate because each was written without sight of the others; `Verdict` exists
three times. A new type whose name already exists is a defect however good its internals.

## Composition of aggregate returns

`arms` is sorted, deduplicated, and never empty in a park variant -- say why it cannot be
empty. `Decision::Leave` has no fields at all; that is deliberate per clause 5, and a reader
should be told so in the doc rather than left to wonder what happened to the blast.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O, **no clock**: `now_ms` is an argument.
- Do not change `quota.rs`, `availability.rs`, `outcome.rs` or any existing signature. This
  module composes them and owns nothing they own.
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
