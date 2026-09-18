<!-- fb:creates crates/fb/src/scope_cmd.rs -->
# Task: two merged gates, and nothing calls either of them

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/scope_cmd.rs` and add `mod scope_cmd;` to `crates/fb/src/main.rs`.
Do not change any other file.

**This is a wiring task, not a pure-logic one.** It reads real worktrees and runs real `git`.
Eleven modules have been merged into `farmerbob-core` in recent waves and two of them have a
caller; the rest are tested logic nothing runs. This closes one of those gaps.

## Why this exists

`scope::assess` decides whether a run stayed inside its declared deliverable. It compares PATHS,
and its single allowance is the `lib.rs` beside the target -- so anything done INSIDE that
`lib.rs` passes. An arm changed three existing declarations there and scored clean; a rival
caught it by reading the diff, and the adjudicator had to disqualify by hand.

`lib_diff::classify` was merged to fix exactly that: it reads the `lib.rs` diff and says whether
it is declarations only, or `Beyond` with every offending line. Nothing calls it either.

This command runs both against a real worktree and prints a verdict.

## Exact API

```rust
/// Assess one arm's worktree, or every arm of a task when `arm` is `None`.
///
/// Returns a process exit code: 0 when every assessed worktree is clean,
/// 1 when any departed, 2 when nothing could be assessed.
pub fn run_cmd(task: &str, arm: Option<&str>) -> i32;
```

Everything else in the file is private. The command is wired as
`fb scope <task> [--arm <arm>]`.

## What it must do

1. Read the task's declared target from `.fb/prompts/<task>.md`, using the SAME marker forms the
   dispatcher uses: an HTML comment carrying `fb:creates <path>` or `fb:modifies <path>`.
   **Recognise a marker only at the start of a line.** A spec that discusses the markers in prose
   must not be read as declaring them; this project refused a whole wave to that bug.
2. For each worktree at `<worktrees>/<task>--<arm>`, collect the changed paths with
   `git diff --name-only HEAD` plus `git ls-files --others --exclude-standard`.
3. Hand those to `scope::assess` as `Change` values and report its `Scope`.
4. When the changed set includes the `lib.rs` beside the target, additionally collect that
   file's diff, convert it to `lib_diff::DiffLine` values, and report `lib_diff::classify`.
5. Print, per arm, one line of verdict plus every departure and every offending `lib.rs` line.

## Falsifiable clauses

1. A worktree whose only change is the declared target is CLEAN, and `run_cmd` returns 0.
2. A worktree that also adds `pub mod y;` to the neighbouring `lib.rs` is CLEAN. That is the
   allowance and it must survive.
3. A worktree whose `lib.rs` diff contains anything else is NOT clean, and every offending line
   is printed. This is the store-batch case and the reason the task exists.
4. A worktree that changed a file which is neither the target nor that `lib.rs` is not clean,
   and the foreign path is named.
5. **A task whose spec declares no target returns 2 and assesses nothing.** Not 0. An
   unassessable run and a clean run must not print the same verdict -- that is the failure this
   project names most often.
6. A worktree that does not exist returns 2 for that arm and does not count as clean.
7. `git` failing -- not a repository, no admin entry -- returns 2 for that arm and SAYS which
   command failed. 103 worktrees in this store have lost their admin directory; an empty change
   list from a broken worktree must never read as "this arm stayed in scope".
8. With no `--arm`, every directory matching `<task>--*` is assessed and the exit code is 1 if
   ANY departed.

## Boundaries, at N and at zero

- A task with no worktrees at all: return 2, and say so rather than printing a clean table.
- A worktree with NO changes: clean, and say it changed nothing. Distinguish that in the output
  from a worktree that changed only its target.
- A `lib.rs` diff that is empty: `lib_diff::Untouched`, which is clean.
- A target that is itself a `lib.rs`: `scope::module_declaration_for` returns `None` for that
  case; there is no neighbouring lib.rs to check, and step 4 is skipped. Pin it.
- An arm name containing `--`: the directory is `<task>--<arm>`, so split on the FIRST `--`
  after the task prefix. Say what you do.

## Superset status on every enumerated list

The MARKER FORMS are a CLOSED set of two, `fb:creates` and `fb:modifies`. The EXIT CODES are a
closed set of three: 0, 1, 2. There is no "warn" code -- a departure is a departure.

`scope::{assess, Change, Declared, Scope, Departure, module_declaration_for}` and
`lib_diff::{classify, DiffLine, LibChange, permitted}` are merged and CLOSED. **Use them,
imported. Do not reimplement either rule here.** Reimplementing the scope rule in a second
place is how this harness ended up with four copies of its verdict logic.

## Composition of aggregate returns

The exit code is the AGGREGATE over every arm assessed: 0 only if every one was clean, 1 if any
departed, 2 if none could be assessed. Say what happens when some arms are assessable and others
are not -- that is the case a reader guesses wrong, and it must be pinned rather than left to
the implementation.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. `std::process::Command` for git; `crate::paths` for locations.
- Tests may build `Change` and `DiffLine` values directly and test the private helpers; they may
  not shell out to git, and they may not require a worktree to exist.
- `cargo build -p fb` and `cargo test -p fb` must pass. Run them.
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
