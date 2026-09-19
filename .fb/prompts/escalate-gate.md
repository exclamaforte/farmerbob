<!-- fb:creates crates/farmerbob-core/src/escalate_gate.rs -->
# Task: an escalated test that fails against the merged reference encodes a misreading

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/escalate_gate.rs`. Declare it in `lib.rs` with one
`pub mod escalate_gate;` line and change nothing else.

## Why this exists

`fb-escalate.sh` takes a CONFIRMED finding and promotes it into the task's permanent
conformance suite, so each round sharpens the gate that judges every future candidate. Two
disciplines decide whether a finding is allowed through, and both are in shell:

**The reference veto.** An escalated test must PASS against the merged reference. A test that
fails there is encoding the critic's misreading of the specification rather than a defect in
any candidate. This is not hypothetical: a critic "confirmed" a test asserting that
`sensitivity()` should be `None` where the spec makes it `Some(0.0)`. It failed the losing
candidate, which looked like confirmation, and it would have failed every correct
implementation forever.

**Provenance.** Every escalated test records the critic that found it, the candidate it was
found on, and the task. A bad test must be traceable and retirable; the arm that contributed a
good one must be creditable.

The decision is pure. The file I/O around it is not, and is not yours.

## Exact API

```rust
/// A finding that has been confirmed against the candidate it was made about.
pub struct Finding {
    /// The arm that made the claim. Non-empty is not enforced by the type.
    pub critic: String,
    /// The arm the claim was made about.
    pub subject: String,
    /// The task it was found on.
    pub task: String,
}

/// What happened when the finding's test was run against the merged reference.
pub enum Reference {
    /// It ran and PASSED. The test does not indict correct code.
    Passed,
    /// It ran and FAILED. The test indicts the reference, so it encodes a
    /// misreading rather than a defect. Carries the reference's failure output.
    Failed(String),
    /// It could not be run. Carries a non-empty reason.
    NotRun(String),
}

/// Whether a finding may join the permanent suite.
pub enum Admit {
    /// Escalate it, with this provenance.
    Escalate(Provenance),
    /// Do not escalate: the reference fails it. Carries the reference output.
    Vetoed(String),
    /// Do not escalate: the reference was not run, so the veto never happened.
    /// An unrun veto is NOT a passed veto. Carries the reason.
    Unverified(String),
    /// Do not escalate: this finding is already in the suite.
    AlreadyPresent,
}

/// What an escalated test records about where it came from.
pub struct Provenance {
    pub critic: String,
    pub subject: String,
    pub task: String,
}

/// Decide whether one confirmed finding joins the suite.
///
/// `present` is the provenance already recorded in the suite, in any order.
pub fn admit(f: &Finding, r: &Reference, present: &[Provenance]) -> Admit;

/// The provenance a set of admitted findings adds to the suite.
///
/// Escalation is IDEMPOTENT: running it twice over the same findings adds
/// nothing the second time.
pub fn escalate(
    findings: &[(Finding, Reference)],
    present: &[Provenance],
) -> (Vec<Provenance>, Vec<(usize, Admit)>);

/// Credit, per critic, for findings that were escalated.
///
/// Keys are critic names; the value is how many of that critic's findings
/// were escalated. A critic with none does not appear.
pub fn credit(escalated: &[Provenance]) -> BTreeMap<String, usize>;
```

`BTreeMap` is `std::collections::BTreeMap`.

## Falsifiable clauses

1. `Reference::Passed` and a finding not already present: `Admit::Escalate`, whose `Provenance`
   carries the finding's critic, subject and task unchanged.
2. `Reference::Failed(out)`: `Admit::Vetoed(out)`, carrying the output unchanged. This is the
   `sensitivity()` case and it is the reason the module exists.
3. `Reference::NotRun(why)`: `Admit::Unverified(why)`, NOT `Escalate`. An unrun veto is not a
   passed veto.
4. Clauses 1 and 3 must be pinned in ONE test that builds both from the same finding and
   asserts they differ. Separately, each passes against an implementation that escalates
   whenever the reference did not explicitly fail -- which is the permissive reading this
   module exists to forbid.
5. A finding whose exact `(critic, subject, task)` triple is already in `present`:
   `Admit::AlreadyPresent`, whatever the reference says. Pin it with `Reference::Passed`,
   because that is the case where a permissive implementation would escalate a duplicate.
6. Matching is on all THREE fields together. The same critic and task with a different subject
   is a DIFFERENT finding and is admitted. Pin one such case per field, three in all.
7. `escalate` returns the provenance of exactly the findings that were `Escalate`, in input
   order, and a `(index, Admit)` entry for every finding that was not.
8. Escalation is idempotent: feed `escalate`'s own output back as `present` with the same
   findings, and the returned provenance vector is empty.
9. Within one call, a finding duplicated in the INPUT is escalated once. The second occurrence
   is `AlreadyPresent`. An implementation that only consults the incoming `present` escalates
   it twice, and the suite then carries the same test twice.
10. `credit` counts each provenance under its critic. A critic with two escalated findings maps
    to 2; a critic with none is absent from the map, not present with 0.

## Boundaries, at N and at zero

- ZERO findings: `escalate` returns two empty vectors, and `credit(&[])` is an empty map.
- ZERO findings present, one finding admitted: escalated. The empty suite is not a reason to
  refuse.
- ONE finding, vetoed: the escalated vector is empty and the rejection vector has exactly one
  entry, with index 0.
- ALL findings vetoed: escalated is empty. Assert it is EMPTY, not merely short -- an
  implementation that escalates a vetoed finding passes clause 2 and fails only here.
- An empty `critic`, `subject` or `task` string: NOT pinned. The type does not enforce
  non-empty. Say in your handoff what you did and do not assert on it.
- An empty reason in `NotRun("")` or output in `Failed("")`: not pinned. Same instruction.

## Superset status on every enumerated list

`Reference` has EXACTLY three variants and `Admit` exactly four. Closed sets.

The RULE that a confirmed finding is the input here is the caller's: this module does not
decide whether a claim was confirmed, and re-deriving that is a defect.
`farmerbob_core::prove_veto::Fate` is what decides it, it is already merged, and if you believe
`Reference` duplicates it, say so in your handoff and name the clause rather than importing it.

**Where this spec's Exact API and an existing type conflict, the EXACT API WINS** — write the
pinned signature and name the conflict in your handoff. This rule is here because the
`stage-key` spec pinned a closed enum and, in another section, told the arm to reuse a
five-variant one; both could not be obeyed, and the arm that noticed said so instead of
guessing. (bead farmerbob-udp5)

## Composition of aggregate returns

`escalate` returns a PAIR whose parts partition the input: every finding is either in the
escalated vector or has an entry in the rejection vector, never both and never neither. Pin
that partition on a mixed input containing one of each `Admit` variant — lengths summing to the
input length.

The rejection vector carries INDEXES into the input, so a caller can report which finding was
refused. Order follows the input.

`credit`'s map is keyed by critic with no ordering promise beyond `BTreeMap`'s own. Its values
sum to the escalated vector's length; pin that sum.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem, no `std::process`: the reference outcome arrives as a
  value.
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
