<!-- fb:creates crates/farmerbob-core/src/availability.rs -->
# Task: an arm that is available and never dispatched looks exactly like one that is gone

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/availability.rs` and add `pub mod availability;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## The measurement

Five free arms carried `parked_until = "2026-09-17T00:00:00+00:00"`. At 2026-09-18T04:55Z they
had been eligible for **28.8 hours** and had appeared in **zero waves**.

Nothing was broken. The eligibility check read the expired park correctly and said yes every
time it was asked. It was never asked, because the only thing that builds an arm list is the
orchestrator, working from the arms it has been using lately. An arm that drops out of the
rotation while parked does not come back on its own, and from the queue's side an expired park
and a live arm are indistinguishable -- both simply "eligible".

The park itself was invented: the provider's refusal said `Rate limit exceeded:
free-models-per-day.` and named no reset instant, so `2026-09-17T00:00:00Z` was a guess at the
next UTC midnight. So the state that needs a name is not "parked" or "ready". It is **"this arm
is dispatchable and nobody has dispatched it"**, and separately **"this park has expired and
the claim that produced it was never checked"**.

## Exact API

The registry row arrives as an argument. It is defined HERE, in full, so that no
implementation has to invent one -- a previous task pinned a type name without saying where it
lived and every arm defined its own.

```rust
/// One registry row, reduced to what availability depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm {
    /// The arm's id, e.g. `"or-nemotron-ultra"`.
    pub name: String,
    /// The registry's `status` field verbatim: `"verified"`, `"disabled"`, `"untested"`, ...
    pub status: String,
    /// `parked_until` as epoch milliseconds, when the row carries one.
    pub parked_until_ms: Option<u64>,
}

/// Whether an arm can be dispatched, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Dispatchable now.
    Ready,
    /// `status` is `"disabled"`. A decision, not a condition; it does not expire.
    Disabled,
    /// `status` is neither `"verified"` nor `"disabled"`.
    Unverified,
    /// `parked_until` is still in the future.
    Parked,
    /// `parked_until` has PASSED. Dispatchable, and distinct from [`Ready`]:
    /// the row still asserts a park, so the claim outlived the condition and
    /// nothing has revisited it.
    ParkExpired,
}

/// Classify one arm.
pub fn availability(arm: &Arm, now_ms: u64) -> Availability;

/// Whether this availability permits dispatch. `Ready` and `ParkExpired` do.
pub fn dispatchable(a: Availability) -> bool;

/// How long since an arm was last dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastSeen {
    /// Dispatched this many milliseconds ago.
    Ago(u64),
    /// Present in the registry and never dispatched at all. NOT the same as a
    /// very old timestamp, and must never be reported as one.
    Never,
}

/// An arm that could run and has not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neglected {
    /// The arm's id.
    pub name: String,
    /// Why it is dispatchable: [`Availability::Ready`] or [`Availability::ParkExpired`].
    pub availability: Availability,
    /// When it last ran.
    pub last_seen: LastSeen,
}

/// Every dispatchable arm that has not run within `stale_after_ms`.
///
/// `last_dispatch` maps an arm name to the epoch-millisecond time it last ran.
/// A name absent from it has never run.
pub fn neglected(
    arms: &[Arm],
    last_dispatch: &[(&str, u64)],
    now_ms: u64,
    stale_after_ms: u64,
) -> Vec<Neglected>;
```

## Falsifiable clauses

1. `status == "disabled"` yields `Disabled` **whatever** `parked_until_ms` says. A disabled arm
   is a decision and a park is a condition; the decision wins and does not expire.
2. `status` neither `"verified"` nor `"disabled"` yields `Unverified`, again whatever the park
   says. Comparison is EXACT: `"Verified"` is not `"verified"`.
3. A verified arm with `parked_until_ms` in the future yields `Parked`.
4. A verified arm with `parked_until_ms` in the PAST yields `ParkExpired`, **not `Ready`**. Say
   in the doc why: the row still asserts a park, so the two differ in what they tell a reader
   about the registry, and collapsing them is what let five arms sit unused for a day.
5. A verified arm with `parked_until_ms: None` yields `Ready`.
6. `dispatchable` is true for `Ready` and `ParkExpired`, false for the other three. Pin all
   five.
7. `neglected` returns only arms for which `dispatchable` is true. A parked or disabled arm is
   never neglected -- it is not being neglected, it is being withheld.
8. An arm in `last_dispatch` whose timestamp is at least `stale_after_ms` old is neglected and
   carries `LastSeen::Ago(now_ms - t)`.
9. **An arm absent from `last_dispatch` is neglected and carries `LastSeen::Never`**, not
   `Ago(now_ms)` and not `Ago(u64::MAX)`. An arm that has never run and one that ran long ago
   are different facts and a reader acts differently on each.
10. An arm dispatched more recently than `stale_after_ms` is NOT neglected.

## Boundaries, at N and at zero

- An EMPTY `arms` slice: `neglected` returns an empty vector.
- `parked_until_ms` EXACTLY equal to `now_ms`: the park has elapsed, so `ParkExpired`. Pin it;
  the other side is defensible and must not be guessed at.
- `stale_after_ms == 0`: every dispatchable arm is neglected, including one dispatched at
  exactly `now_ms`, because zero milliseconds have not elapsed since it ran and the threshold
  is "at least". Pin this rather than special-casing zero.
- An arm dispatched at EXACTLY `now_ms - stale_after_ms`: neglected, by the same "at least".
- A timestamp in `last_dispatch` that is in the FUTURE (greater than `now_ms`): the arm is not
  neglected, and `LastSeen` must not underflow. Say which you return and pin it.
- A name in `last_dispatch` matching no arm: ignored, not an error.
- The same name twice in `last_dispatch`: state which one wins and pin it.

## Superset status on every enumerated list

`Availability` is a CLOSED set of five and `LastSeen` a closed set of two. The STATUS STRINGS
are an OPEN set: `"verified"` and `"disabled"` are the two that carry meaning here and every
other value -- including one added next week -- is `Unverified`. Say both halves, and say that
falling to `Unverified` rather than to `Ready` is deliberate: an unrecognised status must not
become dispatchable by default.

## Composition of aggregate returns

`neglected` returns entries sorted by `name`, ascending byte order, with no duplicates even if
`arms` contains the same name twice -- say which of the duplicate rows is described. An empty
result means every dispatchable arm has run recently, which is a real answer and not an error.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. **No I/O, no clock**: `now_ms` is an argument. A function that reads the
  clock cannot be tested about a boundary at `now`.
- No arithmetic that can overflow or underflow on any input, including a future timestamp.
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
