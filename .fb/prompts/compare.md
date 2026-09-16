<!-- fb:creates crates/farmerbob-core/src/compare.rs -->
# Task: the comparison view a planning agent reads before picking a winner

Create `crates/farmerbob-core/src/compare.rs`, declare it from `lib.rs` with
`pub mod compare;`. Change nothing else except that one `pub mod` line.

## Context

This is the view an orchestrator actually looks at: all candidate runs for one task, side by
side, with the evidence that distinguishes them. Everything else in this crate exists to
fill in a row of it.

The design constraint is honesty about what is missing. Every column here has, at some point
in this project, been absent for some candidate — a cost never measured, a cross-examination
that voided, a conformance suite that did not exist. A view that renders an absent
measurement as a zero invites exactly the wrong conclusion, and the project has already
ranked an arm as free when its cost was simply unknown. So `None` must survive all the way
to the rendered cell, and sorting must never treat it as small.

A second constraint: the comparison must be able to say **"these are indistinguishable"**.
Most fields tie. An interface that always yields an ordering manufactures a winner from
noise.

**Pure logic: no I/O, no formatting to a terminal.** This produces the model behind a view.

## Exact API — implement these signatures verbatim

```rust
/// One candidate's row. Every metric is optional; `None` means not measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub arm: String,
    pub verdict: String,
    pub lines: Option<u32>,
    pub tests: Option<u32>,
    /// Clippy warnings introduced, as a DELTA against the base. May be negative.
    pub clippy_delta: Option<i32>,
    pub conformance: Option<f64>,
    /// Fraction of rivals' discriminating suites this implementation survives.
    pub survival: Option<f64>,
    pub usd: Option<f64>,
    pub secs: Option<u64>,
}

/// Which column an ordering is over. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column { Conformance, Survival, ClippyDelta, Tests, Lines, Usd, Secs }

impl Column {
    /// True when a LOWER value is better: ClippyDelta, Lines, Usd, Secs.
    pub fn lower_is_better(self) -> bool;
}

/// How two rows compare on one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp { Better, Worse, Tied, Incomparable }

/// Compare two rows on one column. `Incomparable` when EITHER value is absent --
/// an unmeasured cell is not a losing cell.
pub fn compare_on(a: &Row, b: &Row, col: Column, epsilon: f64) -> Cmp;

/// Rows ordered best-first on one column. Rows with an absent value are placed LAST,
/// whatever the direction, and keep their input order relative to one another.
pub fn rank(rows: &[Row], col: Column, epsilon: f64) -> Vec<&Row>;

/// Columns on which the field is not entirely tied or absent -- the ones worth showing.
/// Returned in the Column declaration order.
pub fn discriminating_columns(rows: &[Row], epsilon: f64) -> Vec<Column>;

/// A one-line-per-row table as (header, rows) of cells already stringified, with an
/// absent value rendered as the literal "n/a" and NEVER as "0".
pub fn render(rows: &[Row], cols: &[Column]) -> (Vec<String>, Vec<Vec<String>>);
```

## Required behaviour

- `compare_on` is `Incomparable` if either row's value for that column is `None`. This is
  checked before any comparison, so an absent value can never read as better or worse.
- Differences at or below `epsilon` are `Tied` for float columns. Integer columns compare
  exactly.
- `lower_is_better` is true for exactly `ClippyDelta`, `Lines`, `Usd`, `Secs` and false for
  the rest.
- `rank` puts absent values last for BOTH directions, and is a stable sort.
- `discriminating_columns` omits a column where every present value ties, and omits a column
  where fewer than two rows have a value — one measurement discriminates nothing.
- `render` emits `"n/a"` for every `None`. A negative `clippy_delta` renders with its sign.
- An empty `rows` yields an empty table and no discriminating columns, not an error.
- No NaN may escape; a NaN in any field is treated as absent rather than ordered.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: an absent value is Incomparable, not Worse; absent
sorts last ascending AND descending; a NaN is treated as absent; a column with one present
value is not discriminating; "n/a" never renders as "0"; a negative clippy delta keeps its
sign; ranking is stable among ties; an empty field is not an error.

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
