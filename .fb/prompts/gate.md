<!-- fb:creates crates/farmerbob-core/src/gate.rs -->
# Task: the scoring gate — proving work happened, not merely that nothing broke

Create `crates/farmerbob-core/src/gate.rs`, declare it from `lib.rs` with
`pub mod gate;`. Change nothing else except that one `pub mod` line.

## Context

This is the most expensive bug this project has had, and it recurred in four separate tools
before anyone noticed the pattern.

`cargo test` on a crate with no tests prints `test result: ok. 0 passed` and exits 0. An
agent that wrote nothing therefore scores `build=pass, test=pass` — indistinguishable from
one that implemented the task correctly. Every naive gate reads "nothing broke" as "it
worked". The same shape appeared again in a conformance suite that could not compile, in a
cross-examination matrix that could not run, and in a reference veto with no reference: an
instrument that cannot perform its check reports success.

So the gate's job is to require positive evidence, and to distinguish *absent* evidence from
*negative* evidence. Those are different facts and collapsing them is what caused every
instance.

**Pure logic: no I/O, no process spawning.** The caller runs the tools and feeds in results.

## Exact API — implement these signatures verbatim

```rust
/// What the harness observed. `None` means NOT MEASURED, which is never the same as zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub built: Option<bool>,
    pub tests_passed: Option<bool>,
    /// Tests that actually EXECUTED. Zero is a real measurement; None means unknown.
    pub tests_run: Option<u32>,
    /// Lines added to the declared target. Zero is real; None means unknown.
    pub lines_added: Option<u32>,
    /// Files the run was declared to create that now exist.
    pub declared_targets_present: Option<bool>,
}

/// Exactly these verdicts and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// Wrote nothing.
    NoOp,
    /// Wrote something that does not build.
    NoCompile,
    /// Builds, tests executed, some failed.
    TestsFail,
    /// Builds and "passes", but NOTHING executed. The empty-suite trap.
    NoTests,
    /// The declared deliverable is absent though lines were written elsewhere.
    WrongTarget,
    /// Not enough was measured to decide. Never a failure.
    Indeterminate,
}

impl Verdict {
    /// Only Pass. Everything else, including Indeterminate, is not a pass.
    pub fn is_pass(self) -> bool;
    /// True when the verdict reflects the ARM's work. Indeterminate does not.
    pub fn blames_arm(self) -> bool;
}

/// Decide. Checked in this order, and the FIRST applicable verdict wins:
///   Indeterminate, NoOp, NoCompile, WrongTarget, TestsFail, NoTests, Pass.
pub fn judge(o: &Observation) -> Verdict;

/// Which fields were not measured. Empty means fully observed.
/// Returned in the declaration order of `Observation`.
pub fn unmeasured(o: &Observation) -> Vec<&'static str>;

/// A one-line explanation naming the deciding evidence, for a human reading a run record.
/// It must name the field that decided, so a wrong verdict can be traced to its input.
pub fn explain(o: &Observation, v: Verdict) -> String;
```

## Required behaviour

- `Indeterminate` when `built`, `tests_passed` or `tests_run` is `None`. A gate that cannot
  see cannot fail an arm. Checked FIRST, before any other rule.
- `NoOp` when `lines_added` is `Some(0)`, regardless of build or test results.
- `NoCompile` when `built` is `Some(false)`.
- `WrongTarget` when `declared_targets_present` is `Some(false)` while `lines_added > 0`.
  `None` here does not block a `Pass` — many tasks declare no target.
- `TestsFail` when `tests_passed` is `Some(false)`.
- `NoTests` when tests "passed" but `tests_run` is `Some(0)`. This is the empty-suite trap
  and it must never read as `Pass`.
- `Pass` only when: built, tests passed, `tests_run > 0`, `lines_added > 0`, and targets are
  present or unstated.
- `blames_arm` is false for `Indeterminate` and true for every other verdict.
- `explain` names the deciding field. For `Pass` it states the positive evidence.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: an empty suite that "passes" is NoTests and NOT
Pass; a missing `tests_run` is Indeterminate rather than NoTests; `Some(0)` and `None` for
`tests_run` give different verdicts; NoOp beats NoCompile when both apply; a `None` target
does not block Pass; `blames_arm` is false only for Indeterminate; `explain` names the
deciding field; `unmeasured` is empty for a fully observed run.

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
4. **Test depth and generality** — number of distinct behaviours covered, not number of
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
