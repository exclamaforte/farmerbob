<!-- fb:creates crates/fb/src/cmd.rs -->
<!-- fb:modifies crates/fb/src/main.rs -->
# Task: the CLI command tree, exit codes, and machine-readable dispatch

Create `crates/fb/src/cmd.rs`, declare it from that crate's `main.rs` with
`mod cmd;`. Change nothing else except that one `mod` line.

## Context

farmerbob's primary user is another AI agent. The CLI is therefore an API, and the planning
agent scripts against it: it runs `fb ...`, branches on the exit code, and parses the JSON.
A silent change to either breaks those scripts with no error at all.

Two consequences shape this module:

- **Exit codes are the fast path.** An agent should be able to decide what to do next
  without parsing anything. "Would block" and "quota limited" in particular must be
  distinguishable from ordinary failure, because the correct response differs: retry later
  versus fix the request.
- **An unknown subcommand is a usage error, not a crash.** Agents mistype, and agents call
  a version of the CLI older than the docs they were trained on. Both must produce a
  structured, branchable refusal naming what was expected.

**Pure logic: no I/O, no process spawning, no argument parsing library.** This module maps
already-tokenised arguments to a decision. Keeping it pure is what makes the whole command
surface testable without running anything.

## Exact API — implement these signatures verbatim

```rust
/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    Dispatch { task: String, arms: Vec<String> },
    Score { task: String },
    Select { task: String, arm: String },
    Leaderboard,
    Help { topic: Option<String> },
    Version,
}

/// Global flags that may appear before or after the subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags { pub json: bool, pub quiet: bool, pub dry_run: bool }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation { pub command: Command, pub flags: Flags }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Names the subcommand given and the ones that exist.
    UnknownCommand { given: String, known: Vec<String> },
    /// A required positional is absent.
    MissingArgument { command: String, argument: String },
    /// A flag that is not recognised. Exactly the flags listed in Required behaviour are
    /// recognised and no others.
    UnknownFlag { given: String },
    /// No subcommand at all.
    NoCommand,
}

impl ParseError {
    /// Every parse error exits 2 (usage), never 1. An agent distinguishes "I called this
    /// wrong" from "the operation failed" without reading prose.
    pub fn exit_code(&self) -> i32;
}

/// Parse already-tokenised arguments, NOT including argv[0].
pub fn parse(args: &[String]) -> Result<Invocation, ParseError>;

/// Subcommand names, sorted. The single source of truth for help text and for the
/// `known` list in UnknownCommand.
pub fn known_commands() -> Vec<&'static str>;

/// The closest known command to a misspelling, by case-insensitive Levenshtein distance,
/// when that distance is at most 2. `None` otherwise -- a wrong guess is worse than none.
pub fn suggest(given: &str) -> Option<&'static str>;
```

## Required behaviour

- Recognised flags are exactly `--json`, `--quiet`, `--dry-run`, and no others. Anything
  else beginning with `-` is `UnknownFlag`.
- Flags may appear before or after the subcommand, and in any order.
- `--` ends flag parsing: every later token is positional even if it begins with `-`.
- Empty args is `NoCommand`, not `Help`. Silence is not a request for help.
- `dispatch` requires a task and at least one arm; a task with no arms is
  `MissingArgument { argument: "arm" }`.
- `help` takes an optional topic; `help nonexistent-topic` still parses, because reporting
  an unknown topic is the help command's job, not the parser's.
- `suggest("stauts")` is `Some("status")`; `suggest("xyzzy")` is `None`.
- Every `ParseError` exits 2.
- Parsing is total: no input, however malformed, may panic.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No new dependencies; do not reach for clap. This is deliberate: the point is a pure,
  testable mapping, and a parser library would put the behaviour under test in someone
  else's crate.
- `cargo build -p fb` and `cargo test -p fb` must pass.

## Tests you must write

One per rule, named after it. At minimum: flags before and after the subcommand give the
same Invocation; `--` makes a later `--json` positional; empty args is NoCommand; dispatch
with no arms names the missing argument; an unknown flag is distinguished from an unknown
command; every error exits 2; `suggest` finds a one-edit typo and refuses a distant one;
a fuzz-ish table of malformed inputs never panics.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p fb` succeeds
- `cargo test -p fb` passes, with at least one test that actually executes
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
