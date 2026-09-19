<!-- fb:creates crates/farmerbob-core/src/admit_queue.rs -->
# Task: the wave decides who is dispatchable in bash, and an unregistered name crashes it

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/admit_queue.rs`. Declare it in `lib.rs` with one
`pub mod admit_queue;` line and change nothing else.

## Why this exists

`fb-admit.sh` reads a wave manifest -- rows of `task`, `crate`, comma-separated `arms` -- and
flattens it into a dispatch queue. Three rules decide what comes out, and all three exist
because each was once missing:

1. **An arm absent from the registry ABORTS the wave.** A name nobody registered is a typo, and
   a typo means the matrix does not run what its author believed. A deliberately DISABLED arm is
   different: it is skipped, quietly, and the wave continues. Before this, an unregistered name
   reached the provider lookup and crashed it with a `KeyError` *after* burning a dispatch.
   (bead farmerbob-vgn)

2. **Concurrency is capped per upstream vendor, not only per machine.** Agents are network-bound
   -- measured at about 5% CPU, sitting in `epoll_wait` -- so the machine is not the binding
   constraint, the provider is. One provider rate-limited a run when free-tier calls burst.
   Two arms may be distinct routes to the SAME vendor, and the bucket is what counts.
   (bead farmerbob-4ix)

3. **The queue preserves manifest order**, so the wave dispatches in the order its author wrote.

The shell does this with `ls | wc -l` against a lock directory, a `python3 -c` per arm to read
`sources.toml`, and a `grep` against the registry file. None of it is tested.

## Exact API

```rust
/// What the registry knows about one arm.
pub enum Registration {
    /// Registered and dispatchable. Carries its upstream vendor bucket.
    Enabled { bucket: String },
    /// Registered, but not to be dispatched. Carries a non-empty reason.
    Disabled(String),
    /// Not in the registry at all.
    Unregistered,
}

/// One row of the wave manifest.
pub struct Row {
    pub task: String,
    pub crate_name: String,
    /// Arms, in the order the manifest lists them.
    pub arms: Vec<String>,
}

/// One dispatchable run.
pub struct Run {
    pub arm: String,
    pub task: String,
    pub crate_name: String,
    /// The vendor bucket this run counts against.
    pub bucket: String,
}

/// An arm that was dropped, and why. Reported so a skipped arm is visible.
pub struct Skipped {
    pub arm: String,
    pub task: String,
    pub reason: String,
}

/// Why no queue could be built.
pub enum Abort {
    /// An arm is not registered. Carries its name and its task.
    Unregistered { arm: String, task: String },
    /// Every arm in every row was skipped.
    NoEligibleRuns,
}

/// Flatten a manifest into a dispatch queue.
///
/// `lookup` answers for any arm name. The first unregistered arm aborts, and nothing
/// after it is examined.
pub fn queue(
    rows: &[Row],
    lookup: &dyn Fn(&str) -> Registration,
) -> Result<(Vec<Run>, Vec<Skipped>), Abort>;

/// Whether one more run against `bucket` may start now.
///
/// `in_flight` counts runs currently dispatched, by bucket.
pub fn may_start(bucket: &str, in_flight: &BTreeMap<String, usize>, cap: usize) -> bool;

/// The next run in `queue` that may start, given what is in flight, or `None`.
///
/// Does not reorder the queue: it returns the index of the FIRST run whose bucket
/// has room, so a blocked vendor does not block the wave behind it.
pub fn next_dispatchable(
    queue: &[Run],
    in_flight: &BTreeMap<String, usize>,
    cap: usize,
) -> Option<usize>;
```

`BTreeMap` is `std::collections::BTreeMap`.

## Falsifiable clauses

1. A row of three `Enabled` arms yields three `Run`s, in the manifest's order, each carrying the
   row's task and crate and its own bucket.
2. A `Disabled` arm is omitted from the queue and appears in `Skipped` with the registry's reason
   carried unchanged. The wave is NOT aborted.
3. An `Unregistered` arm yields `Err(Abort::Unregistered)` naming that arm and its task.
4. Clauses 2 and 3 must be pinned in tests that use the SAME manifest and differ only in what
   `lookup` answers. Disabled-versus-unregistered is the distinction this module exists to make,
   and an implementation that treats both as "skip" passes clause 2 and fails only clause 3.
5. `may_start` is true when the bucket's in-flight count is strictly less than `cap`, false when
   it is equal, and false when it is greater. Pin all three, including greater -- the shell can
   overshoot and the recovery behaviour matters.
6. A bucket absent from `in_flight` counts as zero.
7. Two arms with the SAME bucket count against one another. Pin a queue of two arms sharing a
   bucket under `cap: 1`: `next_dispatchable` with one of them in flight must not return the
   other.
8. `next_dispatchable` skips a blocked run and returns a LATER one whose bucket has room. Pin a
   queue where index 0 is blocked and index 1 is not: the answer is 1, not `None`.
9. `next_dispatchable` returns `None` when every remaining run's bucket is full.
10. Rows are processed in order and arms within a row in order; the queue is the concatenation.
    Pin with two rows of two arms and assert all four positions.

## Boundaries, at N and at zero

- ZERO rows: `Err(NoEligibleRuns)`. An empty manifest and a manifest of entirely disabled arms
  reach the SAME error, and clause 2's skip list is still returned to nobody; say in your handoff
  whether you consider that a loss of information, because the spec pins the error and not the
  diagnosis.
- A row with ZERO arms: contributes nothing and is not an error by itself.
- `cap: 0`: `may_start` is false for every bucket, including an absent one, and
  `next_dispatchable` is always `None`. Pin it -- zero is a real configuration and the shell's
  `-ge` comparison gets it right only by accident.
- `cap: 1` with an empty `in_flight`: the first run is dispatchable. This is the smallest case
  where clause 7 can fail silently.
- An empty bucket string: not pinned. Do not assert on it; say what you did in your handoff.

## Superset status on every enumerated list

`Registration` has EXACTLY three variants and `Abort` exactly two. Closed sets.

The rule that a bucket is `quota_bucket`, falling back to `provider`, falling back to
`"unknown"` belongs to the REGISTRY READER, not here. This module receives the bucket already
resolved. Parsing `sources.toml` in this file is a defect.

The registry itself is `farmerbob_core::roster`. If `roster` already expresses `Registration`,
USE IT and say so in your handoff rather than defining a second copy -- `Verdict` exists three
times in this crate because modules were written without sight of one another. If it cannot
express clause 2's reason, name that in your handoff and define `Registration`.

## Composition of aggregate returns

`queue` returns a PAIR. Every arm in the input appears in exactly one of the two vectors, or the
call aborted: the runs and the skips partition the eligible-checked arms. Pin that partition on a
manifest mixing enabled and disabled arms -- lengths summing to the arm count, no arm in both.

`next_dispatchable` returns an INDEX into the queue it was given, never a copy of the run, so
the caller can mark it dispatched. It does not mutate anything.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem, no `std::process`: the registry arrives as a closure.
- `cargo test -p farmerbob-core` and `cargo clippy -p farmerbob-core -- -D warnings` must pass.
  Both are clean on HEAD as of this task, so any failure is yours.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus, ONLY when the task
  CREATES a new file, the one `mod y;` line that declares it, in EITHER the `lib.rs` or the
  `main.rs` beside it. `farmerbob_core::scope::module_declarations_for` permits both, and a
  binary crate such as `fb` has no `lib.rs`, so `main.rs` is the only place the declaration can
  go. A task that MODIFIES an existing file touches one file and no other. Not "only that
  crate" --
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
4. **Test depth and generality** — and read this carefully, because it says something
   different from what it used to say.

   **Test COUNT is not a criterion and never was.** The adjudicating module refuses to rank on
   it, and it is right to: a table-driven test that pins nine behaviours scores one, and a
   suite of forty that restate one scores forty. This rubric claimed for weeks that count was
   scored fourth while the code scored it never, and every arm was told the wrong thing.
   (bead farmerbob-0ce)

   What IS scored is whether your tests would FAIL a wrong implementation. The instruments for
   that are defect injection and running your suite against a rival's code; when neither has
   been run, this criterion is `Missing` and ranks nobody. It is not silently replaced by a
   count.

   So write the tests that falsify the spec's clauses, however few that takes. Test the
   behaviour the specification requires, not your particular implementation's internals.
   Asserting on exact error strings, private field names, or an output format the spec does
   not fix makes a test worthless — and, when a rival's code is run against your suite, makes
   it worse than worthless, because it fails a correct implementation.
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

## A field the spec calls prose is not a field your tests may quote

If the specification describes a string by what it should SAY -- a reason, a grounds, a
diagnostic, a message "for a human reading an adjudication" -- then its exact wording is NOT
pinned, and a test asserting the exact text fails a rival that says the same thing differently.

This has cost a cross-examination cell in three consecutive tasks:

- a test asserting the exact text of `Inconclusive::reason`;
- a test asserting `"fewer than two implementations failed"` and `"no test names were recorded"`;
- a test asserting `reason.contains("fails against merged code")`.

In each case the spec pinned that the string was NON-EMPTY and said what it should convey, and
in each case a rival conveying it in other words was marked wrong.

Test the requirement, not the sentence. If the spec says the reason must say the test failed
against merged code, assert that it mentions failing and mentions merged -- separately,
case-insensitively -- or assert only that it is non-empty. One arm put it exactly right while
reviewing another: check "the semantic requirement ... without asserting on fragile, exact
string formatting that would fail against rival implementations".

The exception is a string the spec quotes verbatim as a value, such as a sentinel like
`"(no output)"`. A quoted literal is pinned; a described one is not.

## Your suite is run against OTHER implementations

This is the rule that has cost the most signal, four times, and it is stated here in terms of
what is MEASURED rather than of what is intended.

Every candidate's test suite is extracted and run against every other candidate's code. A suite
that calls anything the specification does not pin **fails to compile against every rival**, and
those cells are recorded as API-incompatible: you forfeit the cross-examination signal you would
otherwise have earned, however good your tests are.

Four arms have lost it this way, each for an addition that was reasonable on its own:

- a `Registry::new()` / `insert()` pair, used to build test fixtures;
- five tests asserting cases the spec never pinned;
- an inherent method beside the pinned free function, called five times in tests;
- a `DiffLine::added(..)` constructor, used to build test inputs.

None of those are bad code. Add them if they help a caller. **Your tests must go through the
pinned surface anyway** -- construct values from their public fields, call the free function the
Exact API names, and assert only on behaviour the specification fixes. If you would have to call
your own addition to write the test, write the test the longer way.

The rule is not "do not add API". It is "do not make your suite depend on API a rival has no
reason to have".

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
