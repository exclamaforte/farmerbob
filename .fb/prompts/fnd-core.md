# Task: implement the farmerbob domain model

You are working in a Rust workspace at the root of this git worktree. It already
builds. Do not change any other crate.

## What to write

Replace the file `crates/farmerbob-core/src/lib.rs` with the farmerbob domain model.
You may split it into additional modules under `crates/farmerbob-core/src/` if you
prefer (declare them with `mod`/`pub mod` from `lib.rs`).

farmerbob is an orchestrator that runs many AI coding agents in parallel on one
machine, confines each to a resource budget, serializes their access to a single
GPU, and scores their output. These are the types that describe that world.

Define, with `#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]`
where it makes sense:

1. `AgentKind` — enum over the backends: Claude, Codex, Gemini, Glm, Ifm, OpenRouter.
2. `Agent` — id, kind, model string, provider string.
3. `Task` — id, name, repo path (`PathBuf`), task directory (`PathBuf`).
4. `RunState` — enum with these states and NO others:
   `Queued`, `Starting`, `Running`, `BlockedOnLease`,
   `BlockedOnQuota { reset_at: DateTime<Utc> }`, `Succeeded`, `Failed { reason: String }`,
   `Killed`, `Abandoned`.
5. `Run` — id, task id, agent id, worktree `PathBuf`, branch `String`, slot index
   `Option<u32>`, state `RunState`, `created_at`/`started_at`/`ended_at`.
6. `ResourceSpec` — cpu_quota_percent `u32`, memory_max_bytes `u64`,
   cpuset `Option<String>`, tasks_max `u32`.
7. `Slot` — index `u32`, spec `ResourceSpec`, occupant `Option<RunId>`.
8. `LeaseClass` — `Exclusive` | `Shared { vram_bytes: u64 }`.
9. `Lease` — id, resource name `String`, class, holder run id, token `String`,
   acquired_at, ttl `Duration`.
10. `Experiment` — id, run id, command `String`, metrics `serde_json::Value`,
    correct `Option<bool>`, duration_ms `Option<u64>`, quarantined `bool`.
11. `Grade` — run id, grader `String`, rubric scores (quality, approach, adherence,
    autonomy, honesty — each `u8` 1..=5), rationale `String`, created_at.

Use newtype wrappers for ids (`RunId`, `TaskId`, `AgentId`, `LeaseId`,
`ExperimentId`) rather than bare `String`/`Uuid` — they must not be
interchangeable.

## Required behaviour

- `RunState::is_terminal(&self) -> bool` — true for Succeeded, Failed, Killed, Abandoned.
- `RunState::can_transition_to(&self, next: &RunState) -> bool` — encode the legal
  state machine. A terminal state can transition to nothing. Queued cannot jump
  straight to Succeeded.
- `Run::duration(&self) -> Option<Duration>`.
- `ResourceSpec::default()` — something sane for a 32-core / 30 GB machine running
  ~4 agents at once.

## Rules

- The crate must have NO I/O: no filesystem, no network, no process spawning.
- It must compile: `cargo build -p farmerbob-core` must succeed.
- Write unit tests in the same crate covering `is_terminal`, `can_transition_to`
  (both a legal and an illegal transition), and serde round-tripping a `Run`.
  `cargo test -p farmerbob-core` must pass.
- Available dependencies (already in Cargo.toml, do not add more): serde,
  serde_json, thiserror, chrono, uuid.
- Run `cargo test -p farmerbob-core` yourself and fix anything that fails before
  you finish.

When you are done, state briefly what you implemented.
