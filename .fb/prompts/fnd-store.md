<!-- fb:modifies crates/farmerbob-store/src/lib.rs -->
# Task: implement the SQLite storage layer

Rust workspace, already builds. Work only inside `crates/farmerbob-store`.

Write `crates/farmerbob-store/src/lib.rs` (split into modules if you like).

## Context

farmerbob is a daemon that orchestrates AI coding agents. Everything it holds in
memory must be reconstructible from this database, because the daemon has to survive
being killed mid-run and re-adopt or reap what it finds. Durability here is what makes
crash recovery possible.

## What to implement

A `Store` wrapping a `rusqlite::Connection`.

1. `Store::open(path: &Path) -> Result<Store>` — must set:
   `PRAGMA journal_mode=WAL`, `PRAGMA foreign_keys=ON`, `PRAGMA busy_timeout=5000`,
   `PRAGMA synchronous=NORMAL`.
2. `Store::open_in_memory() -> Result<Store>` — for tests.
3. **Embedded migrations.** A `const MIGRATIONS: &[(&str, &str)]` of
   `(version_name, sql)` applied in order inside a transaction, tracked in a
   `schema_version` table. Applying twice must be a no-op. Never re-run an applied
   migration.

## Schema

Tables, with real foreign keys and indexes on the columns you query by:

- `agents`      — id PK, kind, model, provider
- `tasks`       — id PK, name, repo_path, task_dir
- `runs`        — id PK, task_id FK, agent_id FK, worktree, branch, slot INTEGER NULL,
                  state TEXT, state_data TEXT NULL (JSON for states carrying data),
                  created_at, started_at NULL, ended_at NULL
- `leases`      — id PK, resource, class, holder_run_id FK, token, acquired_at, ttl_secs
- `experiments` — id PK, run_id FK, command, metrics TEXT (JSON), correct INTEGER NULL,
                  duration_ms NULL, quarantined INTEGER NOT NULL DEFAULT 0
- `grades`      — id PK, run_id FK, grader, quality, approach, adherence, autonomy,
                  honesty, rationale, created_at
- `run_events`  — id PK autoincrement, run_id FK, at, kind, detail TEXT
                  (append-only audit log; index on (run_id, id))

Timestamps: store as RFC3339 TEXT, UTC.

## Required operations

- insert/get/list for agents, tasks, runs
- `update_run_state(run_id, state, state_data, at)` which ALSO appends a `run_events`
  row in the **same transaction** — a state change and its audit record must never
  diverge
- `runs_by_state(state)` and `active_runs()`
- `record_experiment`, `record_grade`
- `leaderboard()` — per agent: run count, success count, mean grade

## Rules

- Every multi-statement write goes through a transaction.
- No `unwrap()`/`expect()` on fallible paths; return `Result`.
- Available deps: farmerbob-core, rusqlite (bundled), anyhow, thiserror, chrono,
  serde_json. Add none. You may define your own minimal structs rather than depending
  on farmerbob-core's types if that is simpler — but say so.
- Tests using `open_in_memory()` covering: migrations are idempotent (run twice);
  a run round-trips; `update_run_state` writes both the run and its event atomically;
  a foreign key violation is actually rejected.
- `cargo build -p farmerbob-store` and `cargo test -p farmerbob-store` must pass. Run
  them yourself and fix failures before finishing.

When done, briefly state what you implemented.

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
4. **Test depth** — number of distinct behaviours covered, not number of assertions.
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
