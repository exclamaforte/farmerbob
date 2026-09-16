<!-- fb:creates crates/farmerbob-core/src/limit_signal.rs -->
<!-- fb:modifies crates/farmerbob-core/src/quota.rs -->
# Task: per-adapter usage-limit classification

Create `crates/farmerbob-core/src/limit_signal.rs`, declare it from `lib.rs` with
`pub mod limit_signal;`. Change nothing else except that one `pub mod` line.

## Context

farmerbob dispatches coding agents through several different CLIs and APIs. Each one signals
"you are out of quota" differently: an exit code, a line on stderr, a JSON error field, an
HTTP 429 surfaced mid-stream. `quota.rs` already models what to DO about a limit
(`LimitHit`, `QuotaTracker::park`, resumption). It does not model how to RECOGNISE one.

The distinction that matters: **a run killed by a quota cap is not a run the agent failed
at.** If those are conflated, the leaderboard charges a model for its provider's billing
policy. A misclassification in either direction is expensive — calling a real failure a
quota hit hides a bad model; calling a quota hit a failure defames a good one.

**Pure logic: no I/O, no network, no clock reads.** Time enters as a parameter.

## Exact API — implement these signatures verbatim

```rust
/// What the harness concluded about one finished run, from its exit status and output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Ran to completion. Not a limit.
    Normal,
    /// Provider refused on quota grounds. Carries the reset instant when one was stated,
    /// as a unix timestamp in seconds.
    Limited { reset_at: Option<u64>, evidence: String },
    /// Something matched a limit-shaped pattern but the rule set is not confident.
    /// The caller must not park a bucket on this alone.
    Ambiguous { evidence: String },
}

/// One provider's recognition rules. Data, not code: a new adapter is a new value,
/// never a new match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRules {
    /// Exit codes that mean quota, e.g. `vec![429]`.
    pub limit_exit_codes: Vec<i32>,
    /// Case-insensitive substrings of stderr/stdout that mean quota.
    pub limit_patterns: Vec<String>,
    /// Substrings that look like a limit but are NOT one, checked first.
    /// e.g. "rate limit" inside a model's own explanatory prose.
    pub exclusions: Vec<String>,
    /// Fallback window when a limit is detected but no reset time is stated.
    pub default_window_secs: u64,
}

impl SignalRules {
    pub fn new(default_window_secs: u64) -> Self;
    pub fn with_exit_code(self, code: i32) -> Self;
    pub fn with_pattern(self, pat: &str) -> Self;
    pub fn with_exclusion(self, pat: &str) -> Self;
}

/// Classify one finished run.
///
/// `now` is the current unix time in seconds, used only to resolve *relative* reset
/// statements ("try again in 3600 seconds") into absolute instants.
pub fn classify(rules: &SignalRules, exit_code: i32, output: &str, now: u64) -> Classification;

/// Extract an absolute reset instant from free text, if one is stated.
///
/// Recognises, at minimum:
///   - `retry-after: 3600`            (seconds from `now`)
///   - `"reset_at": 1789000000`       (absolute unix seconds)
///   - `resets at 2026-09-17T00:00:00Z` (RFC 3339)
pub fn parse_reset(output: &str, now: u64) -> Option<u64>;
```

## Required behaviour

- An exclusion match beats a pattern match: if any exclusion substring is present, the run is
  `Normal` even when a limit pattern also matched. Providers narrate rate limits in prose.
- Pattern and exclusion matching is case-insensitive.
- An exit code in `limit_exit_codes` is sufficient on its own, with no pattern present.
- A limit detected with no parseable reset time yields `reset_at: None` — do NOT silently
  substitute `default_window_secs` here. The caller decides the fallback; a guessed instant
  that looks measured is worse than an absent one.
- `evidence` contains the matched text, not a generic message. It is read by a human deciding
  whether the classifier is right.
- A nonzero exit code with no limit signal at all is `Normal`, not `Ambiguous`: ordinary
  failure is the common case and must not pollute the quota path.
- `Ambiguous` is returned when a limit pattern matches but the exit code is 0 — output said
  "quota" while the run succeeded, which no confident rule explains.
- `parse_reset` returns `None` rather than guessing on unparseable input, and never returns
  an instant in the past relative to `now` for a *relative* statement.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: exclusion beats pattern; case-insensitive match;
exit code alone suffices; limit with no reset yields `None`; nonzero exit with no signal is
`Normal`; pattern match with exit 0 is `Ambiguous`; all three `parse_reset` formats; garbage
input to `parse_reset` is `None`.

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
