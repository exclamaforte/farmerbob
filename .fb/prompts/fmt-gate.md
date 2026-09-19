<!-- fb:creates crates/farmerbob-core/src/fmt_gate.rs -->
# Task: an agent must never be blamed for formatting the base was already missing

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/fmt_gate.rs`. Declare it with one `pub mod fmt_gate;`
line in `crates/farmerbob-core/src/lib.rs` and change nothing else.

## Why this exists

`cargo fmt` is ordinary Rust practice. In this harness it has been a disqualification.

An agent is given a worktree, changes its one declared file, and runs the formatter before
finishing. If the base was rustfmt-dirty, the formatter rewrites every dirty file the agent's
build touched, and the scope gate counts every one as a departure. codex-luna has now lost
four runs this way: three at 51, 52 and 51 departures, and score-delta this morning at 17. The
51 is not a coincidence — it was exactly the number of rustfmt-dirty files its changes
intersected.

`rustfmt.toml` has said "Keep this repo rustfmt-clean, permanently" since 2026-09-17. Two days
later it was dirty again in 19 files and 113 hunks, for the reason that comment itself names:
every merge takes ONE file from ONE arm in that arm's style, and nothing normalises it
afterwards. The workspace was renormalised in the commit before this task was written.

A comment is not a mechanism. The mechanism has two halves, and this task is the decision both
halves need:

1. **Before dispatch** — a base that is not formatted must not be handed to an agent, because
   any agent that then does the ordinary thing is punished for it.
2. **After scoring** — when a departure IS a file the base had left unformatted, the blame
   belongs to the base. That distinction does not exist today: `scope_departures` is one
   number and every departure in it weighs the same.

This module decides both. It runs no formatter and reads no files; a caller runs
`cargo fmt --check` and brings it the answer.

## Exact API

```rust
/// Whether a base may be handed to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatchable {
    /// Nothing on this base would be rewritten by the formatter. An agent may
    /// run it and touch only its own file.
    Yes,
    /// These paths would be rewritten by the formatter, in the order given.
    /// An agent that runs it departs its scope through no fault of its own.
    No {
        /// The unformatted paths, in the order the caller gave them.
        dirty: Vec<String>,
    },
}

/// Whether a base is safe to dispatch against.
///
/// `dirty` is the set of paths `cargo fmt --check` reports, as the caller
/// obtained it. This module does not run the formatter.
pub fn dispatchable(dirty: &[String]) -> Dispatchable;

/// Who a scope departure belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blame {
    /// The agent changed this file for its own reasons.
    Agent,
    /// The file was already unformatted on the base the agent was given.
    /// Running an ordinary formatter rewrites it, so the change is the
    /// base's doing, not the agent's.
    BaseWasUnformatted,
}

/// One departure, with its blame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributed {
    /// The departed path, exactly as the caller gave it.
    pub path: String,
    /// Who it belongs to.
    pub blame: Blame,
}

/// Split a run's scope departures between the agent and the base it was given.
///
/// `departures` are the paths the scope check flagged. `dirty_on_base` are the
/// paths the formatter would have rewritten on the base BEFORE the run.
pub fn attribute(departures: &[String], dirty_on_base: &[String]) -> Vec<Attributed>;

/// How many departures the agent is answerable for.
///
/// This is the number a verdict should key on. `Attributed::len()` is the
/// number to report, and they are different figures on purpose.
pub fn agent_departures(attributed: &[Attributed]) -> u32;
```

Paths are compared as exact strings, the same way `crate::scope` compares them. This module
does not normalise, canonicalise or resolve them; a caller that mixes absolute and relative
paths gets the answer its inputs deserve, and that is the caller's bug to fix.

## Falsifiable clauses

1. `dispatchable(&[])` is `Yes`.
2. `dispatchable(&["a.rs"])` is `No { dirty: vec!["a.rs"] }`. Clauses 1 and 2 pin that an empty
   dirty set and a non-empty one are different answers, which is the whole gate.
3. **The headline.** A departure that appears in `dirty_on_base` is `BaseWasUnformatted`. A
   departure that does not is `Agent`. Pin both in one test over one input containing both;
   pinning them separately does not show that the function distinguishes them.
4. `agent_departures` counts only `Blame::Agent`. A run whose every departure was
   `BaseWasUnformatted` has `agent_departures == 0` while `attribute(..).len()` is non-zero.
   Pin that the two figures differ on that input; a verdict keying on the wrong one is the
   defect this task exists to prevent.
5. A path in `dirty_on_base` that is NOT among `departures` contributes nothing. It is not an
   entry, not a count, and not an error: the agent simply did not touch it.
6. `attribute(&[], &["a.rs"])` is empty. No departures means nothing to attribute, whatever the
   base looked like.
7. `attribute(&["a.rs"], &[])` is one entry, `Agent`. An empty base-dirty set means every
   departure is the agent's — which is what a correctly formatted base should produce, and it
   must not be a special case in the code.
8. These functions are pure. Calling any of them twice with the same inputs gives the same
   answer, and none of them reads the filesystem, the environment or a clock.

## Boundaries, at N and at zero

- Both inputs empty: `attribute` is empty, `agent_departures` is 0.
- A departure listed twice: two entries, both attributed the same way. `attribute` returns one
  entry PER DEPARTURE, not per distinct path, and `agent_departures` counts entries. The scope
  check is not specified to deduplicate, so this module must not paper over it.
- `dirty_on_base` listed twice: no effect. Membership is membership; a path is dirty or it is
  not.
- The empty string as a path: treated as any other string. Decide nothing special, and say so
  in your handoff if you read this differently.
- A path that differs only by case, or by a `./` prefix: NOT the same path. Exact string
  comparison is pinned above; do not normalise.

## Superset status on every enumerated list

`Dispatchable` has exactly two variants. `Blame` has exactly two variants. Both lists are
closed; a third — "partially formatted", "unknown" — is a defect. Where the formatter could not
be RUN at all, that is the caller's `Measurement::Missing` to carry, and it must not be smuggled
into these enums. This module answers only what it was given.

## Composition of aggregate returns

`attribute` returns exactly one `Attributed` per entry in `departures`, in `departures` order,
each carrying that path unchanged. Nothing is filtered, merged, deduplicated or sorted. A caller
that wants only the agent's departures filters them; a caller reporting "17 departures, 17 of
them the base's" needs both figures, and clause 4 is the statement that it can have them.

`No { dirty }` carries `dirty` in the caller's order, unchanged and unsorted.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem, no process, no environment, no clock.
- Derive `Debug, Clone, PartialEq, Eq` on every type this task defines.
- **The base you are given is rustfmt-clean.** Run `cargo fmt` if you want it; it will touch
  your file and nothing else. This instruction is the opposite of what the rubric said this
  morning, and the reason is this very task: the base was dirty, and it is not any more.
- Create `crates/farmerbob-core/src/fmt_gate.rs` plus the one `pub mod` line, and nothing else.
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

  **`.fb/handoff.md` is the one exception, and it OVERRIDES the task's own Rules section.**
  Every spec's Rules says "change <target> and NOTHING else"; the Handoff section below then
  requires you to write `.fb/handoff.md`. Read literally those contradict, and a critic caught
  it: "one correct implementation must leave it untouched to obey the Rules, while another must
  write it to satisfy the Gate". Write the handoff. It is exempt, it has always been exempt,
  and the scope measurement already excludes it. Nothing else is.

  **`cargo fmt` is safe, and this line used to say the opposite.** The base you are given is
  rustfmt-clean -- `cargo fmt --all -- --check` exits 0 on it -- so running the formatter
  rewrites your file and nothing else. Run it if you want it.

  It was not always so, and the history is why this paragraph exists rather than a bare
  permission. The workspace drifted dirty because every merge takes ONE file from ONE arm in
  that arm's style and nothing normalised it afterwards. An arm that then ran a formatter had
  every dirty file its build touched rewritten, and the scope gate counted each one as a
  departure. One arm lost four runs that way, at 51, 52, 51 and 17 departures; the 51 was
  exactly the number of rustfmt-dirty files its changes intersected. The arms were doing
  ordinary Rust and the repository was wrong.

  So if `cargo fmt` DOES touch a file you did not change, the base has drifted again. Revert
  that file, keep your own, and say so in your handoff -- that sentence routes a harness bug
  back where it belongs instead of costing you the run.

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
