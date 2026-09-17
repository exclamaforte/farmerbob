<!-- fb:creates crates/farmerbob-core/src/pricing.rs -->
# Task: turn token counts into a defensible dollar figure, or admit you cannot

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/pricing.rs` and add `pub mod pricing;` to `lib.rs`.
Change nothing else.

## Context

This harness ranks coding agents on a curve of cost against capability. The cost axis is
currently the weakest number in the project: it is inferred from a launcher's price table,
and reconciled against the real provider invoice it was out by about 24% in mixed directions.
Two specific defects cause most of it.

An arm routed through a **metered but unpriced** route displays as `$0.0000`, which is
indistinguishable from a genuinely free route and fabricates a zero on the leaderboard. And a
**free route and a paid route to the same weights** are recorded as two arms, so one
capability is counted twice at two different prices.

This module computes the price and, where it cannot, says so instead of guessing.

It is pure: no I/O, no clock, no network. Everything arrives as arguments.

## Reuse the crate's existing type

Costs are reported as `crate::measurement::Measurement<f64>`, which already exists. **Do not
define your own optional-cost type.** Use `Measurement::observed`, `Measurement::not_attempted`,
`Measurement::instrument_failed`, `Measurement::nothing_to_measure` and
`Measurement::untrusted` as the variants below require. Read that module before starting.

## What to implement

```rust
pub struct Usage {
    pub input_tokens:  u64,
    pub output_tokens: u64,
}

/// How an arm is billed. These five and no others — the list is EXHAUSTIVE and you must
/// not add a variant.
pub enum Billing {
    /// Free route. A zero here is a real measured zero.
    Free,
    /// Billed per million tokens, both prices known.
    Metered { input_per_mtok: f64, output_per_mtok: f64 },
    /// Billed, but the price table has no entry. This is the case that must NOT read as free.
    MeteredUnpriced,
    /// Covered by a flat subscription, so per-run marginal cost is zero but total spend is
    /// not attributable to this run.
    Subscription,
    /// Billing is not known at all.
    Unknown,
}

pub struct Route {
    /// The route identifier as the launcher reports it, e.g. "openrouter/qwen/qwen3.8-flash:free".
    pub id: String,
    pub billing: Billing,
}

pub fn price(route: &Route, usage: &Usage) -> Measurement<f64>;
pub fn canonical_model(route_id: &str) -> String;
pub fn same_capability(a: &str, b: &str) -> bool;
pub fn total(prices: &[Measurement<f64>]) -> Priced;

pub struct Priced {
    /// Sum of the observed costs only.
    pub usd: f64,
    /// How many inputs were observed.
    pub observed: usize,
    /// How many were not, for any reason.
    pub missing: usize,
}
```

## Rules, each of which is testable

1. `Billing::Free` yields `Measurement::observed(0.0)` for any usage, including zero usage.
   A free route's zero is a real measurement.

2. `Billing::Metered` yields
   `observed(input_tokens / 1_000_000 * input_per_mtok + output_tokens / 1_000_000 * output_per_mtok)`.
   Compute in `f64`. Do not round, truncate, or clamp the result.

3. `Billing::MeteredUnpriced` yields
   `Measurement::instrument_failed("<reason>")` — never `observed(0.0)`. This is the whole
   point of the module: a billed run whose price is unknown must not be reported as free.

4. `Billing::Subscription` yields `Measurement::nothing_to_measure("<reason>")`. The marginal
   cost is genuinely zero but it is not this run's spend, and summing it as `0.0` would claim
   an attribution that was never made.

5. `Billing::Unknown` yields `Measurement::not_attempted()`.

6. A `Metered` route with a negative `input_per_mtok` or `output_per_mtok` yields
   `Measurement::untrusted("<reason>")`. A negative price is a corrupt table, not a discount.
   A price of exactly `0.0` is **not** negative and is priced normally, giving `observed(0.0)`.

7. A `Metered` route where either price is `NaN` or infinite yields
   `Measurement::untrusted("<reason>")`.

   **This rule is about the INPUT PRICES. Separately, if the COMPUTED cost is not finite --
   which `u64::MAX` tokens against a large price can produce by overflow -- the result is also
   `Measurement::untrusted("<reason>")`.** No non-finite value may ever reach `observed`, and
   an earlier version of this spec stated that invariant while scoping the only rule that
   enforced it to the inputs. Two critics flagged the gap; one wrote that the code "doesn't
   honor the literal sentence it wrote", which was true of the spec before it was true of any
   implementation.

8. Every reason string passed to `instrument_failed`, `nothing_to_measure` and `untrusted`
   **contains `route.id` verbatim.** The `missing` count in `Priced` is otherwise untraceable:
   a caller knows how many runs were unpriced and has no way to learn which, short of
   replaying `price` per run. A critic put it exactly: the id "was in hand at construction
   time; embedding it in the reason string costs nothing and is what would make the cost axis
   auditable -- this module's stated purpose".

## `canonical_model` and `same_capability` — pinned

`canonical_model` reduces a route id to the weights it reaches, so a free route and a paid
route to the same model compare equal:

- **Lowercase the whole id FIRST.** Every step below then matches against the lowercased
  string, so the markers are effectively case-insensitive.
- Strip a trailing `":free"` if present. That suffix and no other.
- Strip a leading `"or-"` or `"openrouter/"` if present, whichever matches first, and only one.
- Everything else is preserved exactly, including all remaining `/` separators.

The order matters and an earlier version of this spec had it backwards -- strip, strip,
lowercase -- which made the markers case-SENSITIVE. All four implementations followed that
order exactly, so it was a defect in the specification and not in any of them, and the
cross-examination matrix was in full consensus precisely because every arm shared it. Two
critics found it independently: `OpenRouter/Qwen/Qwen3.8-Flash:free` canonicalised to
`openrouter/qwen/qwen3.8-flash` while the paid route canonicalised to `qwen/qwen3.8-flash`,
so `same_capability` silently failed to deduplicate -- which is the exact bug this module
exists to prevent.

Worked examples, which are the contract:

| input                                      | output                    |
|--------------------------------------------|---------------------------|
| `openrouter/qwen/qwen3.8-flash:free`        | `qwen/qwen3.8-flash`      |
| `or-qwen38-flash`                           | `qwen38-flash`            |
| `qwen/QWEN3.8-Flash`                        | `qwen/qwen3.8-flash`      |
| `OpenRouter/Qwen/Qwen3.8-Flash:free`        | `qwen/qwen3.8-flash`      |
| `OR-Qwen38-Flash`                           | `qwen38-flash`            |
| `":free"`                                   | `""` (empty, not an error)|
| `""`                                        | `""`                      |

`same_capability(a, b)` is `canonical_model(a) == canonical_model(b)`. It is reflexive, and
`same_capability("", "")` is `true`.

## Composition of the returned aggregate — pinned

`total(prices)` sums **only** the `Observed` values. `observed` counts them; `missing` counts
every input that was not observed, regardless of which `Absent` reason it carries. The two
counts always sum to `prices.len()`.

`usd` for an empty slice is `0.0` and `observed` and `missing` are both `0`. A total of `0.0`
with `missing > 0` is a meaningful and expected state — it means nothing was priced — and
callers are expected to look at `missing`. `total` must never silently treat a missing value
as zero for the purpose of `observed`.

## Boundaries — pinned, including the degenerate cases

- Zero tokens on a `Metered` route: `observed(0.0)`. Real work with no tokens is real.
- `u64::MAX` tokens must not panic; `f64` conversion is expected and lossy precision is fine.
- An empty `prices` slice: see above.
- `canonical_model` on a string that is exactly `"or-"` returns `""`.
- A route id containing `":free"` other than at the end is left alone.

## Types

Use the concrete types above exactly. Do **not** make any parameter generic — no
`impl Into<String>`, no `impl AsRef<str>`. A previous task produced three implementations that
could not be compiled against one another because each invented its own generic bounds, and
the cross-examination matrix measured nothing at all as a result.

Derive `Debug`, `Clone` and `PartialEq` on every public type.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none. This module needs none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them
  yourself and fix any failures before finishing.
- Write a unit test per numbered rule above, named after it, plus one per worked example and
  per pinned boundary.

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
4. **Test depth and generality** — measured directly where possible, by injecting known
   defects and by running your suite against rival implementations. Where neither
   measurement could be taken, the number of distinct behaviours you covered stands in for
   it. Count is the fallback, not the target:
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

## A signature that cannot compute what the spec promises

If a clause in this spec describes a value that the API it also fixes makes **uncomputable**,
say so in your handoff and implement the closest honest thing. Do not silently return a
placeholder.

This is not hypothetical. A previous task's spec asked `status(resource)` to report "how long
the current holder has held the lease" while fixing a signature that takes no clock. Several
implementations returned `Duration::zero()` -- correct by necessity, indistinguishable from a
bug -- and the same spec's `holder_died(holder) -> Option<Grant>` could report only one grant
for a holder that may hold many, so its own "never left locked" invariant was unreportable.
Three critics found all of it, in three different implementations, which is how the fault was
traced to the spec rather than to any arm.

A defect that appears in nearly every implementation is evidence about the specification, not
about the field. Naming it in your handoff routes it where the fix belongs.

## Reuse the crate's existing types

If this spec names a type that already exists in `farmerbob-core` -- `Verdict`, `Grade`,
`RunState`, `Outcome`, `Measurement` and so on -- you **use that type**, imported, and you do
not define your own. A new public type whose name already exists in the crate is a defect,
scored as one, however good its internals are.

Forty-four modules have been merged, each written without sight of the others, and seven core
concepts now exist two or three times in mutually incompatible shapes. `Verdict` exists three
times. That happened one reasonable-looking local decision at a time. If you believe the
existing type genuinely cannot express what this task needs, say so in your handoff, name the
type and the clause it cannot express, and extend it rather than shadowing it.

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
