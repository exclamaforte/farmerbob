<!-- fb:creates crates/farmerbob-core/src/testout.rs -->
# Task: the evidence that distinguishes a strict suite from an over-fitted one is thrown away

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/testout.rs` and add `pub mod testout;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`witness::read` can tell one disagreement repeated across a field from several separate faults,
which is the difference the crossx `OVER-FITTED` label gets wrong. It was merged and it has no
input: it takes `Witness` values carrying the NAMES of failing tests, and the cross-examination
stage records only pass/fail per cell and discards the rest.

Twice in one week the label pointed the wrong way, both times because that detail was missing:

- `marker-grammar`: a suite was marked OVER-FITTED for failing two of three. The two failures
  were DIFFERENT confirmed defects -- a precedence violation in one arm, a silently-accepted
  tab in the other. Taking the label would have merged an implementation with a confirmed bug.
- `availability`: six failing cells across a 4x4, every one of them the same unpinned clause.
  An arm scored `survival=0.00` and its suite `OVER-FITTED` with no defect anywhere in it.

Same label, opposite truths, and the data that separates them was in the `cargo test` output
both times.

## Exact API

```rust
use crate::witness::Witness;

/// Names of the tests that failed in one `cargo test` run, sorted and
/// deduplicated.
///
/// Empty means the run had no failing tests, which is NOT the same as the
/// run having failed to build -- see [`Outcome`].
pub fn failed_tests(output: &str) -> Vec<String>;

/// What a `cargo test` run actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every test passed.
    Passed,
    /// The suite ran and these tests failed.
    Failed {
        /// Sorted, deduplicated names.
        failed: Vec<String>,
    },
    /// The suite did not build. No test names are available and none must be
    /// invented: an empty failure list here would read as "nothing failed".
    DidNotCompile,
    /// The output could not be classified. Distinct from every other variant:
    /// a reader must be able to tell "I could not parse this" from "it
    /// passed".
    Unreadable,
}

/// Classify one run's combined stdout+stderr.
pub fn classify(output: &str) -> Outcome;

/// Build the value `witness::read` consumes.
///
/// Returns `None` when the outcome carries no test names to report --
/// [`Outcome::DidNotCompile`] and [`Outcome::Unreadable`] -- because a
/// `Witness` with an empty `failed` list asserts that the suite passed, and
/// this module must never assert that on missing evidence.
pub fn witness_for(impl_arm: &str, output: &str) -> Option<Witness>;
```

## Falsifiable clauses

1. `failed_tests` reads the names from the `failures:` block that `cargo test` prints, which
   lists one indented test path per line after a line that is exactly `failures:`. Pin a
   realistic multi-failure block.
2. It ALSO reads the `---- <path> stdout ----` header lines, which appear once per failing
   test. A real run prints both; taking the union and deduplicating is what makes the result
   independent of which section a given cargo version emits. Pin a case with both present and
   agreeing, and a case with only the headers.
3. The returned names are sorted and deduplicated. `cargo test` prints the same name in both
   sections and repeats the `failures:` block; a name must appear once.
4. `classify` on output containing `test result: ok.` and no failures is `Passed`.
5. `classify` on output containing `error[E0433]:` or `could not compile` is `DidNotCompile`,
   **even if it also contains a `failures:` block** from an earlier crate in the same
   invocation. Say in the doc which wins and why: a suite that did not build produced no
   evidence about the implementation, and reporting stale names from another crate would
   attribute one crate's failures to another.
6. `classify` on empty or unrecognisable output is `Unreadable`, never `Passed`. Pin the empty
   string explicitly. This is the clause the whole module exists for: the harness has twice
   read "no failures found" off output it never parsed.
7. `witness_for` returns `Some` for `Passed` (with an empty `failed`) and for `Failed`, and
   `None` for `DidNotCompile` and `Unreadable`. Pin all four.
8. Test paths are taken VERBATIM, including module prefixes like
   `quota::tests::succeeded_resets_attempts` and the `xtests_0` prefix the graft adds. No
   stripping, no normalising. Two different implementations' suites are compared by name and a
   normalisation that collapses two distinct names is a false agreement.

## Boundaries, at N and at zero

- EMPTY output: `classify` is `Unreadable`, `failed_tests` is empty, `witness_for` is `None`.
- Output with `test result: FAILED` but no parseable names: `Failed { failed: vec![] }` is
  WRONG. State which you return and pin it -- an empty failure list from a failed run is the
  exact shape this module exists to refuse. `Unreadable` is the defensible answer.
- A single failing test: one name.
- A `failures:` block with zero entries beneath it: no names, and the outcome follows clause 6
  or clause 4 by what else the output contains. Pin it.
- Output from several crates in one invocation, one passing and one failing: the failing names
  are returned. Say what you do about which crate they came from -- the input is a single
  cell's output and the caller ran one crate, so this is about robustness, not attribution.

## Superset status on every enumerated list

`Outcome` is a CLOSED set of four. The MARKERS this parses -- `failures:`, `---- ... stdout
----`, `test result: ok.`, `test result: FAILED`, `could not compile`, `error[E....]:` -- are an
OPEN set: cargo's output format is not a stability guarantee, so an unrecognised shape must
fall to `Unreadable` and never to `Passed`. Say both halves and say which way the failure
falls.

## Composition of aggregate returns

`failed_tests` returns sorted, deduplicated names; `Outcome::Failed::failed` carries the same
list under the same contract. `witness_for` fills `Witness::failed` with it and
`Witness::impl_arm` with the argument verbatim.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O. The output arrives as a `&str`.
- **Use `witness::Witness`, imported.** Do not define your own.
- Do not change `witness.rs`.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the ONE file the task declares, and nothing else.** Not "only that crate" --
  that is what this line used to say, and it understated the rule by a wide margin. The
  measurement is `farmerbob_core::scope`, it compares paths as exact strings, and it permits
  exactly two things: the declared target, and adding `pub mod y;` to the `lib.rs` beside it
  when a NEW file needs that to compile. There is no tolerance band: ONE other changed file
  is a departure, and a departure now yields the verdict `OutOfScope`, which is not a pass.

  Read this as permission, not only as prohibition. If the task's own instructions make the
  wider workspace fail to build -- a new enum variant breaking a caller in another crate, say
  -- that breakage is EXPECTED and you must leave it. Reaching out to fix it is the departure.
  Say what you left broken in your handoff.

  This line was wrong until 2026-09-17, and three consecutive runs by one arm changed 51, 52
  and 51 files while being told "only the crate". They were measured against a rule they had
  not been given, which is the whole of farmerbob-vg0: disclose what is scored, or you are
  measuring house style rather than capability.

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
   it. Count is the fallback, not the target: ten tests that pin ten distinct behaviours beat
   forty that restate one. Your tests must be good enough to catch a bug in **any** correct-looking
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

Three tasks have now been decided by candidates disagreeing about exactly this -- an unpinned
boundary or an unstated list -- rather than about anything either of them got wrong, every time
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

## Run the tests quietly

Use `cargo test -q` and `cargo build -q`. This workspace has over nine hundred tests and a
plain `cargo test` prints a line for every one of them; three arms on a recent task spent
their whole run reading their own test output and produced nothing at all. `-q` prints the
summary and any failures, which is the entire signal.

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
