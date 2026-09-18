<!-- fb:creates crates/farmerbob-store/src/batch.rs -->
# Task: a half-written wave is worse than an unwritten one

Rust workspace, already builds. Work only inside `crates/farmerbob-store`.
Create `crates/farmerbob-store/src/batch.rs` and add `pub mod batch;` to
`crates/farmerbob-store/src/lib.rs`. Do not change any other file.

**This is not a pure-logic task.** It is real SQLite, real transactions, real partial failure.
Every other task in this project's recent history has been a single file of pure functions;
this one owns a database connection and has to be correct when the write fails halfway.

## Why this exists

`Store::insert_run` writes one row and returns. A wave dispatches three to six runs and the
caller loops. If the fourth insert fails -- a duplicate id, a constraint, a disk error -- the
first three are already committed and the wave exists in the database as a partial record that
no later reader can distinguish from a wave that only ever had three arms.

This project has a name for that shape: a failure state indistinguishable from a success state.
It has cost it a merged patch, five stranded tasks and a void matrix. In the store it would
cost the adjudication record itself.

## Exact API

```rust
use crate::{Store, StoreError};
use core::run::Run;

/// What a batch write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// How many rows were committed. Equals the input length, or zero.
    pub committed: usize,
}

/// Which element of a batch rejected the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Zero-based position in the input slice.
    pub index: usize,
    /// The store's own error, unchanged.
    pub cause: String,
}

/// Insert every run, or none.
///
/// On failure the transaction is rolled back and the database is left
/// exactly as it was found.
pub fn insert_runs(store: &Store, runs: &[Run]) -> Result<Written, Rejected>;
```

`Store` and `StoreError` already exist. **Use them, imported.** Do not define your own error
type, do not add a field to `Store`, and do not change `insert_run`.

## Falsifiable clauses

1. A batch of N valid runs commits all N and returns `Written { committed: N }`. Verify by
   reading them back with the existing `get_run`, not by trusting the return value.
2. **A batch where element `k` fails commits NOTHING.** After the error, `get_run` returns
   `None` for every element of the batch, including the ones before `k`. This is the clause the
   task exists for and it must be tested by reading the database back.
3. `Rejected::index` is the zero-based position of the failing element, not its id and not a
   count. Pin a failure at index 0 and one at the last index.
4. `Rejected::cause` carries the underlying store error's text unchanged. Do not reword it: the
   caller needs the database's own message, and a summary loses the constraint name.
5. **An EMPTY batch is `Ok(Written { committed: 0 })`**, not an error. Nothing to write is not
   a failure, and a caller looping over an empty wave must not have to special-case it.
6. A batch containing the same run twice fails, because the second insert violates the primary
   key. It must fail as clause 2 requires -- neither copy committed -- rather than committing
   the first. Pin it.
7. Rows written by a batch are readable by the EXISTING single-row API, and rows written by
   `insert_run` are visible to a later batch's conflict checks. This module adds a way to
   write, not a second table.
8. **A failed batch leaves no transaction open.** After a rejection, a subsequent
   `insert_run` on the same `Store` succeeds. Pin it: a rollback that leaves the connection
   unusable turns one bad row into a dead store.

## Boundaries, at N and at zero

- N = 0: clause 5.
- N = 1, valid: committed 1. N = 1, invalid: `Rejected { index: 0, .. }` and nothing written.
- A batch whose FIRST element fails: nothing written, index 0.
- A batch whose LAST element fails: nothing written, index N-1, and the N-1 valid rows before
  it are absent when read back.
- Two separate batches: the first's rows are still present after the second is rejected.
- The same `Store` used after a rejection is fully usable, per clause 8.

## Superset status on every enumerated list

There is deliberately no partial-success variant and no `force` flag. All-or-nothing is the
entire contract, and an API that can return "four of six" recreates the state this module
exists to prevent. `Written` carries a count rather than a bool because a caller logs it, not
because zero and N are the only values it may hold -- they are.

## Composition of aggregate returns

`Written::committed` equals `runs.len()` on success and the function does not return on
failure, so there is no partial count anywhere in the type. Say that in the doc: a reader
should not have to infer it from the absence of a variant.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. `rusqlite` is already a dependency of this crate; use its transaction
  support rather than hand-rolling BEGIN/COMMIT strings.
- Tests use `Store::open_in_memory()`. Do not write to disk in a test.
- `cargo build -p farmerbob-store` and `cargo test -p farmerbob-store` must pass. Run them.
  Also run `cargo test` for the workspace and say in your handoff whether anything else moved.
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
