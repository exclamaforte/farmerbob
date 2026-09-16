<!-- fb:creates crates/farmerbob-core/src/adapter.rs -->
# Task: implement the agent adapter contract

Create `crates/farmerbob-core/src/adapter.rs`, declare it from `lib.rs` with
`pub mod adapter;`. Change nothing else.

## Context

farmerbob dispatches work to many different agent CLIs. Twenty-four are configured on this
machine and they disagree about almost everything: argv shape, whether they honour the working
directory, how they stream output, how they report a usage limit, whether they can resume.
Adding a twenty-fifth must be configuration, not code.

This module is the contract. **Pure logic — no process spawning, no filesystem, no clock.**
Everything is passed in, which is what makes it testable.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ArmId(pub String);

/// How an arm is invoked. Declarative so a new backend is a config entry.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdapterSpec {
    pub arm: ArmId,
    /// Program to run, e.g. "codex".
    pub program: String,
    /// Argv template. The literal tokens `{prompt}` and `{worktree}` are substituted.
    pub args: Vec<String>,
    /// Whether the program runs in the working directory it is given. When false the
    /// worktree must be passed explicitly — one real backend writes into its own scratch
    /// directory otherwise, silently escaping its worktree.
    pub cwd_honored: bool,
    /// Environment to set, as key/value pairs.
    pub env: Vec<(String, String)>,
    /// Substrings that identify a usage-limit refusal in the arm's output.
    pub limit_markers: Vec<String>,
    /// Substrings that identify an auth failure.
    pub auth_markers: Vec<String>,
}

/// Why a run ended. The distinctions matter: only `Completed` says anything about the arm.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ExitKind {
    Completed,
    UsageLimit { marker: String },
    AuthFailure { marker: String },
    /// Terminated by a signal. `signal` is the raw signal number.
    Signalled { signal: i32 },
    Crashed { code: i32 },
}

impl AdapterSpec {
    /// Build the argv. `{prompt}` and `{worktree}` are replaced wherever they appear as a
    /// whole token. When `cwd_honored` is false, push `--add-dir <worktree>` as the last
    /// two arguments so the arm cannot escape its worktree.
    pub fn argv(&self, prompt: &str, worktree: &str) -> Vec<String>;

    /// Classify an exit. `status` is the raw wait status code as the shell reports it, so
    /// 128+N means signal N; 143 is SIGTERM and 137 is SIGKILL. `output` is the arm's
    /// combined stdout and stderr.
    ///
    /// Precedence, highest first: usage limit, auth failure, signal, non-zero code,
    /// otherwise completed. A limit marker in the output wins even when the exit code is 0,
    /// because several backends report a refusal and then exit cleanly.
    pub fn classify(&self, status: i32, output: &str) -> ExitKind;
}

/// Only `Completed` may update an arm's success statistics. Everything else is an
/// infrastructure outcome and must be excluded.
pub fn counts_for_posterior(kind: &ExitKind) -> bool;
```

## Required behaviour

- `argv` substitutes whole tokens only: an argument equal to `{prompt}` becomes the prompt;
  an argument containing `{prompt}` inside a longer string is left alone.
- `argv` with `cwd_honored == false` appends exactly `--add-dir` and the worktree, once, at
  the end, even if the template already mentions the worktree.
- `classify` checks markers against `output` case-insensitively.
- `classify` returns `Signalled` for any status in 129..=192, with `signal = status - 128`.
- `counts_for_posterior` is true only for `Completed`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule above, named after it. At minimum: whole-token substitution versus a token
embedded in a longer string; `--add-dir` appended exactly once; a limit marker beating exit
code 0; 143 classified as `Signalled { signal: 15 }`; and `counts_for_posterior` false for
every variant except `Completed`.

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
