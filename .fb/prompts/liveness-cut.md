<!-- fb:modifies crates/farmerbob-core/src/liveness.rs -->
# Task: decide when a run has stopped, without believing a silent log

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/liveness.rs`. Do not change any other file.

`liveness.rs` already models what an agent is DOING -- `Liveness`, `Authority`, `Observation`,
`Belief`, `Tracker`, `decay`. Read it first; you are extending it, not replacing it. Nothing in
it decides whether a run should be CUT, and that is the whole of this task.

## The incident

On 2026-09-17 a run was dispatched and was still running 21 minutes later. Measured:

    CPU consumed    26 seconds across 21 minutes
    its own log     0 bytes -- a peer on the same task wrote 25,596
    worktree        no file created
    typical run     100-350 seconds on that task

It had stopped. Nothing in the harness could say so, and because a stalled run holds the wave
lock, it held the entire queue for as long as it hung. A human read `ps` and killed it.

## The trap this task exists to avoid

The obvious rule is "cut it when its log goes quiet". **That rule is wrong, and this module
already says why** in its own header:

> A frozen log gets read as a dead agent when the launcher is merely buffering output.

`Authority::Output` is ranked WEAKEST for exactly this reason. A cut decision that keys on log
silence would kill healthy runs on any buffering launcher, and this repo has several.

What distinguished the real stall was not the silence. It was silence **together with a cgroup
that had live pids and flat CPU** -- 26 seconds consumed in 21 minutes. `Authority::Cgroup`
outranks `Output` and is what a cut must rest on. A run whose log is silent while its CPU climbs
is WORKING, and must survive.

Pin that distinction in tests. It is the point of the task.

## Exact API

Keep every existing public item working unchanged: `Authority`, `Liveness`, `Observation`,
`Belief`, `Tracker`, `decay` and their signatures. `crates/fb/src/score.rs` imports several.

Add exactly this, public:

```rust
/// Limits a caller supplies. There is no default: a budget is a policy decision and the
/// caller owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutBudget {
    /// Hard wall-clock ceiling. A run past this is cut whatever it is doing.
    pub max_total_ms: u64,
    /// How long a run may be believed `Idle` before it is cut.
    pub max_idle_ms: u64,
    /// How long a run may be believed `Blocked` before it is cut. Separate from
    /// `max_idle_ms` deliberately: blocked-on-network is ordinary and often longer-lived
    /// than an idle loop, and collapsing them forces one number to serve two behaviours.
    pub max_blocked_ms: u64,
}

/// Whether to end a run, and on what grounds.
///
/// NOT an exhaustive account of every reason a run can end. It is the set this function
/// decides; a caller may end a run for reasons this type knows nothing about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cut {
    /// Let it run. Carries why, so a caller can log a decision it did not take.
    Keep { because: String },
    /// End it. `because` is written for a human reading an incident later.
    Cut { because: String },
    /// Refuse to decide. The evidence does not support ending a run, and ending one on
    /// insufficient evidence is worse than waiting.
    Insufficient { missing: String },
}

/// Decide whether a run should be cut.
///
/// `elapsed_ms` is the run's age. `now_ms` is the current instant, used to age the belief.
pub fn cut(belief: &Belief, elapsed_ms: u64, now_ms: u64, budget: &CutBudget) -> Cut;
```

## Falsifiable clauses

State each as a test.

1. A belief of `Working` from `Authority::Cgroup`, with `elapsed_ms` under `max_total_ms`,
   returns `Keep`.
2. A belief of `Working` whose ONLY authority is `Output` does not produce `Cut` when the run is
   otherwise within budget. A weak source may not end a run.
3. **The buffering launcher.** `Liveness::Working` from `Authority::Cgroup`, log silent for
   longer than `max_idle_ms`, returns `Keep`. A silent log against a live cgroup is not a stall.
4. **The real stall.** `Liveness::Idle` from `Authority::Cgroup`, held longer than
   `max_idle_ms`, returns `Cut`, and `because` names the idle duration.
5. `Liveness::Blocked` held longer than `max_idle_ms` but under `max_blocked_ms` returns `Keep`.
   Past `max_blocked_ms` it returns `Cut`.
6. `elapsed_ms` past `max_total_ms` returns `Cut` regardless of liveness -- including
   `Working`. Say in the doc comment why the hard ceiling outranks apparent progress.
7. A `Belief` with `authority: None` -- nothing has been observed -- returns `Insufficient`,
   NEVER `Cut`. This is the bug this whole project keeps having: "no evidence of progress" and
   "evidence of no progress" must not produce the same decision.
8. A belief older than the run itself, or with `at_ms` in the future relative to `now_ms`,
   returns `Insufficient` rather than computing a negative or wrapped duration. Do not panic and
   do not saturate silently.
9. `Liveness::Gone` returns `Cut` with a `because` distinguishing it from a stall: the run ended
   on its own and there is nothing to kill.
10. `Liveness::Unknown` returns `Insufficient`, not `Cut`. The module's own docs say callers must
    not treat `Unknown` as `Gone`.

## Boundaries, at N and at zero

- `max_total_ms: 0` means every run is instantly over its ceiling. Return `Cut`. Do not special-
  case zero into "no limit" -- a caller that wants no limit passes `u64::MAX`, and silently
  reinterpreting zero is how a config typo disables a safety limit. Say this in the doc comment.
- `max_idle_ms: 0` cuts on the first `Idle` observation. Legal, and a caller's choice.
- `elapsed_ms: 0` with a healthy belief returns `Keep`.
- All three budget fields at `u64::MAX`: never cut on duration; only `Gone` still cuts.
- Arithmetic must not overflow or wrap at `u64::MAX`. Use saturating operations and pin one test
  at the maximum.

## Superset status on every enumerated list

`Cut` must document that it is **the set this function decides, not every way a run can end**.
A caller kills runs for reasons outside this module -- an operator, a provider refusal, a machine
reboot -- and nothing here may imply `Keep` means "this run is healthy". It means "this function
found no ground to end it", which is weaker, and the doc comment must say so in those terms.

Do not add variants to `Liveness` or `Authority`. If you believe one is missing, say so in the
handoff with the case it cannot express, and leave the type alone.

## Composition of aggregate returns

If you add any function over several runs or several beliefs, its doc comment must state what it
returns for an EMPTY input and whether the result is ordered, and both must be pinned by tests.
An aggregate whose empty case is unstated is exactly how "nothing was stalled" and "nothing was
examined" become the same answer.

## Use the crate's own types

`farmerbob_core::measurement::Measurement<T>` is `Observed(T)` or `Missing(Absent)`, with no
`unwrap`, no `unwrap_or`, no `Default` and no `From<Option>`. Note that `Cut::Insufficient`
already carries the "could not determine" case for this function, so do NOT wrap the return in
`Measurement` as well -- one representation of absence per value. If you add a helper that can
fail to determine something else, that one returns `Measurement`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Add no dependencies.
- `cargo build` and `cargo test` must pass for the whole workspace. Run them yourself.
- Do not make parameters generic. Concrete types only.
- Do not weaken or delete an existing test to make a new one pass. If an existing test encodes
  the bug, change it and say which one and why.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment saying what you could not verify. Returning less with a stated reason is a correct answer
here and is scored as one. A previous submission in this series shipped a file that emitted its
script's exact output format with the numbers hardcoded, and passed build, scope, lint and its
own tests, because every other gate in this harness is a gate on form. This one is read.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- only the crate named in the task is modified

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
   it. Count is the fallback, not the target:
   assertions. Your tests must be good enough to catch a bug in **any** correct-looking
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

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
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
