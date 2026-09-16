<!-- fb:creates crates/farmerbobd/src/cgroup.rs -->
# Task: implement cgroup confinement via systemd transient scopes

Rust workspace, already builds. Work only inside `crates/farmerbobd`.

Create `crates/farmerbobd/src/cgroup.rs`, declare it from `main.rs` with `mod cgroup;`.

## Context

farmerbob runs several AI coding agents concurrently on one machine. Each must be
confined so a runaway agent cannot thrash the box or starve the others. On this
machine (Arch, systemd 261, cgroup v2) `user.slice` has `cpu`, `memory` and `pids`
delegated — `cpuset` is NOT delegated by default.

Killing the scope must kill the agent's **entire process tree**, including any
benchmark child it spawned. That is the whole point: a stray child surviving its run
would corrupt a later GPU measurement.

## What to implement

```rust
pub struct ResourceLimits {
    pub cpu_quota_percent: Option<u32>,   // -> CPUQuota=400%
    pub memory_max: Option<u64>,          // bytes -> MemoryMax=
    pub memory_high: Option<u64>,         // bytes -> MemoryHigh=
    pub tasks_max: Option<u32>,           // -> TasksMax=
    pub allowed_cpus: Option<String>,     // -> AllowedCPUs=0-7  (needs cpuset delegation)
    pub io_weight: Option<u32>,           // -> IOWeight=
}
```

1. `fn build_command(unit: &str, limits: &ResourceLimits, argv: &[String]) -> Vec<String>`
   — build the full `systemd-run --user --scope --quiet --unit=<unit> -p K=V ... -- argv`
   argument vector. **Pure function, no process spawning.** This is the part that gets
   unit-tested.
2. `fn spawn(unit, limits, argv, cwd) -> std::io::Result<Child>` — run it.
3. `fn kill_scope(unit: &str) -> std::io::Result<()>` — `systemctl --user stop <unit>.scope`,
   which tears down the whole tree.
4. `fn scope_state(unit: &str) -> ScopeState` — query
   `systemctl --user show <unit>.scope --property=ActiveState,Result` and map to
   `Active | Failed{result} | Gone`. Needed so the daemon can tell, after a crash,
   whether a run it thinks is alive actually still is.
5. `fn cgroup_stats(unit: &str) -> Option<CgroupStats>` — read from the scope's cgroup
   directory: `cpu.stat` (usage_usec, throttled_usec), `memory.current`, `memory.peak`,
   `memory.events` (look for `oom_kill`), `pids.current`. Returns None if the scope is gone.

## Rules

- **Do not hardcode a uid or username.** Derive the cgroup path from
  `/proc/self/cgroup` or `$XDG_RUNTIME_DIR`.
- Omit a `-p` flag entirely when its `Option` is `None` — do not emit `MemoryMax=0`.
- Never panic; return Results.
- Available deps: farmerbob-core, tokio, anyhow, serde, serde_json, tracing. Add none.
- Unit tests: `build_command` with all limits set, with none set (must emit no `-p`),
  and a parser test for `cpu.stat`/`memory.events` text given as a `&str`. Do not write
  tests that actually spawn systemd units.
- `cargo build -p farmerbobd` and `cargo test -p farmerbobd` must pass. Run them
  yourself and fix failures before finishing.

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
