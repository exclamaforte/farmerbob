<!-- fb:modifies crates/fb/src/main.rs -->
# Task: five merged command modules that nothing can call

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/main.rs`. Do not change any other file.

## Why this exists

Eight modules in this crate compile, run their tests, and are reachable from nothing. Five of
them are finished command layers waiting only for an argument-parser arm:

- `compare_cmd` / `compare_gather` — render a task's candidates as a table, built from its
  `score.json`.
- `timing_cmd` — render time-to-first-write, span and rate per arm.
- `prices_cmd` — render pending price changes, keeping "correct a figure" apart from "fill a
  blank".
- `sem_cmd` — render the slot table and the grant decision.

All five carry `#![allow(dead_code)]`, which every one of their specs told the arm to add so an
uncalled `pub fn` would not trip `-D warnings`. That instruction silenced the compiler's own
unreachability check, so nothing reports these: build is silent, clippy is silent, and the
orphan check looks for a `mod` line, which they all have. Declared is not reachable.
(bead farmerbob-649f)

## Exact API

`main.rs` gains four subcommands:

```rust
/// Compare a task's candidates: tests, lines added, clippy.
Compare {
    /// Task name; its score file is <logs>/<task>.score.json.
    task: String,
},
/// How each arm spent its run: time to first write, span, rate.
Timing {
    task: String,
},
/// Pending price changes between sources.toml and the provider catalogue.
Prices,
/// The slot table and whether another run may start.
Sem,
```

Each arm calls its module's `run`, writing to `std::io::stdout()`, and exits with the returned
code via `std::process::exit`.

`Compare` goes through `compare_gather::run`, which reads the score file and calls
`compare_cmd::run`. You do not add a second path.

`Timing`, `Prices` and `Sem` take their inputs from the existing sources this binary already
reads; if a module's `run` needs a value `main.rs` cannot supply, **say so in your handoff,
name the module and the value, and wire the ones you can.** Wiring four of four is better than
wiring none, and a module that cannot be reached from the CLI as it stands is a finding worth
reporting rather than a reason to stop.

## Falsifiable clauses

1. `fb compare <task>` on a task with a score file prints a table and exits with
   `compare_gather::run`'s code, unchanged.
2. `fb compare <task>` on a task with NO score file exits with that function's unreadable code,
   not 0. Pin that a missing file and a present one give different codes.
3. `fb timing <task>`, `fb prices` and `fb sem` each invoke their module and exit with its code.
   For any you could not wire, clause 3 does not apply — say which in your handoff.
4. No subcommand's behaviour changes except by addition. Pin one existing subcommand's output
   before and after; `fb slots --available-mb N --headroom-mb N --memory-mb N --plan` is pure
   and cheap.
5. Every `#![allow(dead_code)]` you make unnecessary is REMOVED. After wiring, a module whose
   `run` is called no longer needs it, and leaving it hides the next unreachable module.
   **Removing it is inside this task's scope only where the module is `main.rs` itself** — you
   may not edit the other files. So instead: NAME in your handoff each module whose allow is now
   removable, so a follow-up can delete them.
6. `fb --help` lists the new subcommands.

## Boundaries, at N and at zero

- A task name that does not exist: whatever the module returns for a missing score file —
  clause 2. Do not add a pre-check in `main.rs`; the module already decides.
- No argument where one is required: clap's own usage error, exit 2. Do not catch it.
- `fb prices` and `fb sem` take no task argument. Pin that they run without one.

## Superset status on every enumerated list

This task adds EXACTLY four subcommands and changes no existing one.

It adds NO logic. Gathering, rendering and deciding all belong to the modules. A `match` on a
module's output, a format string, or a second way of reading a score file in `main.rs` is the
defect this task exists to remove.

## Composition of aggregate returns

Each subcommand writes what its module writes and exits with what its module returns. Clause 1
and clause 3 are that statement; clause 2 is the one a shell caller depends on, because a
missing score file must not look like an empty comparison.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`. `std::process::exit` is not a panic and is how every other subcommand returns.
- Add no dependencies.
- Change `crates/fb/src/main.rs` and NOTHING else.
- RUN `cargo clippy -p fb --all-targets -- -D warnings` BEFORE you finish, including over your
  tests. Note that `cargo test` may need `cargo build -p fb` first: at least one test in this
  crate shells out to `target/debug/fb` and will read a stale binary otherwise.
- `cargo test -p fb` and that clippy command must pass. Both are clean on HEAD as of this task,
  so any failure is yours.

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
