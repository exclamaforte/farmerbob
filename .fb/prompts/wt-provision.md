<!-- fb:creates crates/farmerbobd/src/worktree.rs -->
# Task: implement git worktree provisioning

Rust workspace, already builds. Work only inside `crates/farmerbobd`.

Create `crates/farmerbobd/src/worktree.rs`, declare it from `main.rs` with `mod worktree;`.

## Context

farmerbob gives every agent run its own git worktree and branch, so several agents can
attack the same task independently and their results can be compared and merged
selectively. Worktrees live OUTSIDE the repo (default
`$XDG_DATA_HOME/farmerbob/worktrees`) so agents cannot see each other's attempts.

## What to implement

```rust
pub struct WorktreeSpec {
    pub repo: PathBuf,
    pub root: PathBuf,       // where worktrees are created
    pub run_id: String,
    pub task: String,
    pub agent: String,
    pub base: String,        // commit-ish to branch from
}
pub struct Worktree { pub path: PathBuf, pub branch: String, pub base_commit: String }
```

1. `fn branch_name(task, agent, run_id) -> String` — `fb/<task>/<agent>/<run-id>`,
   **sanitised** to be a legal git ref: no spaces, no `..`, no leading/trailing `/` or
   `.`, no `~^:?*[\`, no control characters, not ending in `.lock`. Pure function — test it hard.
2. `fn create(spec) -> anyhow::Result<Worktree>` — resolve `base` to a commit sha, then
   `git worktree add -b <branch> <path> <base>`. Fail cleanly if the path exists or the
   branch is taken.
3. `fn remove(repo, path, force: bool) -> Result<()>` — `git worktree remove`, then
   `git worktree prune`.
4. `fn archive_branch(repo, branch, task, run_id) -> Result<String>` — copy the branch
   to `refs/fb/archive/<task>/<run-id>` before deletion, so a swept loser is recoverable.
   Return the archive ref.
5. `fn list(repo) -> Result<Vec<WorktreeEntry>>` — parse `git worktree list --porcelain`
   into `{path, head, branch, detached, locked, prunable}`. **Parse function must take
   `&str` and be unit-tested** against real porcelain output including a detached entry
   and a `bare` entry.
6. `fn diffstat(repo, branch, base) -> Result<DiffStat>` — files changed, insertions,
   deletions, via `git diff --numstat`. Parser takes `&str`, unit-tested.

## Rules

- Run git via `std::process::Command`; never shell out through `sh -c` (paths may
  contain spaces).
- Never panic. Every fallible operation returns `Result` with context.
- Only ever remove a worktree under `spec.root` — guard against deleting anything else.
- Available deps: farmerbob-core, tokio, anyhow, serde, serde_json, tracing. Add none.
- Unit tests required for `branch_name` sanitising (at least 5 nasty inputs), the
  `worktree list --porcelain` parser, and the `--numstat` parser. These must not touch
  a real repo.
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
