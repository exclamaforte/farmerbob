<!-- fb:creates crates/farmerbob-core/src/task_contract.rs -->
# Task: implement the experiment task contract

Create `crates/farmerbob-core/src/task_contract.rs`, declare it from `lib.rs` with
`pub mod task_contract;`. Change nothing else.

## Context

farmerbob runs benchmark experiments on a single GPU, one at a time, and scores them. A task
is a directory of scripts plus a manifest. This module parses the manifest, validates it, and
interprets what the scripts emit — all pure logic, so the scoring rules are testable without
a GPU.

The scoring rule that matters: **a wrong kernel scores zero regardless of speed.** Correctness
gates everything.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TaskName(pub String);

/// What kind of evidence a task's success rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Verification { DeterministicTests, Benchmark, ReviewerJudgement }

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskManifest {
    pub name: TaskName,
    pub description: String,
    pub verification: Verification,
    /// Seconds. Zero or absent means no limit.
    pub timeout_s: u64,
    /// Named resources this task needs held exclusively, e.g. "gpu0".
    pub exclusive: Vec<String>,
    /// Minimum measured trials before a benchmark result may be scored.
    pub min_trials: u32,
}

/// What `bench.sh` printed on stdout.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchOutput { pub ms: f64, pub metrics: serde_json::Value }

/// What `verify.sh` printed on stdout.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifyOutput { pub correct: bool, pub detail: String }

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Score {
    /// Correct and measured. `speedup` is baseline_ms / measured_ms.
    Scored { speedup: f64, trials: u32 },
    /// Ran, but wrong. Speed is irrelevant.
    Incorrect { detail: String },
    /// Measured but not trustworthy: too few trials, or variance above threshold.
    Unreliable { reason: String },
    /// Could not be measured at all.
    NotMeasured { reason: String },
}

impl TaskManifest {
    /// Parse from TOML. Reject a manifest with an empty name, or with `min_trials == 0`
    /// when `verification == Benchmark` — a benchmark scored from zero trials is not a
    /// measurement.
    pub fn parse(toml_src: &str) -> Result<TaskManifest, String>;
}

/// Score one experiment.
///
/// `baseline_ms` is the locally-measured reference time. `samples` are the per-trial
/// measurements in milliseconds.
///
/// Rules, in order:
/// 1. `verify.correct == false` -> `Incorrect`. Nothing else is considered.
/// 2. fewer than `manifest.min_trials` samples -> `Unreliable`.
/// 3. any sample <= 0.0, or a non-finite sample or baseline -> `NotMeasured`.
/// 4. relative spread `(max - min) / median` above `max_spread` -> `Unreliable`.
/// 5. otherwise `Scored` with `speedup = baseline_ms / median(samples)`.
pub fn score(
    manifest: &TaskManifest,
    verify: &VerifyOutput,
    samples: &[f64],
    baseline_ms: f64,
    max_spread: f64,
) -> Score;

/// Median of a slice, `None` when empty. Must not mutate the caller's slice.
pub fn median(samples: &[f64]) -> Option<f64>;
```

## Rules

- Correctness is checked before anything else. A fast wrong answer is `Incorrect`, never
  `Scored`.
- `median` handles even and odd lengths, and must not panic on NaN.
- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: an incorrect-but-fast result scores `Incorrect`;
too few trials gives `Unreliable` even when correct and fast; a zero or negative sample gives
`NotMeasured`; high spread gives `Unreliable`; `median` on even and odd lengths; a manifest
with `Benchmark` and `min_trials == 0` is rejected by `parse`.

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
