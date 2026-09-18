<!-- fb:modifies crates/farmerbob-core/src/quota.rs -->
# Task: a reset nobody can observe is not a reset

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/quota.rs`. Do not change any other file.

## The defect, found by a critic and confirmed

`QuotaTracker` tracks parked buckets. `due(now)` yields the buckets whose park has elapsed and
**deletes them from the map**. `succeeded(bucket)` documents a "consecutive-park backoff" reset.

Because `due` deletes the entry, `succeeded` has nothing to reset. The existing test
`succeeded_resets_attempts` passes VERBATIM with the body of `succeeded` deleted -- a check
whose failure state is indistinguishable from its success state, which is this project's
recurring bug.

It is worse than a dead function. The escalation is observable and depends on polling:

    park at t=0, window 10, limit again at t=20
      caller never called due()   ->  attempts escalates, until = now + 2*window = 40
      caller called due() at t=15 ->  entry gone, attempts resets, until = now + window = 30

Identical event history, two different parks, decided by whether someone happened to drain the
iterator. That is a boundary the spec never pinned and the caller cannot see.

## The ruling

**`due` must not delete.** Draining an iterator is a READ, and a read must not change what a
later read returns. A bucket's park history survives until something explicitly clears it, and
the only thing that clears it is `succeeded`.

## Exact API

Keep every existing public item in `quota.rs` -- `Bucket`, `LimitHit`, `BucketState`,
`ResumeHandle`, `QuotaTracker`, `Park`, `park_after`, `Blast`, `Source`, `Registry`,
`arms_in_blast`, `blast_of` -- with their current signatures. Change `due`'s BEHAVIOUR, not its
signature, and add:

```rust
/// Whether this bucket is currently parked, without changing anything.
///
/// A pure query. Calling it any number of times returns the same answer and
/// leaves `due` returning the same answer it would have returned.
pub fn is_parked(&self, bucket: &Bucket, now_ms: u64) -> bool;

/// How many consecutive parks this bucket has accumulated.
///
/// `None` when the tracker has never parked it. `Some(0)` is impossible:
/// a bucket that is known has been parked at least once. Say why in the doc.
pub fn attempts(&self, bucket: &Bucket) -> Option<u32>;
```

## Falsifiable clauses

1. **`due(now)` is idempotent.** Calling it twice with the same `now` returns the same set both
   times. Pin this directly: it is the whole task.
2. After `due(now)` yields a bucket, `attempts(bucket)` is unchanged -- still `Some(n)`, not
   `None`.
3. `succeeded(bucket)` is the ONLY thing that clears a bucket's history. After it,
   `attempts(bucket)` is `None`.
4. **`succeeded_resets_attempts` must now FAIL if the body of `succeeded` is removed.** Write a
   test that would catch that deletion, and say in its comment that the old one did not.
5. The polling-independence property, pinned as a test: park at t=0 with window 10, limit again
   at t=20, and assert the resulting park is the SAME whether or not `due` was called at t=15.
   Name the test for the property, not for the mechanism.
6. `is_parked` never mutates. Pin it by asserting `due` returns the same thing before and after
   a run of `is_parked` calls.
7. `attempts` returns `None` for a bucket the tracker has never seen, and for one cleared by
   `succeeded`. These two are the same answer to different questions; say in the doc that the
   type cannot distinguish them and that no caller should need to.

## Boundaries, at N and at zero

- `due(now)` when NOTHING is parked: an empty result, and no mutation.
- A bucket whose park instant is EXACTLY `now_ms`: elapsed, so `due` yields it and `is_parked`
  is false. Pin both halves together -- they must agree at the boundary or a caller sees a
  bucket that is due and parked at once.
- `succeeded` on a bucket never parked: not an error, `attempts` stays `None`.
- `due` called twice with an ADVANCING `now`: the second call still yields a bucket the first
  yielded, if nothing cleared it. Pin this; it is clause 1 with the clock moving and it is the
  case a reader expects to behave differently.
- A bucket parked, drained, then limited again: `attempts` is 2, not 1.

## Superset status on every enumerated list

Every existing enum in `quota.rs` keeps its current variants and closed status. This task adds
no variants. The set of BUCKET NAMES is open.

## Composition of aggregate returns

`due` keeps its current return type and now returns the same membership on repeated calls with
the same `now`. State the ORDER it returns in and pin it -- the existing code inherits map
order, and a caller that logs the result gets a different log line each run if it is not fixed.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O, no clock.
- Do not change any existing signature. `park_after`, `blast_of` and `arms_in_blast` are merged
  and depended on; leave them exactly as they are.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
  Existing tests that encoded the deleting behaviour must be UPDATED and you must say which in
  your handoff.
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
