# Task: implement the `fb` CLI surface

Rust workspace, already builds. Work only inside `crates/fb`. Do not touch other crates.

## Context

`fb` is the command-line interface to farmerbob, an orchestrator that runs many AI
coding agents in parallel on one machine. **Its primary user is another AI agent**, a
planning agent that dispatches work, polls it, compares candidate implementations and
picks a winner. Machine-readable output and branchable exit codes are therefore
features, not polish.

The daemon does not exist yet. You are building the CLI *surface*: the command tree,
the argument types, the output envelope, the exit-code contract, and the pure logic
listed below. Every command that would need the daemon should parse its arguments
fully, then print a clear "daemon not yet implemented" message and exit 1. Structure
the code so a transport can be dropped in later behind one trait or module boundary.

## Command tree

```
fb run [SPEC]                 dispatch a task to N agents
fb status [TRIAL]             everything, or one trial
fb watch [TRIAL]              live status
fb logs <RUN> [-f]            tail one agent's output
fb kill <RUN>
fb compare <TRIAL>            the decision table
fb diff <RUN>
fb select <RUN>               promote a winner
fb sweep <TRIAL>              clean up losers
fb task {add,list,show}
fb agents {list,check}
fb grade <RUN> [...]
fb leaderboard
fb quota status
fb resume <RUN>
fb gpu {acquire,release,status,run}      # agent-facing, used inside a worktree
fb exp {submit,status,result,wait}       # agent-facing
fb daemon {start,stop,status,install}
fb doctor
fb config show
```

### `fb run` in detail

```
fb run "make the lease manager FIFO-safe"     # short form
fb run --spec ./spec.md --agents 6            # spec from a file
cat spec.md | fb run --spec -                 # spec from stdin
```

Flags: `--spec <PATH|->`, `--agents <N|LIST>` (a count, or a comma list of source
ids), `--exclude <LIST>`, `--base <COMMITISH>`, `--wait`, `--json`.

- **Non-blocking by default.** Print the trial id and the queued run ids, then exit.
  `--wait` opts into blocking.
- `--agents` accepts `all`, `all-but <list>`, a count, or an explicit list.

## The two pieces of real logic to implement

Everything else can be a stub. These two must be correct and well tested.

### 1. Content-hashed task identity

```rust
pub fn task_id(spec_text: &str) -> String;
```

A task's id is derived from its **normalised spec text**, so re-running the same spec
lands on the same task and accumulates history across time. This is what makes the
leaderboard meaningful — "codex beat mercury on task X" only means something if X is
the same object next week.

Normalisation before hashing, in this order: strip a leading UTF-8 BOM; convert CRLF
and lone CR to LF; strip trailing whitespace from every line; drop leading and
trailing blank lines; collapse 3+ consecutive blank lines to exactly one.
Do NOT lowercase and do NOT collapse internal spaces.

Id format: `t-` followed by the first 10 lowercase hex characters of a stable digest
of the normalised text. **Implement a small digest function yourself (e.g. FNV-1a
128-bit or a simple SHA-256) — you may not add a dependency.** The same text must
always yield the same id, on any machine, across process restarts.

Tests: identical text → identical id; text differing only by trailing whitespace,
line endings, or surrounding blank lines → identical id; text differing by one
character → different id; two different specs do not collide.

### 2. The `--agents` selector

```rust
pub enum AgentSelector { All, AllBut(Vec<String>), Count(u32), List(Vec<String>) }
pub fn parse_agent_selector(s: &str) -> Result<AgentSelector, String>;
pub fn resolve(sel: &AgentSelector, available: &[String]) -> Result<Vec<String>, String>;
```

`resolve` returns the chosen source ids in a **deterministic** order (do not rely on
hash iteration order). `Count(n)` larger than `available.len()` is an error naming
both numbers. An unknown id in a list is an error naming the unknown id and listing
what is available. Test each of these.

## Output contract

- Global `--json` on every command. Envelope: `{"schema": "<command>", "version": 1,
  "data": {...}}` on success and `{"schema":"error","version":1,
  "error":{"code":"...","message":"..."}}` on failure. Treat it as an API.
- Human output is a plain aligned table, no colour when not a TTY.

## Exit codes — the planning agent branches on these without parsing text

```
0 ok        1 error        2 usage        3 would-block        4 quota-limited
```

Define them as an enum with an explicit `i32` mapping and use it everywhere. Test
the mapping.

## Rules

- Use `clap` with the derive API.
- Never panic: no `unwrap()`/`expect()` on any path reachable from user input.
- Available deps (already in Cargo.toml, add none): clap, anyhow, serde, serde_json,
  toml, directories, farmerbob-core.
- `cargo build -p fb` and `cargo test -p fb` must both pass. Run them yourself and
  fix every failure before you finish.
- Unit tests are required for: `task_id` normalisation (all four cases above),
  `parse_agent_selector`, `resolve` (including both error cases), and the exit-code
  mapping.

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
