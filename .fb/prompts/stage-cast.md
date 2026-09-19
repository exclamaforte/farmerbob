<!-- fb:creates crates/farmerbob-core/src/stage_cast.rs -->
# Task: a one-arm field still deserves every check that does not need a second implementation

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/stage_cast.rs`. Declare it with one `pub mod stage_cast;`
line in `crates/farmerbob-core/src/lib.rs` and change nothing else.

## Why this exists

The project now runs one arm per task. On a one-arm field the entire subjective tier goes dark:

```text
  crossx:   n/a, fewer than two candidates -- no matrix to build
  critique: n/a, fewer than two candidates -- nobody to cross-review
  promote:  n/a, no critiques were written -- nothing to promote
  prove:    n/a, nothing was promoted -- no claims to execute
```

Only the FIRST of those is true. `crossx` compares implementations against each other, and with
one implementation there is genuinely nothing to compare. `critique` does not compare anything —
it puts a model in front of a deliverable and asks what is wrong with it. That needs a second
**agent**, not a second **implementation**, and twenty arms are registered. The other two lines
are just that mistake cascading downstream.

`farmerbob_core::critique_plan` bakes the same confusion into a type: `plan()` assigns "arm `i`
reviews arm `i+1` round the ring", so critics are drawn from the candidate set and one candidate
yields `NoPlan::TooFew`. `crates/fb/src/critique.rs` re-implements it and pins the behaviour in a
test named `one_candidate_returns_not_applicable_and_writes_nothing`.

Two facts have one spelling, which is this project's recurring defect: **a stage that cannot run
is indistinguishable from a stage nobody staffed.**

Second half of this task, and equally the point: when a model spec-critiques, reviews or proves,
that work must be **credited to it by name**. The Pareto curve is meant to rank ability, and a
model that finds the defect nobody else saw has demonstrated ability whether or not it wrote a
line of the implementation.

## Exact API

```rust
/// A stage of the task pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Critique of the SPECIFICATION, before anyone implements.
    Speccheck,
    /// Objective measurement of each candidate.
    Score,
    /// Every candidate's test suite run against every other candidate's code.
    Crossx,
    /// A model reads one candidate's deliverable and reports findings.
    Critique,
    /// Findings become claims. Pure computation over critiques.
    Promote,
    /// A promoted claim becomes an executable discriminating test.
    Prove,
}

/// What a stage needs before it can run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Nothing but the specification. Runs before any candidate exists.
    SpecOnly,
    /// At least this many candidates. No second opinion required, because
    /// the stage runs no model.
    Candidates(u32),
    /// At least this many candidates AND one arm that did not author the
    /// work being examined. That arm comes from the ROSTER, not from the
    /// field: a second implementation is not required and never was.
    CandidatesAndIndependent(u32),
    /// At least this many candidates, which no roster can substitute for,
    /// because the stage compares implementations AGAINST EACH OTHER.
    ComparedCandidates(u32),
}

/// What the named stage needs. Exactly these six stages and no others.
pub fn needs(stage: Stage) -> Needs;

/// The seat an arm is cast in. This is the unit of CREDIT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    /// Reviewed the specification before anyone implemented.
    SpecCritic,
    /// Reviewed an implementation.
    Critic,
    /// Turned a promoted claim into an executable test.
    Prover,
}

/// One arm, cast in one seat, for one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Casting {
    /// The stage this casting is for.
    pub stage: Stage,
    /// The seat being filled.
    pub seat: Seat,
    /// The arm that does the work AND receives the credit. For an examining
    /// seat this is the examiner, never the examined.
    pub arm: String,
    /// Whose work is under examination. `None` when the seat examines the
    /// specification rather than an arm.
    pub subject: Option<String>,
}

/// Why a stage could not be cast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uncast {
    /// A comparing stage has too few candidates, and no roster can fix it.
    TooFewCandidates {
        /// How many the stage needs.
        needed: u32,
        /// How many the field has.
        have: u32,
    },
    /// Every arm available is also the author of the work to be examined, so
    /// no independent examiner exists. DISTINCT from `TooFewCandidates`:
    /// there is work to examine and nobody free to examine it.
    NoIndependentArm,
}

/// Cast the seats a stage needs for one field.
///
/// `candidates` are the arms that produced reviewable work, in a stable
/// order. `roster` is every arm available to be cast, in preference order.
pub fn cast(stage: Stage, candidates: &[String], roster: &[String])
    -> Result<Vec<Casting>, Uncast>;
```

`cast` returns `Ok(vec![])` for a stage that is applicable but casts nobody — `Score`, `Crossx`
and `Promote` run no model. An empty `Ok` and an `Err` are different answers and the whole task
turns on keeping them apart.

For an examining stage, the examiner is **the first entry in `roster` that is not the subject**.

## Falsifiable clauses

1. `needs(Stage::Crossx)` is `ComparedCandidates(2)`. `needs(Stage::Critique)` is
   `CandidatesAndIndependent(1)`. These two must be pinned together: they are the distinction
   this module exists to draw, and today both stages say "fewer than two candidates".
2. `needs(Stage::Speccheck)` is `SpecOnly`. `needs(Stage::Score)` and `needs(Stage::Promote)`
   are both `Candidates(1)`. `needs(Stage::Prove)` is `CandidatesAndIndependent(1)`.
3. **The headline.** `cast(Critique, &["a"], &["a", "b", "c"])` is
   `Ok(vec![Casting { stage: Critique, seat: Critic, arm: "b", subject: Some("a") }])`.
   One candidate produces one critique assignment. It must not be `Err`.
4. `cast(Crossx, &["a"], &["a", "b", "c"])` is `Err(TooFewCandidates { needed: 2, have: 1 })`.
   Clauses 3 and 4 take the SAME arguments but for the stage, and must be pinned as a pair:
   the roster rescues one and cannot rescue the other.
5. `cast(Critique, &["a"], &["a"])` is `Err(NoIndependentArm)` — there is work to examine and
   nobody free. Pin that this is NOT `TooFewCandidates`; conflating them is how a staffing
   problem gets reported as a structural one.
6. `arm` is the examiner and `subject` is the examined. Pin a case where they differ and assert
   on both fields. Reversing them is the credit defect, and it type-checks.
7. `cast(Critique, &["a", "b"], &["a", "b"])` gives `a`'s critic as `b` and `b`'s critic as `a` —
   the existing ring, falling out of the first-non-self rule rather than being coded separately.
8. `cast(Speccheck, &[], &["a", "b"])` is `Ok` with exactly one `Casting`, `seat: SpecCritic`,
   `arm: "a"`, `subject: None`. Speccheck runs with NO candidates; that is the point of it.
9. `cast(Score, &["a"], &["a"])` is `Ok(vec![])`, and so is `cast(Promote, &["a"], &[])`. A
   stage that runs no model needs no roster at all. Pin the empty roster case.
10. `cast(Crossx, &["a", "b"], &[])` is `Ok(vec![])` — applicable, casts nobody, and the empty
    roster is irrelevant to it.

## Boundaries, at N and at zero

- Zero candidates for a stage needing one: `cast(Critique, &[], &["a"])` is
  `Err(TooFewCandidates { needed: 1, have: 0 })`, not `NoIndependentArm`. At zero there is
  nothing to examine, so the staffing question never arises.
- Zero candidates for `Crossx`: `Err(TooFewCandidates { needed: 2, have: 0 })`.
- `cast(Speccheck, &[], &[])` is `Err(NoIndependentArm)`. There is a spec to review and nobody
  to review it; `TooFewCandidates` would be a lie, because speccheck never wanted a candidate.
- Exactly at the boundary: `cast(Crossx, &["a", "b"], ..)` is `Ok`, and one fewer is `Err`. Pin
  both sides of 2.
- A roster that repeats a name: the first non-subject entry still wins, and the same arm may be
  cast for more than one subject. Pin that a two-candidate field whose roster is `["c"]` casts
  `c` twice — once per subject. Reviewing two rivals is allowed; reviewing yourself is not.
- A candidate absent from the roster is still a valid subject. The roster says who may EXAMINE,
  not who may be examined. Pin it.

## Superset status on every enumerated list

`Stage` has exactly these six variants, `Needs` exactly these four, `Seat` exactly these three,
`Uncast` exactly these two. Each list is closed; a seventh stage or a third `Uncast` is a defect.

`Seat` has no `Author` variant on purpose. An author is credited by the score record, which
already names the arm; adding a seat for it would make one fact answerable two ways, which is the
duplication this crate has been paying for.

## Composition of aggregate returns

For an examining stage, `cast` returns **exactly one `Casting` per candidate, in `candidates`
order**, each with `subject: Some(that candidate)`. Not one per roster entry, not a deduplicated
set, and nothing filtered — a caller wanting a subset filters it, and a caller reporting coverage
needs the total.

For `Speccheck`, exactly one `Casting`, with `subject: None`.

For a stage that runs no model, exactly zero.

Every `Casting` in one call carries the same `stage` and the same `seat`. One call staffs one
stage; a mixed vector would mean this function decided something it was not asked.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. This module is pure: no filesystem, no process environment, no clock.
- Derive `Debug, Clone, PartialEq, Eq` on every type this task defines.
- Do NOT edit `critique_plan.rs`, `reviewer.rs` or `crates/fb/src/critique.rs`. Rewiring them is
  a separate task; this one must land first and alone. Say in your handoff what you would change
  in `critique_plan::plan` if you were allowed to.
- `Seat` is a new concept and does not collide with `reviewer::Role`, which is a reviewer's
  STANCE — Prosecutor, Defender, Cartographer. A seat is a pipeline position. Do not merge them
  and do not redefine `reviewer::Role`.
- Create `crates/farmerbob-core/src/stage_cast.rs` plus the one `pub mod` line, and nothing else.
- RUN `cargo clippy -p farmerbob-core --all-targets -- -D warnings` BEFORE you finish.

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

## A "derive these traits" instruction applies to types YOU define

If a spec says to derive `Debug, Clone, PartialEq, Eq` "on every type in the Exact API", it
means every type the task DEFINES. An Exact API block also NAMES types it imports --
`Measurement`, `Absent`, `Verdict`, `BuildVerdict` -- and you can neither define nor change
those. Read literally the two instructions contradict, and codex-luna reported exactly that
before implementing: "the derive rule requires the implementation to derive those traits on
`Measurement` and `Absent`, but the adjacent rule says those existing types must not be
defined".

Derive on what you write. If an imported type lacks a trait your tests need, say so in your
handoff and test around it; do not edit the other file.

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
