<!-- fb:creates crates/farmerbob-core/src/envelope.rs -->
# Task: implement the JSON output envelope

Create `crates/farmerbob-core/src/envelope.rs`, declare it from `lib.rs` with
`pub mod envelope;`. Change nothing else.

## Context

farmerbob's primary user is another AI agent. Its machine-readable output is an API: a
planning agent scripts against it, and a silent schema change breaks those scripts with no
error. So the envelope is versioned, the error shape mirrors the success shape, and exit codes
are branchable without parsing text.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// Branchable outcome. The planning agent switches on this without reading prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExitCode { Ok, Error, Usage, WouldBlock, QuotaLimited }

impl ExitCode {
    /// 0 ok, 1 error, 2 usage, 3 would-block, 4 quota-limited.
    pub fn code(self) -> i32;
    pub fn from_code(code: i32) -> Option<ExitCode>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ErrorBody { pub code: String, pub message: String }

/// Every command emits exactly this shape under --json.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Envelope {
    Ok { schema: String, version: u32, data: serde_json::Value },
    Err { schema: String, version: u32, error: ErrorBody },
}

impl Envelope {
    pub fn ok(schema: &str, version: u32, data: serde_json::Value) -> Envelope;
    pub fn err(schema: &str, version: u32, code: &str, message: &str) -> Envelope;
    /// Serialise to exactly ONE line: no interior newline, terminated by `\n`.
    /// A multi-line record breaks any consumer reading line by line.
    pub fn to_line(&self) -> Result<String, String>;
    pub fn exit_code(&self) -> ExitCode;
    /// The schema name, whichever variant this is.
    pub fn schema(&self) -> &str;
}

/// Parse a line back. Unknown fields must NOT be an error: a consumer built against version 1
/// has to keep working when version 2 adds a field.
pub fn parse_line(line: &str) -> Result<Envelope, String>;
```

## Required behaviour

- `to_line` never emits an interior newline, even when a string field contains `\n`.
- An error envelope's `exit_code` is `Error` unless its `code` is `"would_block"` or
  `"quota_limited"`, which map to those variants.
- `parse_line` tolerates unknown fields and rejects a line that is not an envelope at all.
- `from_code` returns `None` for an unmapped integer rather than guessing.
- Round-tripping an envelope through `to_line` and `parse_line` yields an equal value.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a data field containing a literal newline still
serialises to one line; round-trip equality for both variants; an envelope with an extra
unknown field still parses; `from_code(9)` is `None`; `would_block` maps to exit code 3.

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
