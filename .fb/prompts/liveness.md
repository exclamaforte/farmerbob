<!-- fb:creates crates/farmerbob-core/src/liveness.rs -->
# Task: liveness — separate run lifecycle from agent liveness

Create `crates/farmerbob-core/src/liveness.rs`, declare it from `lib.rs` with
`pub mod liveness;`. Change nothing else except that one `pub mod` line.

## Context

A run has a lifecycle the harness owns: queued, dispatched, verifying, scored. An *agent*
has a liveness the harness only observes: working, idle, blocked, gone. Conflating them is
how this project repeatedly lied to itself — a frozen log was read as a dead agent when the
launcher was merely buffering output, and a process-name match reported seven live agents
when one was running.

The rule that prevents it: **every liveness belief carries the authority it came from, and a
weaker source may never override a stronger one.** Ranked:

1. **Lifecycle report** — the agent or its wrapper said so. Authoritative.
2. **Cgroup evidence** — the run's own cgroup has live pids and rising CPU. Strong; it
   cannot say what the agent is *doing*, only that it is doing something.
3. **Output heuristic** — the log grew recently. Weak, and wrong for any launcher that
   buffers.

When the best available source cannot decide, the answer is `Unknown`, which is a real
state and not an error. A harness that guesses `Dead` because a log went quiet will kill
healthy work.

**Pure logic: no I/O, no clock reads, no process inspection.** Observations are fed in.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Authority { Output = 1, Cgroup = 2, Lifecycle = 3 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness { Working, Idle, Blocked, Gone, Unknown }

/// One observation about one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub authority: Authority,
    pub liveness: Liveness,
    /// Unix milliseconds. Later observations from the SAME authority supersede earlier ones.
    pub at_ms: u64,
    pub evidence: String,
}

/// What the harness currently believes, and on what grounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Belief {
    pub liveness: Liveness,
    pub authority: Option<Authority>,
    pub at_ms: u64,
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tracker { /* your fields */ }

impl Tracker {
    pub fn new() -> Self;
    /// Fold one observation. See Required behaviour for precedence.
    pub fn observe(&mut self, o: Observation);
    /// The current belief. `Unknown` with `authority: None` before any observation.
    pub fn belief(&self) -> Belief;
    /// Drop every belief from `authority`, e.g. when a lifecycle integration is uninstalled
    /// and its reports must stop counting. The belief recomputes from what remains.
    pub fn release(&mut self, authority: Authority);
    /// An observation from a source this weak or weaker is ignored while a stronger,
    /// unexpired source holds the belief. Exposed so callers can skip gathering it.
    pub fn would_accept(&self, authority: Authority) -> bool;
}

/// A belief older than `max_age_ms` decays to `Unknown` rather than persisting.
/// A stale claim that an agent is Working is worse than admitting ignorance.
pub fn decay(b: &Belief, now_ms: u64, max_age_ms: u64) -> Belief;
```

## Required behaviour

- A higher authority always wins, regardless of arrival order: an `Output` observation
  arriving after a `Lifecycle` one does not change the belief.
- Two observations from the SAME authority resolve by `at_ms`, latest winning. Equal
  timestamps keep the existing belief — an arbitrary tiebreak invents information.
- `release` removes that authority's observations entirely and recomputes from the rest,
  which may drop the belief to `Unknown`.
- `decay` returns `Unknown` with `authority: None` when the belief is older than
  `max_age_ms`, and returns the belief unchanged otherwise. It never *strengthens* a belief.
- `Gone` from `Cgroup` is authoritative over `Working` from `Output`: an empty cgroup means
  no process exists, whatever the log suggests.
- `would_accept` returns true when no belief is held, and true for an authority at least as
  strong as the current one.
- `evidence` is preserved verbatim from the winning observation. A human reading a belief
  must be able to see what produced it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No arithmetic that can overflow on adversarial timestamps; use checked or saturating ops.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a late weak observation loses to an early strong
one; same-authority ties keep the incumbent; `release` can drop the belief to Unknown;
`decay` past the age yields Unknown; an empty cgroup beats a fresh log line; `would_accept`
is true before any observation; evidence survives to the belief.

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
