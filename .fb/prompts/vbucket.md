<!-- fb:creates crates/farmerbob-core/src/verifier.rs -->
# Task: the verifier is its own bucket, and its errors move labels, not artefacts

Create `crates/farmerbob-core/src/verifier.rs`, declare it from `lib.rs` with
`pub mod verifier;`. Change nothing else except that one `pub mod` line.

## Context

farmerbob routes implementation work with a bandit over arms. Verification is different in
kind and must not share that machinery.

When an implementer fails, the artefact is bad. When a *verifier* fails, the artefact may be
fine and the **label** is wrong — and a wrong label is worse than a missing one, because it
propagates into every posterior that consumed it. This project has already been bitten: a
broken conformance suite once indicted all eleven arms at once, and the only reason it was
caught is that unanimity is implausible. A verifier that fails loudly costs one run; a
verifier that fails quietly corrupts a leaderboard.

So a verifier carries its own trust state, earned separately from any implementer's, and the
system must be able to say *which labels to retract* when a verifier is later found faulty.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// A label a verifier produced about one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub verifier: String,
    pub candidate: String,
    pub task: String,
    /// True when the verifier judged the candidate correct.
    pub passed: bool,
    /// Sequence number, so retraction can be ordered. Unique per verifier.
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Never evaluated. Its labels are recorded but must not reach a posterior.
    Untrusted,
    /// Agrees with the consensus often enough to be believed.
    Trusted,
    /// Demonstrated faulty. Its labels are retracted from `since` onward.
    Faulty { since: u64 },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry { /* your fields */ }

impl Registry {
    pub fn new() -> Self;
    pub fn record(&mut self, l: Label);
    /// Promote after `n` labels that agreed with the consensus. Errors if the verifier
    /// is already Faulty: exoneration is a human decision, not an automatic one.
    pub fn promote(&mut self, verifier: &str, n: u32) -> Result<(), String>;
    /// Mark faulty from `since` onward. Idempotent; an earlier `since` always wins,
    /// because the first evidence of a fault bounds what can still be believed.
    pub fn mark_faulty(&mut self, verifier: &str, since: u64);
    pub fn trust(&self, verifier: &str) -> Trust;
    /// Labels that must be withdrawn from every posterior that consumed them.
    /// Ordered by verifier then seq.
    pub fn retracted(&self) -> Vec<&Label>;
    /// Labels safe to consume: from a Trusted verifier, and not retracted.
    pub fn usable(&self) -> Vec<&Label>;
}

/// Unanimity across independent verifiers is evidence about the INSTRUMENT, not the
/// candidates. Returns tasks where every verifier agreed and there were at least
/// `min_verifiers` of them, which are the tasks to re-check first.
pub fn suspicious_unanimity(r: &Registry, min_verifiers: usize) -> Vec<String>;

/// Fraction of a verifier's labels that match the majority label for the same
/// (task, candidate). `None` when it has no label with a comparable peer.
pub fn agreement(r: &Registry, verifier: &str) -> Option<f64>;
```

## Required behaviour

- A label from an `Untrusted` verifier is recorded and returned by `retracted()` never, but
  is excluded from `usable()`. Untrusted is not faulty; it is unproven.
- `mark_faulty` keeps the EARLIEST `since` across repeated calls.
- `promote` on a `Faulty` verifier is an error and leaves the state unchanged.
- `retracted()` returns only labels with `seq >= since` from `Faulty` verifiers.
- A verifier with no labels has trust `Untrusted` and `agreement` of `None`.
- `agreement` counts only (task, candidate) pairs labelled by at least two verifiers; a
  pair only this verifier judged says nothing about it.
- An exact tie in the majority counts as agreement for every verifier, since no majority
  exists to disagree with.
- `suspicious_unanimity` requires at least `min_verifiers` distinct verifiers on a task,
  and returns task ids sorted.
- Recording the same `(verifier, seq)` twice keeps the first and ignores the second.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: an untrusted verifier's labels are not usable and
not retracted; the earliest `since` wins across two mark_faulty calls; promote on a faulty
verifier errors and changes nothing; a label below `since` survives; agreement ignores
singleton pairs; a tie counts as agreement; unanimity below min_verifiers is not reported;
a duplicate seq is ignored.

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
