<!-- fb:creates crates/fb/src/bench_cmd.rs -->
# Task: the benchmark loop is complete except for the part that runs it

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/bench_cmd.rs`. Declare it in `main.rs` with one `mod bench_cmd;` line and
change nothing else. `fb` is a binary crate with no `lib.rs`.

## Why this exists

Four merged modules now form a complete benchmark path, and nothing connects them because
nothing runs a process:

- `trial_plan::next` decides whether to run another trial, stop with enough, or abandon.
- `bench_read::verify` / `::bench` turn one run's captured stdout into a measurement.
- `task_contract::score` turns verification plus samples into a `Score`.
- `gate_kind::cleared` turns that `Score` into a gate decision.

Every one of them is pure by design, and every one of them is reachable from nothing. This is
the impure layer: spawn `verify.sh` once and `bench.sh` per trial, capture stdout and exit
status, and drive the loop the other four describe. (bead farmerbob-p172)

## Exact API

```rust
/// Where a task's scripts live and how hard to try.
pub struct BenchPlan {
    /// Directory containing verify.sh and bench.sh.
    pub dir: PathBuf,
    /// Seconds before one trial is killed. Zero means no limit.
    pub timeout_s: u64,
    /// Total trials that may be attempted.
    pub max_attempts: u32,
    /// Unreadable runs tolerated before the instrument is judged unreliable.
    pub max_bad: u32,
}

/// What a whole benchmark run produced.
pub enum Outcome {
    /// Verification and enough trials. Carries what the scorer needs.
    Measured {
        verify: VerifyOutput,
        samples: Vec<f64>,
    },
    /// Verification ran and said the implementation is INCORRECT. This is a
    /// result about the candidate, and no trials were attempted.
    Incorrect(VerifyOutput),
    /// The instrument failed: verification could not be read, or too many
    /// trials were unreadable, or attempts ran out. Carries a reason.
    Instrument(String),
}

/// Run verification, then trials, driving trial_plan.
pub fn run_bench(m: &TaskManifest, p: &BenchPlan) -> Outcome;
```

`TaskManifest` and `VerifyOutput` are `farmerbob_core::task_contract`'s.

## The order, pinned

Verification runs FIRST and exactly once. If it reads as incorrect, NO trials are attempted and
the answer is `Incorrect` — timing a kernel that computes the wrong answer measures nothing.
Only a correct verification leads to trials.

## Falsifiable clauses

1. A verify script printing `{"correct":true,"detail":"ok"}` and a bench script printing
   `{"ms":5}` each trial, with `min_trials: 3`, yields `Measured` with three samples.
2. A verify script printing `{"correct":false,"detail":"mismatch"}` yields `Incorrect`, and
   `bench.sh` is NEVER EXECUTED. Pin the non-execution — have `bench.sh` create a file and
   assert the file does not exist. This is the clause that carries the task: a wrong kernel that
   is fast is still wrong, and timing it wastes the machine.
3. A verify script that exits non-zero yields `Instrument`, NOT `Incorrect`. Pin clauses 2 and 3
   as a PAIR: a verification that RAN and said no is a result about the candidate; one that
   crashed is a result about the harness, and they must not share an answer.
4. A bench script that fails more than `max_bad` times yields `Instrument`, even if enough good
   samples were already collected. That is `trial_plan`'s rule and this must not override it.
5. Trials stop as soon as `trial_plan` says `Enough`. With `min_trials: 3` and
   `max_attempts: 10`, a bench script that always succeeds is executed exactly THREE times.
   Assert the count — have the script append to a file and count the lines.
6. `timeout_s` kills a trial that exceeds it, and that trial counts as BAD, not as a
   measurement. Pin with a script that sleeps longer than the timeout.
7. `timeout_s: 0` means no limit. Pin that a short script still completes.
8. The decision to continue comes from `trial_plan::next` and the reading from `bench_read`.
   Assert agreement rather than re-deriving: a count comparison or a JSON parse in this file is
   the defect this task exists to avoid.

## Boundaries, at N and at zero

- `max_attempts: 0`: `Instrument`, and neither script runs. `trial_plan` already says Abandon.
- `min_trials: 0` with a correct verification: `Measured` with zero samples, and `bench.sh` is
  not executed. Say in your handoff whether you think that is useful; it is what the manifest
  asked for.
- `dir` containing no `verify.sh`: `Instrument` naming what was missing.
- `dir` containing `verify.sh` but no `bench.sh`, with a correct verification: `Instrument`, NOT
  `Measured` with zero samples. Pin it against the `min_trials: 0` case above — they differ in
  whether the script was ABSENT or merely unneeded.

## Superset status on every enumerated list

`Outcome` has EXACTLY three variants. The two script names are EXACTLY `verify.sh` and
`bench.sh`, matching what `task_contract` documents as their producers.

This module makes no scoring decision. It does not compute a speedup, does not compare against
a baseline, does not decide whether a candidate cleared anything. `task_contract::score` and
`gate_kind` do that, and they are called by whoever calls this.

## Testing this

Write real shell scripts into a temporary directory and run them. That is the point of this
module and there is no honest way to test it without spawning. Do NOT fabricate a fake spawner
and test that instead — the defect this module can have is that it runs the wrong thing in the
wrong order, which a fake cannot catch. Keep the scripts trivial and fast.

## Composition of aggregate returns

`Measured.samples` holds the good measurements in the order the trials produced them, and its
length is what `trial_plan` counted as `good`. `Instrument` carries a reason naming which stage
failed — verification, reading, or attempts — so an operator can tell a broken script from a
flaky one.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. Use `std::process::Command`.
- Do NOT wire a subcommand; that is a separate task. Say so in your handoff, and use
  `#![allow(dead_code)]` with a comment if `-D warnings` needs it — but NAME the allow in your
  handoff so a follow-up removes it, because eight modules in this crate are currently
  unreachable behind exactly that attribute.
- RUN `cargo clippy -p fb --all-targets -- -D warnings` BEFORE you finish, including over your
  tests.

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
