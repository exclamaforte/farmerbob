<!-- fb:creates crates/farmerbob-core/src/promote.rs -->
# Task: auto-promote objective claims out of critiques

Create `crates/farmerbob-core/src/promote.rs`, declare it from `lib.rs` with
`pub mod promote;`. Change nothing else except that one `pub mod` line.

## Context

farmerbob splits work by who can be trusted with what. **Objective metrics are the harness's
job; subjective critique is the model's.** A critic reviewing a rival implementation may say
"this allocates on every call, which will hurt under load" — that is a judgement, and useful.
It may not say "this fails on empty input", because that is a *claim*, and a claim is either
true or false and the harness can find out for free.

So claims do not get to stay claims. A critique is parsed, every falsifiable assertion is
extracted, and each one becomes an executed test. The critic's opinion of a rival never
reaches the score; only the test result does. This removes the incentive to write a
persuasive critique, because persuasion does not survive execution.

A CLAIM is falsifiable only when it states an input, an expectation, and an observation that
DIFFER. "It returns the wrong value for -1" names no expectation and cannot be promoted.

**Pure logic: no I/O, no test execution.** This module decides what *would* be run.

## Exact API — implement these signatures verbatim

```rust
/// A line a critic wrote about a rival implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assertion {
    /// Falsifiable: names a target, an input, what the critic expected, what they say happens.
    Claim { target: String, input: String, expect: String, actual: String },
    /// Opinion. Recorded, never scored, never executed.
    Judgement { target: String, text: String },
}

/// Why an assertion could not become a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// `expect` and `actual` are the same: nothing is being alleged.
    NotFalsifiable,
    /// A required field is empty.
    Incomplete { field: String },
    /// A judgement was submitted where a claim was required.
    NotAClaim,
    /// The same claim, already promoted.
    Duplicate,
}

/// A claim that earned execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedTest {
    pub target: String,
    pub input: String,
    pub expect: String,
    /// Stable identity: equal for two claims alleging the same thing, so the same
    /// assertion made by two critics is executed once.
    pub fingerprint: String,
}

/// Promote what can be executed; reject the rest with a reason.
///
/// Order is preserved, and a rejection never aborts the batch: one unfalsifiable line
/// must not discard the critique it arrived in.
pub fn promote(assertions: &[Assertion]) -> (Vec<PromotedTest>, Vec<(usize, Rejection)>);

/// Normalised identity of a claim, ignoring surrounding whitespace and letter case.
pub fn fingerprint(target: &str, input: &str, expect: &str) -> String;

/// Does this critique contain an objective assertion where only judgement was allowed?
///
/// The handoff and the critique are both forbidden from stating anything the harness can
/// check. Returns the indices of the offending entries.
pub fn objective_leaks(assertions: &[Assertion]) -> Vec<usize>;
```

## Required behaviour

- A `Claim` whose `expect` equals `actual` after normalisation is `NotFalsifiable`. This is
  the single most common way a critique smuggles an opinion through a claim-shaped form.
- Any empty or whitespace-only required field yields `Incomplete` naming that field.
- `fingerprint` ignores case and leading/trailing whitespace, and is independent of `actual` —
  two critics alleging the same failure with different wording of what they observed produce
  one test.
- Duplicates are rejected as `Duplicate`, keeping the FIRST occurrence.
- `Judgement` entries are never promoted and never rejected as malformed; they are simply
  not tests. Passing one to `promote` yields `NotAClaim`.
- `objective_leaks` flags a `Judgement` whose text asserts a checkable fact. Treat text
  containing any of `passes`, `fails`, `panics`, `compiles`, `returns`, or a bare integer
  followed by `tests` as objective.
- Rejections carry the index of the assertion in the input slice, so a critic can be told
  exactly which line was refused.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: expect == actual is NotFalsifiable; case and
whitespace differences still count as equal; an empty input field names that field; two
critics alleging the same thing yield one test; the first duplicate is kept; a Judgement is
NotAClaim; `objective_leaks` catches "all tests pass" and ignores "this seems fragile";
one bad line does not discard the batch.

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
