<!-- fb:creates crates/farmerbob-core/src/board.rs -->
# Task: something must turn a directory listing into facts, and it is currently grep

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/board.rs` and add `pub mod board;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`farmerbob_core::attention` ranks what the orchestrator should look at first. It takes
`attention::Facts` and nothing builds one.

What builds it today is `fb-autopilot.sh`, in shell, by inferring state from which files exist:
a `<task>.score.json` means the task ran, a `<task>.claims.json` means the subjective tier
ran, a `.tsv` in the queue directory means a wave is waiting. The inference is right; the
medium is not. It cannot be tested, it silently treats an unreadable directory as an empty one,
and it has been wrong in production -- 228 identical retries of a task whose pipeline had
failed, because the shell could not tell "failed" from "not yet run".

This module is that inference, as a function over a listing the caller supplies.

## Exact API

```rust
use crate::attention::{Facts, Item};
use crate::measurement::Measurement;

/// One task, as the caller found it on disk. Every field is what the caller
/// could actually observe; nothing here is inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFiles {
    /// The task name.
    pub name: String,
    /// A spec exists at `.fb/prompts/<name>.md`.
    pub has_spec: bool,
    /// How many candidates passed the gate, from `<name>.score.json`.
    /// `Missing` when the file is absent or could not be read.
    pub passing: Measurement<usize>,
    /// The subjective tier left a non-empty `<name>.claims.json`.
    pub has_claims: bool,
    /// An adjudication record exists at `.fb/adjudicated/<name>`.
    pub adjudicated: bool,
    /// Stages the last pipeline run reported as failed, in pipeline order.
    /// EMPTY means the last run failed nothing; it does not mean no run.
    pub failed_stages: Vec<String>,
}

/// One queued matrix the caller found in `.fb/queue/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMatrix {
    /// The file's basename, e.g. `wave94.tsv`.
    pub matrix: String,
    /// How many runs its lines would dispatch.
    pub runs: usize,
}

/// Turn a listing into the facts `attention::rank` consumes.
///
/// `live_agents` is passed through unchanged: this module observes nothing
/// itself and never substitutes a count for an absent one.
pub fn observe(
    tasks: &[TaskFiles],
    queued: &[QueuedMatrix],
    live_agents: Measurement<usize>,
) -> Facts;
```

## Falsifiable clauses

1. A task with `failed_stages` non-empty yields `Item::PipelineFailed` carrying those stages in
   the order given, unchanged and unsorted.
2. A task with `passing: Observed(n)`, no claims and no adjudication yields
   `Item::RunPipeline { missing: "critique" }` -- the subjective tier is the stage whose absence
   this is evidence of. Pin the literal `"critique"`, because a caller matching on it needs it
   fixed.
3. A task with `passing: Observed(n)`, claims present, and NOT adjudicated yields
   `Item::Adjudicate { passing: n }`.
4. A task that IS adjudicated yields no item from clauses 2 or 3. It may still yield
   `PipelineFailed` from clause 1: a finished decision does not repair a broken instrument.
5. A task whose `passing` is `Missing` yields no `Adjudicate` and no `RunPipeline`, whatever
   else is true. Not knowing how many passed is not evidence that any did.
6. Every element of `queued` yields one `Item::LaunchWave` carrying its `matrix` and `runs`.
7. `Item::QueueEmpty` appears EXACTLY WHEN `queued` is empty AND no task yielded an
   `Adjudicate`, a `RunPipeline` or a `PipelineFailed`. It is the statement that there is
   nothing to do, so it may not sit beside something to do.

## Boundaries, at N and at zero

- `tasks` and `queued` both EMPTY: the only item is `QueueEmpty`. Clause 7.
- A task with `has_spec: false`: it still yields whatever clauses 1 to 5 give it. A result
  whose spec has been deleted is still a result, and hiding it loses the record.
- `passing: Observed(0)` with claims and no adjudication: `Adjudicate { passing: 0 }`. A field
  where nothing passed still needs a decision recorded. Clause 3 makes no exception for zero.
- `failed_stages` EMPTY: no `PipelineFailed`. The doc on that field says an empty vector means
  the last run failed nothing, and this is where that matters.
- A task appearing TWICE in `tasks` under the same name: it is degenerate, not invalid. Say what
  you do -- both entries yield their items, or the second is ignored -- and **your tests may not
  assert on it either way**, because the caller lists a directory and a directory cannot hold
  two entries of one name.
- A `QueuedMatrix` with `runs: 0`: still a `LaunchWave`. `attention` already pins that a matrix
  dispatching nothing is worth being told about.

## Superset status on every enumerated list

`Item`'s variants are a CLOSED set of exactly five and they are `attention::Item`'s:
`Adjudicate`, `RunPipeline`, `PipelineFailed`, `LaunchWave`, `QueueEmpty`. You define no new
variant and no new enum, and your tests may not assert on anything outside them.

`Measurement` and `Absent` are `crate::measurement`'s. `Facts` is `attention::Facts`.

The stage name in clause 2 is a CLOSED set of exactly one: `"critique"`.

## Composition of aggregate returns

`Facts.items` holds every item the clauses above produce and nothing else, ordered: all items
from `tasks` in the order `tasks` was given, then all items from `queued` in the order `queued`
was given, then `QueueEmpty` if clause 7 admits it. One task producing two items emits them
adjacently, `PipelineFailed` first. `observe` sorts nothing -- ordering within a variant is
`attention::rank`'s to decide, and doing it twice in two places is how two orderings come to
disagree.

`Facts.live_agents` is the argument, unchanged.

Say why clauses 3, 5 and 7 must be tested together rather than separately: each is a rule about
when an item is ABSENT, and an implementation that emits everything always passes any test that
only checks an item is present. The absences are the content of this function.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O and no clock. The caller lists the directory; this maps it.
- `cargo build -p farmerbob-core`, `cargo test -p farmerbob-core` and
  `cargo clippy -p farmerbob-core -- -D warnings` must all pass. All three are clean on HEAD as
  of this task, so any failure is yours.

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
