<!-- fb:creates crates/farmerbob-core/src/gate_kind.rs -->
# Task: the gate can only ask cargo, so a benchmark task cannot be scored at all

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/gate_kind.rs`. Declare it in `lib.rs` with one
`pub mod gate_kind;` line and change nothing else.

## Why this exists

Everything this harness executes is welded to cargo. Measured: `crossx.rs` has seventeen cargo
references, `testout.rs` eight, and `score.rs` IS the cargo gate — `cargo build -p X`,
`cargo test -p X`, count clippy. That is not a default with alternatives; it is the only path.

A benchmark task differs at every point. The deliverable is a CUDA kernel, success is
CORRECTNESS AGAINST A REFERENCE plus a SPEEDUP RATIO, and "tests passed" is not a number that
exists.

`farmerbob_core::task_contract` already holds the right shape and has ZERO callers:

```rust
pub enum Verification { DeterministicTests, Benchmark, ReviewerJudgement }
pub enum Score { Scored { speedup: f64, trials: u32 }, Incorrect { detail: String }, .. }
pub fn score(manifest, verify, samples, baseline_ms, max_spread) -> Score;
```

`Verification` already names all three kinds. Nothing branches on it. This module is that
branch: given a task's verification kind and what the harness observed, what is the verdict.
(bead farmerbob-p172)

## Exact API

```rust
/// What the harness observed for a task, in whichever form its kind produces.
pub enum Evidence {
    /// A cargo run: the build-and-test verdict `build_verdict` already decides.
    Tests(BuildVerdict),
    /// A benchmark run: the score `task_contract::score` already decides.
    Bench(Score),
    /// A reviewer's judgement. Carries whether it passed and a non-empty grounds.
    Review { passed: bool, grounds: String },
}

/// Whether a candidate cleared its task's gate.
pub enum Cleared {
    /// It cleared. Carries a one-line summary for an operator.
    Yes(String),
    /// It did not. Carries a one-line reason.
    No(String),
    /// The evidence does not match the task's declared verification kind, so no
    /// verdict is possible. Carries what was declared and what arrived.
    Mismatched { declared: Verification, got: String },
}

/// Decide whether the evidence clears the gate for this verification kind.
pub fn cleared(kind: Verification, e: &Evidence) -> Cleared;

/// The evidence variant a kind requires, as a name, for a caller's diagnostics.
pub fn expects(kind: Verification) -> &'static str;
```

`BuildVerdict` is `farmerbob_core::build_verdict::BuildVerdict`. `Score` and `Verification` are
`farmerbob_core::task_contract`'s. You define none of them, and you re-derive neither
`build_verdict::read` nor `task_contract::score` — both already decided before this is called.

## Falsifiable clauses

1. `DeterministicTests` with `Evidence::Tests(BuildVerdict::Passed { .. })` is `Yes`.
2. `DeterministicTests` with `Tests(BuildFailed)`, `Tests(Failed { .. })` or `Tests(NoTests)` is
   `No`, each with a non-empty reason. Pin all three — `NoTests` is a `No`, because the gate
   requires at least one test that actually executes.
3. `Benchmark` with `Evidence::Bench(Score::Scored { .. })` is `Yes`, and the summary mentions
   the speedup. Assert it CONTAINS the number; do not assert the wording.
4. `Benchmark` with `Bench(Score::Incorrect { .. })` is `No`, carrying a reason that mentions
   correctness. Case-insensitive `contains`; do not assert the sentence.
5. `ReviewerJudgement` with `Review { passed: true, .. }` is `Yes`; with `passed: false` it is
   `No` carrying the grounds.
6. **A kind given the WRONG evidence variant is `Mismatched`, never `No`.** Pin every wrong
   pairing: `DeterministicTests` with `Bench`, `DeterministicTests` with `Review`, `Benchmark`
   with `Tests`, and so on — six in all. This is the clause that carries the task. `No` means
   the candidate failed; `Mismatched` means the HARNESS handed the wrong instrument's output to
   the wrong gate, which is a fault in the harness and must not be recorded against an arm.
7. Clauses 2 and 6 must be pinned in ONE test that produces both from the same kind, because an
   implementation that returns `No` for everything it cannot clear passes clause 2 alone and
   silently blames arms for harness faults.
8. `expects` returns a distinct non-empty name per kind, and the three differ. Assert they
   differ; do not assert the strings.

## Boundaries, at N and at zero

- `Score::Scored { speedup: 0.0, trials: 1 }`: `Yes`. A measured zero speedup is a measurement,
  and whether a slow kernel PASSES is `task_contract::score`'s decision, not this module's — it
  already returned `Scored`. Pin that this does not second-guess it.
- `Score::Scored { trials: 0 }`: NOT pinned here. Say in your handoff what you returned and do
  not assert on it; `score` decides whether zero trials can be `Scored` at all.
- `Review { passed: true, grounds: "" }`: `Yes`. An empty grounds on a pass is not pinned as an
  error. Say what you did.
- `Review { passed: false, grounds: "" }`: the reason must still be NON-EMPTY, because a
  rejection with no stated reason is unactionable. Pin that it is non-empty.

## Superset status on every enumerated list

`Evidence` has EXACTLY three variants and `Cleared` exactly three. `Verification` is
`task_contract`'s and has exactly three — the mapping between them is one to one, and clause 6
is what enforces that a caller cannot cross the wires silently.

This module runs nothing. It does not invoke cargo, does not run a benchmark, does not read a
file. Every piece of evidence arrives already decided by the module that owns it.

## Composition of aggregate returns

`cleared` returns exactly one `Cleared`. `Mismatched` carries BOTH what was declared and what
arrived, so a caller can report the wiring fault precisely rather than saying "bad evidence".

`expects` and `cleared` must agree: for every kind, the variant `expects` names is the one that
does NOT produce `Mismatched`. Pin that agreement across all three kinds rather than asserting
the names.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem, no `std::process`.
- Derive `Debug, Clone, PartialEq` on every type in the Exact API. Not `Eq`: `Score` carries an
  `f64`.
- RUN `cargo clippy -p farmerbob-core --all-targets -- -D warnings` BEFORE you finish, including
  over your tests.
- `cargo test -p farmerbob-core` and that clippy command must pass. Both are clean on HEAD as of
  this task, so any failure is yours.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus `.fb/handoff.md`, which
  the Handoff section below REQUIRES you to write and which is exempt from this rule; plus,
  ONLY when the task
  CREATES a new file, the one `mod y;` line that declares it, in EITHER the `lib.rs` or the
  `main.rs` beside it. `farmerbob_core::scope::module_declarations_for` permits both, and a
  binary crate such as `fb` has no `lib.rs`, so `main.rs` is the only place the declaration can
  go. A task that MODIFIES an existing file touches one file and no other. Not "only that
  crate" --
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
4. **Test depth and generality** — and read this carefully, because it says something
   different from what it used to say.

   **Test COUNT is not a criterion and never was.** The adjudicating module refuses to rank on
   it, and it is right to: a table-driven test that pins nine behaviours scores one, and a
   suite of forty that restate one scores forty. This rubric claimed for weeks that count was
   scored fourth while the code scored it never, and every arm was told the wrong thing.
   (bead farmerbob-0ce)

   What IS scored is whether your tests would FAIL a wrong implementation. The instruments for
   that are defect injection and running your suite against a rival's code; when neither has
   been run, this criterion is `Missing` and ranks nobody. It is not silently replaced by a
   count.

   So write the tests that falsify the spec's clauses, however few that takes. Test the
   behaviour the specification requires, not your particular implementation's internals.
   Asserting on exact error strings, private field names, or an output format the spec does
   not fix makes a test worthless — and, when a rival's code is run against your suite, makes
   it worse than worthless, because it fails a correct implementation.
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

## A field the spec calls prose is not a field your tests may quote

If the specification describes a string by what it should SAY -- a reason, a grounds, a
diagnostic, a message "for a human reading an adjudication" -- then its exact wording is NOT
pinned, and a test asserting the exact text fails a rival that says the same thing differently.

This has cost a cross-examination cell in three consecutive tasks:

- a test asserting the exact text of `Inconclusive::reason`;
- a test asserting `"fewer than two implementations failed"` and `"no test names were recorded"`;
- a test asserting `reason.contains("fails against merged code")`.

In each case the spec pinned that the string was NON-EMPTY and said what it should convey, and
in each case a rival conveying it in other words was marked wrong.

Test the requirement, not the sentence. If the spec says the reason must say the test failed
against merged code, assert that it mentions failing and mentions merged -- separately,
case-insensitively -- or assert only that it is non-empty. One arm put it exactly right while
reviewing another: check "the semantic requirement ... without asserting on fragile, exact
string formatting that would fail against rival implementations".

The exception is a string the spec quotes verbatim as a value, such as a sentinel like
`"(no output)"`. A quoted literal is pinned; a described one is not.

## Your suite is run against OTHER implementations

This is the rule that has cost the most signal, four times, and it is stated here in terms of
what is MEASURED rather than of what is intended.

Every candidate's test suite is extracted and run against every other candidate's code. A suite
that calls anything the specification does not pin **fails to compile against every rival**, and
those cells are recorded as API-incompatible: you forfeit the cross-examination signal you would
otherwise have earned, however good your tests are.

Four arms have lost it this way, each for an addition that was reasonable on its own:

- a `Registry::new()` / `insert()` pair, used to build test fixtures;
- five tests asserting cases the spec never pinned;
- an inherent method beside the pinned free function, called five times in tests;
- a `DiffLine::added(..)` constructor, used to build test inputs.

None of those are bad code. Add them if they help a caller. **Your tests must go through the
pinned surface anyway** -- construct values from their public fields, call the free function the
Exact API names, and assert only on behaviour the specification fixes. If you would have to call
your own addition to write the test, write the test the longer way.

The rule is not "do not add API". It is "do not make your suite depend on API a rival has no
reason to have".

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
