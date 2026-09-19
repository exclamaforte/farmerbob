<!-- fb:creates crates/farmerbob-core/src/stage_key.rs -->
# Task: the pipeline decides four things per stage and none of them are tested

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/stage_key.rs`. Declare it in `lib.rs` with one
`pub mod stage_key;` line and change nothing else.

## Why this exists

`fb-pipeline.sh` runs five stages over a task. Before each one it decides, in shell:

- **Is it already done?** Compare a stored signature against a freshly computed one. Equal means
  skip.
- **What is the signature keyed on?** Two answers. `score` and `differential` are keyed on the
  candidates ON DISK (`wtfield`); `crossx`, `critique`, `promote` and `prove` are keyed on who
  PASSED (`field`). Keying `differential` on the passing set left it stale whenever a failing
  candidate changed -- the first real run measured 5 arms under a 3-arm fingerprint, because the
  passing set is a strict subset of what the artefact covers.
- **Did it actually produce anything?** Exit 0 with a missing or empty artefact is a failure,
  not a pass.
- **Does the whole pipeline pass?** It did not track this at all: every caller discarded
  `run_stage`'s return, the script's last command was an `echo`, so it always exited 0. The
  autopilot's `if ! ./fb-pipeline.sh ...; then touch .skip; fi` guard could therefore never
  fire. Zero skip markers were ever written and one task was re-run 228 times.

That is four rules, all pure, none tested, one of which was wrong for the life of the project.
This task moves the decision. The shell keeps running the commands; that is not yours.

## Exact API

```rust
/// What a stage's signature is computed over.
pub enum Keyed {
    /// Every candidate that has a worktree, whether or not it passed the gate.
    OnDisk,
    /// Only the candidates that passed the gate.
    Passing,
}

/// One stage of the pipeline.
pub struct Stage {
    pub name: String,
    pub keyed: Keyed,
}

/// The state of a stage's artefact before the stage runs.
pub struct Artefact {
    /// True when the artefact file exists and is non-empty.
    pub present: bool,
    /// The signature stored beside the artefact, if any.
    pub stored_signature: Option<String>,
}

/// What the pipeline should do with a stage.
pub enum Action {
    /// Signature matches and the artefact is present.
    Skip,
    /// Run it, and afterwards store this signature.
    Run { store: String },
}

/// Decide whether a stage runs.
///
/// `on_disk` and `passing` are the two signatures the caller computed; this function
/// selects between them per `stage.keyed` and never computes either.
pub fn action(stage: &Stage, a: &Artefact, on_disk: &str, passing: &str) -> Action;

/// What a stage's run produced.
pub enum Produced {
    /// Exit zero and a non-empty artefact.
    Ok,
    /// Non-zero exit. Carries the code.
    Exit(i32),
    /// Exit zero and a missing or empty artefact. The failure that looks like success.
    Nothing,
}

/// Classify one completed stage run.
pub fn produced(rc: i32, artefact_present: bool) -> Produced;

/// The pipeline's own verdict over every stage it ran.
pub enum Pipeline {
    /// Every stage produced something.
    Ready,
    /// At least one did not. Carries the failing stage names, in the order they ran.
    NotReady(Vec<String>),
}

/// Compose the whole pipeline's verdict.
pub fn pipeline(results: &[(String, Produced)]) -> Pipeline;
```

## Falsifiable clauses

1. `action` with `Keyed::OnDisk` compares `stored_signature` against `on_disk` and IGNORES
   `passing`. Pin this with a case where the two signatures DIFFER and only the on-disk one
   matches: the result is `Skip`. An implementation that consults the wrong one gives `Run`.
2. `action` with `Keyed::Passing` compares against `passing` and ignores `on_disk`. Same shape,
   mirrored. Clauses 1 and 2 must be tested with DIFFERENT signature values; equal values make
   both pass against an implementation that always reads the same one.
3. `Skip` requires BOTH a matching signature and `present: true`. A matching signature with
   `present: false` is `Run` -- the artefact was deleted and the signature outlived it.
4. `stored_signature: None` is always `Run`, whatever the artefact's presence.
5. `Run { store }` carries the signature the stage is KEYED on, so that storing it makes the
   next call `Skip`. Pin the round trip: take `Run{store}`, feed it back as
   `stored_signature: Some(store)` with `present: true`, and assert `Skip`.
6. `produced(0, true)` is `Ok`.
7. `produced(0, false)` is `Nothing`, NOT `Ok`. This is the "ran but produced nothing" case.
8. `produced(rc, _)` for any non-zero `rc` is `Exit(rc)`, carrying the code, whether or not an
   artefact is present. A stage that failed and left a stale artefact behind is still a failure.
9. `Nothing` and `Ok` are different values, and `Nothing` makes the pipeline `NotReady`. Pin
   that a pipeline of one stage that exited 0 with no artefact is `NotReady`, because the
   original defect was exactly this composing to success.
10. `pipeline` returns `NotReady` with the names of every non-`Ok` stage, in input order, and
    `Ready` only when all are `Ok`.

## Boundaries, at N and at zero

- ZERO stages: `pipeline(&[])`. Not pinned. Say in your handoff which you returned and why, and
  do NOT assert on it -- a rival may reasonably call an empty pipeline `Ready` or
  `NotReady(vec![])`, and this boundary has already cost three tasks.
- ONE stage, `Ok`: `Ready`.
- ONE stage, `Exit(1)`: `NotReady` naming exactly that one stage.
- ALL stages failing: `NotReady` listing all of them, in order, none dropped.
- An empty stage name: not pinned. Do not assert on it.
- `produced(0, true)` where the artefact is present but was written by an EARLIER run: this
  module cannot tell, and clause 5's signature round trip is the only defence. Say so in your
  handoff.

## Superset status on every enumerated list

`Keyed` has EXACTLY two variants, `Action` exactly two, `Produced` exactly three, `Pipeline`
exactly two. Closed sets.

The five stage NAMES the pipeline runs -- `score`, `differential`, `crossx`, `critique`,
`promote`, `prove` -- are the CALLER's, passed in as strings. This module does not enumerate
them, does not validate them, and a `const` list of them here is a defect: a sixth stage must
not require editing this file.

`StageOutcome` already exists in `farmerbob_core::stage_outcome` and classifies ONE command's
exit against ONE artefact for the `fb stage` subcommand. If it already expresses `Produced`,
USE IT and say so in your handoff rather than defining a second copy -- `Verdict` exists three
times in this crate because forty-four modules were each written without sight of the others.
If it genuinely cannot express clause 8, name the clause in your handoff and define `Produced`.

## Composition of aggregate returns

`Pipeline::NotReady` carries names in the order the stages were given, not sorted and not
deduplicated. Pin the order with at least three stages of which the first and last fail.

`action` returns the signature to STORE, never the one that was stored: clause 5 is the
statement that these compose.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem access: `Artefact.present` is given to you, you do not stat
  anything.
- `cargo test -p farmerbob-core` and `cargo clippy -p farmerbob-core -- -D warnings` must pass.
  Both are clean on HEAD as of this task, so any failure is yours.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus, ONLY when the task
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
