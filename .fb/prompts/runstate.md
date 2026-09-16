<!-- fb:creates crates/farmerbob-core/src/runstate.rs -->
# Task: the run lifecycle, including being blocked on quota

Create `crates/farmerbob-core/src/runstate.rs`, declare it from `lib.rs` with
`pub mod runstate;`. Change nothing else except that one `pub mod` line.

## Context

A run moves through a lifecycle the harness owns. Most of it is ordinary, and one transition
is not: a run stopped because the provider refused on quota grounds has **not failed**, and
must be resumable when the bucket resets rather than restarted from nothing or scored as a
loss. Runs have already been lost both ways here — scored as arm failures when a provider
cut them off, and silently restarted, discarding work that was on disk.

Two rules follow, and they are the whole difficulty:

- **Blocking is not terminal.** `BlockedOnQuota` must be distinguishable from `Failed` by
  type, not by inspecting a message, because everything downstream branches on it.
- **Resumption must be idempotent.** The quota tracker may report the same bucket due
  several times, and two resume attempts must not produce two running agents against one
  worktree — that is how a dispatcher deletes another dispatcher's work.

**Pure logic: no I/O, no clock reads.** Time enters as a parameter.

## Exact API — implement these signatures verbatim

```rust
/// Exactly these states and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Queued,
    Running { started_ms: u64 },
    /// Stopped by the provider, not by the agent. `resume_at_ms` is `None` when the
    /// provider stated no reset time.
    BlockedOnQuota { bucket: String, since_ms: u64, resume_at_ms: Option<u64> },
    Verifying,
    Done { passed: bool },
    /// Terminal. A run that failed on its own merits.
    Failed { reason: String },
    /// Terminal. Stopped by the orchestrator.
    Cancelled,
}

impl RunState {
    /// True for Done, Failed and Cancelled. BlockedOnQuota is NOT terminal.
    pub fn is_terminal(&self) -> bool;
    /// True only for BlockedOnQuota.
    pub fn is_blocked(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Dispatched { at_ms: u64 },
    QuotaBlocked,
    QuotaReset,
    VerifyStarted,
    Verified { passed: bool },
    FailedNow,
    CancelledNow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// The event is meaningless in this state.
    Illegal { from: String, event: String },
    /// The run is already terminal and cannot move again.
    Terminal { from: String },
}

/// Apply one event. `bucket` and `resume_at_ms` are used only by `QuotaBlocked`.
pub fn step(
    s: &RunState,
    e: Event,
    now_ms: u64,
    bucket: &str,
    resume_at_ms: Option<u64>,
) -> Result<RunState, TransitionError>;

/// A run that may be resumed now: blocked, and either its reset has passed or it stated
/// no reset time and `now_ms - since_ms` is at least `default_window_ms`.
pub fn resumable(s: &RunState, now_ms: u64, default_window_ms: u64) -> bool;

/// Total milliseconds a run has spent blocked, so the leaderboard can report an arm's
/// wallclock without charging it for its provider's billing policy.
pub fn blocked_ms(history: &[(u64, RunState)]) -> u64;
```

## Required behaviour

- `QuotaBlocked` is legal only from `Running`. From anywhere else it is `Illegal`.
- `QuotaReset` is legal only from `BlockedOnQuota`, and returns to `Running` with
  `started_ms` set to `now_ms` — the run resumes, it does not rewind.
- Any event applied to a terminal state is `Terminal`, checked BEFORE `Illegal`, so the
  message names the real reason.
- `CancelledNow` is legal from every non-terminal state, including `BlockedOnQuota`.
- A second `QuotaBlocked` while already blocked is `Illegal`, not a silent no-op. Two
  blocks mean the caller lost track of the run, and hiding that produces two resumes and
  two agents on one worktree.
- `resumable` is false for a blocked run whose `resume_at_ms` is in the future, true once
  `now_ms >= resume_at_ms`, and for `None` becomes true only after `default_window_ms` has
  elapsed since `since_ms`.
- `blocked_ms` pairs each `BlockedOnQuota` entry with the next entry that is not blocked,
  and counts a still-blocked tail as zero rather than guessing an end. It saturates rather
  than overflowing, and ignores entries whose timestamps go backwards.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No arithmetic that can overflow or underflow on adversarial timestamps.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: QuotaBlocked from Queued is Illegal; a double
block is Illegal; an event on Done reports Terminal not Illegal; cancel works while blocked;
QuotaReset sets started_ms to now; resumable respects a stated reset and falls back to the
window for None; blocked_ms ignores a still-blocked tail; backwards timestamps do not
underflow.

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
