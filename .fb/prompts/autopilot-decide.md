<!-- fb:creates crates/farmerbob-core/src/autopilot.rs -->
<!-- fb:reads fb-autopilot.sh -->
# Task: the orchestrator's next move, as a decision instead of a shell loop

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/autopilot.rs` and declare it from
`crates/farmerbob-core/src/lib.rs` with `pub mod autopilot;`. Do not change any other file.

`fb-autopilot.sh` decides what the machine does next, every two minutes, forever. That decision
is pure -- it reads counts and filenames and picks one action -- but it lives in a bash loop
interleaved with the I/O that gathers those counts, so it cannot be tested and has never been.
This task extracts the decision. It performs no I/O and launches nothing.

## What the decision actually is

From the script, in its own order, and the ORDER IS THE CONTRACT:

1. If anything is live -- agents running OR a dispatcher still working -- do nothing this tick.
2. Otherwise, if a wave is queued, launch the FIRST by natural sort (`ls -1v`), so wave9 precedes
   wave10. The script's comment explains why this outranks the pipeline: "A pipeline call blocks
   this loop for as long as a critique takes -- ten minutes or more -- so running it ahead of
   dispatch starves the thing the daemon exists to do. Observed: heartbeat 638s stale with wave21
   sitting queued."
3. Otherwise, if some scored task has not been carried through the subjective tier, run its
   pipeline. That tier has no other driver (farmerbob-k9f).
4. Otherwise, idle and say the queue is empty.

## Exact API

```rust
/// What the orchestrator can see when it decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// Agent scopes currently alive.
    pub live_agents: u32,
    /// Dispatcher processes still working, which outlive their agents.
    pub live_dispatchers: u32,
    /// Queued wave filenames, exactly as they appear on disk, in any order.
    pub queued_waves: Vec<String>,
    /// Tasks that have been scored but not carried through the subjective tier.
    pub pipelines_pending: Vec<String>,
}

/// What to do next. Exactly these four and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Something is still running; do not disturb it.
    Wait { live_agents: u32, live_dispatchers: u32 },
    /// Launch this wave file.
    LaunchWave { file: String },
    /// Run this task's pipeline.
    RunPipeline { task: String },
    /// Nothing to do, and nothing running.
    Idle,
}

/// Decide the next action. Pure: no I/O, no clock, no randomness.
pub fn decide(s: &Sighting) -> Action;

/// Natural sort key comparison, so wave9 precedes wave10.
///
/// The script gets this from `ls -1v`. A lexicographic sort would run wave10 before wave9 and
/// silently reorder the queue.
pub fn natural_less(a: &str, b: &str) -> bool;
```

## Falsifiable clauses

1. `live_agents: 3` with everything else populated returns `Wait`, carrying both counts.
2. `live_agents: 0, live_dispatchers: 1` returns `Wait`. A dispatcher outlives its agents and
   the machine is not idle. This is the one most easily got wrong: the script tests BOTH.
3. All quiet with queued waves returns `LaunchWave` for the natural-first, not the
   lexicographic-first: given `["wave10.tsv", "wave9.tsv"]` it is `wave9.tsv`.
4. All quiet, no waves, one pending pipeline returns `RunPipeline` for it.
5. All quiet, no waves, no pipelines returns `Idle`.
6. **A queued wave outranks a pending pipeline.** Given both, the answer is `LaunchWave`. Pin it
   with a comment naming the 638-second stall, so nobody "improves" the order later.
7. `natural_less("wave9.tsv", "wave10.tsv")` is true; `natural_less("wave10.tsv", "wave9.tsv")`
   is false.

## Boundaries, at N and at zero

- Zero of everything: `Idle`.
- `live_agents: u32::MAX`: `Wait`, no overflow.
- One queued wave: launched. Two: the natural-first.
- Names with no digits at all -- `["alpha.tsv", "beta.tsv"]` -- have a defined order: state the
  rule and pin it.
- Equal numeric prefixes with different suffixes -- `wave9a.tsv` against `wave9b.tsv` -- have a
  defined order: state it and pin it.
- Digit runs longer than `u64` -- a 30-digit name -- must not panic and must not wrap. State what
  you do; comparing digit-strings by length then lexically is one correct answer.
- An empty string as a wave name: no panic.

## Superset status on every enumerated list

`Action` is a CLOSED set of four. Do not add a variant; the script has exactly these branches and
a fifth would be a change of behaviour disguised as a refactor.

`Sighting`'s fields are a known subset of what the orchestrator could observe -- it does not yet
carry memory pressure, provider caps or the clock. Say so, and say that `decide` must therefore
never be described as "the" scheduling policy, only as the policy over what it is given.

## Composition of aggregate returns

`decide` returns one `Action`. If you add a helper returning the ordered wave list, its doc must
state what it returns for an EMPTY input and that the order is the natural sort, both pinned.
An empty result must be distinguishable from "not examined" by the caller.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No I/O, no `std::process`, no filesystem, no clock.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
- `crates/fb` must still build; you are adding a module, not changing one.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here and
is scored as one.

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
