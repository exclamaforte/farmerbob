<!-- fb:creates crates/farmerbob-core/src/measurement.rs -->
# Task: a measurement that cannot silently claim it happened

Create `crates/farmerbob-core/src/measurement.rs`, declare it from `lib.rs` with
`pub mod measurement;`. Change nothing else except that one `pub mod` line.

## Context

One bug has occurred seven times in this project, in seven different instruments:

    fb-dispatch, fb-score, fb-verify, fb-conform   an empty test suite reads as "passed"
    fb-crossx                                      a suite that cannot compile reads as "no signal"
    fb-prove                                       a veto with no reference reads as "not vetoed"
    fb-escalate                                    a veto matching no tests reads as "passed"

Every one is the same rule: **a check that could not run reported the same value as a check
that ran and found nothing wrong.** Each was written by someone who knew about the previous
ones. Documentation did not stop it; only a type can.

So: a measurement is never a bare value. It is either a value that was actually observed, or
a stated reason it was not, and the second must not be coercible into the first by accident.
`unwrap_or(0)` and `unwrap_or_default()` are precisely how this keeps happening, so the API
must make the safe reading the short one and the dangerous reading long and explicit.

**Pure logic: no I/O.**

## Exact API — implement these signatures verbatim

```rust
/// Why a measurement is absent. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Absent {
    /// The instrument was never run.
    NotAttempted,
    /// The instrument ran but could not produce a value, e.g. the suite did not compile.
    InstrumentFailed { reason: String },
    /// There was nothing to measure, e.g. a veto with no reference implementation.
    NothingToMeasure { reason: String },
    /// A value was produced but is not trustworthy, e.g. a known-broken suite.
    Untrusted { reason: String },
}

/// An observation, or a stated reason there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Measurement<T> { Observed(T), Missing(Absent) }

impl<T> Measurement<T> {
    pub fn observed(v: T) -> Self;
    pub fn not_attempted() -> Self;
    pub fn instrument_failed(reason: &str) -> Self;
    pub fn nothing_to_measure(reason: &str) -> Self;
    pub fn untrusted(reason: &str) -> Self;

    pub fn is_observed(&self) -> bool;
    pub fn value(&self) -> Option<&T>;
    pub fn absent(&self) -> Option<&Absent>;

    /// Map over an observed value, preserving absence unchanged.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Measurement<U>;

    /// The ONLY way to get a bare value out. Named to be conspicuous at the call site and
    /// to require the caller to state, in prose, why a default is defensible here.
    pub fn or_default_because(self, default: T, justification: &str) -> T;
}

/// True when every measurement in the slice was observed.
pub fn all_observed<T>(ms: &[Measurement<T>]) -> bool;

/// The reasons, in slice order, for every measurement that is missing.
pub fn missing_reasons<T>(ms: &[Measurement<T>]) -> Vec<&Absent>;

/// Combine two measurements of the same quantity from independent instruments.
///
/// Two observations that DISAGREE are `Untrusted`: independent instruments contradicting
/// each other means at least one is broken, and picking either is guessing.
pub fn corroborate<T: PartialEq + Clone>(a: Measurement<T>, b: Measurement<T>) -> Measurement<T>;
```

## Required behaviour

- `or_default_because` is the only method yielding a bare `T`. There is deliberately no
  `unwrap`, no `unwrap_or`, and no `Default` shortcut: the seven recurrences all came
  through one of those.
- `or_default_because` ignores its `justification` at runtime. It exists to force the caller
  to write one, and its presence in a diff is the review signal.
- `map` preserves the exact `Absent` variant and its reason unchanged.
- `corroborate` returns: the value when both observed and equal; `Untrusted` naming the
  disagreement when both observed and unequal; the observed one when exactly one is
  observed; and when neither is observed, the FIRST argument's absence.
- `all_observed` is true for an empty slice — vacuous truth, with nothing missing.
- `missing_reasons` preserves slice order and omits observed entries.
- `Absent` reasons are never empty strings; a constructor given a blank reason stores
  `"unspecified"` rather than an empty string, so a log line always says something.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Do not implement `Default` for `Measurement`, and do not implement `From<Option<T>>`:
  both re-open the coercion this type exists to close.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: two disagreeing observations corroborate to
Untrusted; one observed and one absent yields the observed; neither observed yields the
first's absence; `map` preserves the reason text; a blank reason becomes "unspecified";
`all_observed` is true for an empty slice; `missing_reasons` keeps order.

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

**How to read a list in this spec.** Every enumerated list of keywords, formats or cases
states its own status, and you should read it literally:

- *"exactly these and no others"* — accepting anything further is a defect.
- *"at least these; accepting more is neither required nor penalised"* — a superset is fine,
  and **your tests may not assert on cases outside the list**, because another correct
  implementation may reasonably not handle them.
- *"at least these, plus the obvious morphological variants"* — the stemming rule is pinned
  where it says so.

If a list carries no such marker, treat it as the second form, and say so in your handoff.

**And to the composition of anything aggregate you return.** If a function returns a
table, a tuple, or a collection whose membership is not forced by its type, the spec states
exactly what is in it and in what order. Where it does not, say so in your handoff and do
not let your tests assert on it.

**The same applies to every numeric boundary.** Where a clause says "after N has elapsed",
"at least N", or "below N", the behaviour AT N and at the degenerate value (N = 0, an empty
collection, a timestamp that runs backwards) is part of the contract. If the spec does not
pin it, your tests may not assert on it either -- another correct implementation may
reasonably choose the other side. Say in your handoff which boundary you found unpinned and
which way you resolved it.

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
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
