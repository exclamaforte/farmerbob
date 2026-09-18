<!-- fb:modifies crates/farmerbob-core/src/park_decision.rs -->
# Task: the audit trail dies exactly where the join is built

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/park_decision.rs`. Do not change any other file.

## The defect, found by all three arms independently

`park_decision::decide` merged yesterday. Every one of its three candidates, reviewing a
different rival and none having seen the others' reports, found the same thing:

  "`grounds` dies at the handoff ... `park_after` quotes `grounds` 'for the next reader of the
   parked-until record'; the spec-fixed `Decision` has no field for it, so `decide` must discard
   it. If `Decision` is what the dispatcher persists, the audit trail dies exactly where this
   wave builds the join. Mine discards it too -- evidence about the specification, not the
   patch."

  "Lost diagnostic context: `Park::Backoff` carries a `grounds: String` explaining the backoff
   reason. `decide` discards it via `..` pattern match, and `Decision::ParkFor` has no field to
   preserve it."

A defect in every implementation is evidence about the SPECIFICATION. It was mine: I fixed a
`Decision` enum with no room for the one string the module below it builds specifically to be
read later. This task gives it room.

## Why it matters, concretely

A parked arm is questioned days later: why is `or-hy3` benched? Today the registry can say
`parked_until = <instant>` and nothing else. `park_after` already resolved the reason -- it
builds strings like `"provider stated a reset (resolves to 4600000 ms); log: ..."` -- and
`decide` throws that away. The instant survives; the justification does not. A park whose
reason cannot be recovered is indistinguishable from a park somebody invented, which this
project has already had once, for 28.8 hours, across five arms.

## Exact API

Keep `Decision`, `decide` and every existing item. Change `Decision`'s two park variants to
carry the justification, and add a reader:

```rust
pub enum Decision {
    /// Change nothing. The run says nothing about availability.
    Leave,
    ParkUntil {
        at_ms: u64,
        arms: Vec<String>,
        blast: Blast,
        /// Why, carried verbatim from [`quota::Park`]. Never synthesised
        /// here: this module decides nothing about the reason.
        grounds: String,
    },
    ParkFor {
        backoff_ms: u64,
        arms: Vec<String>,
        blast: Blast,
        /// Why, carried verbatim from [`quota::Park`].
        grounds: String,
    },
}

/// The justification a decision carries, if it parks anything.
///
/// `None` for [`Decision::Leave`] -- a decision that parks nothing has no
/// park to justify, and an empty string would read as "parked for no
/// stated reason", which is the thing this task exists to prevent.
pub fn grounds(d: &Decision) -> Option<&str>;
```

`decide` keeps its exact signature.

## Falsifiable clauses

1. `ParkUntil` carries the `grounds` string from `Park::Until` **byte for byte**. Not trimmed,
   not lowercased, not truncated, not reformatted. Pin equality against the exact string
   `park_after` produced.
2. `ParkFor` carries the `grounds` string from `Park::Backoff`, byte for byte.
3. **`decide` never synthesises a `grounds` string.** If the park carries an empty one, the
   decision carries an empty one. Say in the doc why: a reason this module invented would be
   indistinguishable from one the provider gave, and telling those apart is the entire value of
   the field.
4. `grounds(&Decision::Leave)` is `None`. Pin that it is `None` and not `Some("")`.
5. `grounds` on either park variant returns `Some` of exactly the stored string, including when
   that string is empty -- `Some("")` is a real answer meaning "parked, and the reason recorded
   was empty", which is different from `None`.
6. Adding these fields changes no existing decision: for every case already pinned in this
   module's tests, the variant, `at_ms`/`backoff_ms`, `arms` and `blast` are unchanged. Say in
   your handoff which existing tests you had to update for the new field and confirm none of
   them changed an asserted value.

## Boundaries, at N and at zero

- An EMPTY `grounds` from `park_after`: carried as empty, clause 3. `grounds()` returns
  `Some("")`.
- A `grounds` containing newlines or non-UTF8-looking escapes: carried verbatim. It is a
  `String`; it is not parsed here.
- A very long `grounds`: carried whole. No truncation, and say so -- a truncated reason that
  still looks like a reason is worse than none.
- `Decision::Leave` has no `grounds` field at all, not an empty one. Pin that the variant is
  still a unit variant.

## Superset status on every enumerated list

`Decision` stays a CLOSED set of three variants. This task adds FIELDS, not variants. There is
still no `ParkIndefinitely`: a park with no end is a decision to retire an arm and that is a
human's call.

## Composition of aggregate returns

`arms` keeps its existing contract: sorted, deduplicated, never empty in a park variant. The
new `grounds` field is a `String` and not an `Option<String>` -- a park always has a stated
reason even when that reason is the empty string, and the absence of a park is modelled by
`Leave`, not by a null. Say this in the doc; a reader will otherwise ask why one is `Option` and
the other is not.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O, no clock.
- Do not change `quota.rs`. `Park` already carries `grounds`; this task consumes it.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
  Existing tests in this file must be updated for the new fields; that is expected, and no
  asserted VALUE may change.
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
