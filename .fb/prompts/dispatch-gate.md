<!-- fb:modifies crates/farmerbob-core/src/spec_gate.rs -->
<!-- fb:modifies crates/farmerbob-core/src/slots.rs -->
<!-- fb:modifies crates/farmerbob-core/src/lib.rs -->
<!-- fb:modifies crates/fb/src/admit_cmd.rs -->
<!-- fb:modifies crates/fb/src/speccheck_cmd.rs -->

# Task: nothing rules on a spec before an arm is paid to read it

Rust workspace, already builds. Create `crates/farmerbob-core/src/spec_gate.rs`; modify the four
other declared files and nothing else.

## Why this exists

Three separate places in the dispatch path answer a question by not asking it. This is one task
because they are the same defect wearing three hats: **a gate whose failure is indistinguishable
from its success.**

### Defect 1 — admission never rules on the spec (bead farmerbob-mu7s)

`admit_cmd.rs` composes a spec path and spawns the arm:

```rust
let spec = format!(".fb/prompts/{}.md", run.task);
match std::process::Command::new(&exe)
    .args(["dispatch", &run.arm, &run.task, &spec, &run.krate])
```

Between those two lines there is no check that the file exists and no consultation of any
verdict about it. `fb speclint` and `fb speccheck` both exist and both produce findings; neither
is in this path. On 2026-09-19 a spec failed `fb speclint` with exit 1 and was queued into a
wave by the same command, because the only thing that reads a lint verdict is a human reading
scrollback.

`farmerbob_core::speclint_shape::lint(&str) -> Vec<Lint>` is pure and already a dependency of
this workspace. Admission can rule for itself; it does not need a prior stage to have run.

### Defect 2 — `capacity()` and `admit()` disagree (bead farmerbob-7vh)

In `slots.rs`, with `budget.memory_mb == 0`:

- `capacity(machine)` returns `0` — it guards the division explicitly.
- `admit(run, machine)` computes `committed = n * 0 = 0` and `requested = 0`, so
  `0 <= ceiling` holds for every run and it admits **without bound**.

`memory_mb == 0` is what an unmeasured budget looks like. The machine sizer answers "this
machine holds nothing" and the admitter answers "this machine holds everything", from the same
two structs. The disagreement is not confined to zero: they are two independent implementations
of one question and only coincide by arithmetic accident.

### Defect 3 — a working launcher and a hung one print the same thing (bead farmerbob-ei23)

`speccheck_cmd.rs` prints a header, then loops over arms calling the blocking
`crate::launch::launch`, then prints a result line per arm once it returns. Nothing is written
before the call. For a serial run of N arms, the operator sees the header and then silence for
as long as the arms take, and that silence is byte-identical to a launcher that will never
return.

## Exact API

### `crates/farmerbob-core/src/spec_gate.rs` (new; add `pub mod spec_gate;` to `lib.rs`)

```rust
use crate::measurement::Measurement;

/// Whether one spec may be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ruling {
    /// A gate ran and found nothing disqualifying.
    Clear,
    /// A gate ran and found a reason not to dispatch.
    Refused { why: String },
    /// No gate reached a verdict. NOT a pass.
    Unruled { why: String },
}

/// Rule on one spec.
///
/// `present` is whether the spec file is there: `Observed(false)` means it was looked for and
/// is not there, which is a finding; `Missing(_)` means the lookup itself failed, which is not.
/// `findings` is the lint result for the spec's text, absent when the text could not be read.
pub fn ruling(present: Measurement<bool>, findings: Measurement<Vec<String>>) -> Ruling;
```

The three arms of `Ruling` are fixed here. Do not add a fourth and do not collapse `Unruled`
into `Refused`: the caller treats them differently and a test below pins that.

`ruling` decides in this order:

1. `present` is `Missing(a)` -> `Unruled { why }`, and `why` contains `"spec"`.
2. `present` is `Observed(false)` -> `Refused { why }`, and `why` contains `"no spec"`.
3. `findings` is `Missing(a)` -> `Unruled { why }`.
4. `findings` is `Observed(v)` and `v` is non-empty -> `Refused { why }`, and `why` contains
   every string in `v`.
5. Otherwise -> `Clear`.

Both `Unruled` reasons must carry the `Absent`'s own text. `Missing(Absent::InstrumentFailed {
reason })` must produce a `why` containing `reason`; a `why` that names only the stage and
discards the instrument's account of itself is the defect this task exists to remove.

### `crates/farmerbob-core/src/slots.rs`

`admit` keeps its signature. Add one refusal ahead of the existing arithmetic: a run is queued,
not admitted, when the pool is already at the capacity this machine supports.

```rust
if self.active() >= self.capacity(machine) {
    return Admission::Queued { reason: /* see below */ };
}
```

placed **after** the "already holds a slot" early return (a run that holds a slot still gets it
back) and **before** the ceiling arithmetic. The reason text is fixed:

- when `self.budget.memory_mb == 0`:
  `"budget.memory_mb is 0 -- the per-run budget is unmeasured, so no run can be sized"`
- otherwise:
  `format!("{} active at capacity {}", self.active(), self.capacity(machine))`

The first string is required verbatim; a test asserts it contains `"unmeasured"`.

`capacity` is unchanged. After this change `admit` and `capacity` agree at every value of
`memory_mb`, because there is now one implementation and `admit` calls it.

### `crates/fb/src/speccheck_cmd.rs`

Extract the per-arm body so the launcher is a parameter, as `crossx.rs` already does for its
cargo runner:

```rust
pub fn check_one(
    log_dir: &std::path::Path,
    arm: &str,
    launcher: &dyn Fn() -> Measurement<String>,
) -> Measurement<String>;
```

`check_one` must, in this order:

1. write `log_dir/<arm>.started` (contents are unconstrained; the file's existence is the
   signal),
2. call `launcher()`,
3. write `log_dir/<arm>.log` from the result exactly as the current code does,
4. return the launcher's result.

Step 1 happening before step 2 is the whole point and is what the test pins. Leave
`<arm>.started` in place afterwards: `<arm>.started` without `<arm>.log` is a run that began and
did not finish, and that is the state the operator currently cannot see.

`run` calls `check_one` for each arm, passing a closure over the existing `crate::launch::launch`
call, and prints `"  {arm:<22} started"` before it. Everything `run` prints today it still
prints.

### `crates/fb/src/admit_cmd.rs`

Before the `Command::new(&exe)` that spawns `fb dispatch`, rule on the spec and act on it:

- read `repo.join(&spec)`; on `Ok(text)` the presence is `Observed(true)` and the findings are
  `Observed(speclint_shape::lint(&text).iter().map(|l| l.to_string()).collect())`;
- on `Err` with `ErrorKind::NotFound`, presence is `Observed(false)` and findings are
  `Missing(Absent::NothingToMeasure { .. })`;
- on any other `Err(e)`, presence is `Missing(Absent::InstrumentFailed { reason: e.to_string() })`
  and findings likewise.

Then:

- `Ruling::Clear` -> dispatch, exactly as today.
- `Ruling::Refused { why }` -> do not dispatch. Print `"refused {task}/{arm}: {why}"` to stderr
  and go on to the next run.
- `Ruling::Unruled { why }` -> do not dispatch. Print `"unruled {task}/{arm}: {why}"` to stderr
  and go on to the next run.

A refused or unruled run must not consume a provider slot or a child handle, and must not abort
the rest of the matrix: the other rows still dispatch. `run` returns `0` when at least one run
dispatched and every skip was reported; it returns `1` when the matrix was non-empty and nothing
dispatched at all.

`Lint`'s existing `Display` is what produces the finding strings. If `Lint` has no `Display`,
use `format!("{l:?}")` and say so in your handoff; do not add a `Display` impl to a file this
task does not declare.

## Tests

In-module `#[cfg(test)]`, in the file each item lives in. Required, each an independent
assertion:

1. `ruling(Observed(true), Observed(vec![]))` is `Clear`.
2. `ruling(Observed(false), _)` is `Refused` and the `why` contains `"no spec"`.
3. `ruling(Observed(true), Observed(vec!["a".into(), "b".into()]))` is `Refused` and the `why`
   contains both `"a"` and `"b"`.
4. `ruling(Observed(true), Missing(InstrumentFailed { reason: "disk on fire".into() }))` is
   `Unruled` and the `why` contains `"disk on fire"`. **A `Clear` here is the bug.**
5. `ruling(Missing(..), Observed(vec![]))` is `Unruled`, not `Clear` — an unreadable spec with
   no findings is not a clean spec.
6. A `SlotPool` with `memory_mb: 0` admits its **first** run as `Queued`, and the reason
   contains `"unmeasured"`. Before this change it admitted every run.
7. For some non-zero `memory_mb` and a machine sized to hold exactly two runs: the third
   `admit` is `Queued`, and `capacity(machine) == 2` — the two functions agree on the same
   machine.
8. A run that already holds a slot is re-admitted as `Admitted` even when the pool is at
   capacity.
9. `check_one` with a launcher closure that asserts `log_dir/<arm>.started` already exists at
   the moment it is called. The assertion lives inside the closure, so a `check_one` that
   writes the marker afterwards fails the test rather than passing it.
10. After `check_one` returns, both `<arm>.started` and `<arm>.log` exist.

Use `std::env::temp_dir()` plus `std::process::id()` and a counter for test directories, as
`defects.rs` does; clean up on the way out.

## Rules

- Change the five declared files and nothing else, plus `.fb/handoff.md`.
- `cargo test -p farmerbob-core -p fb` and `cargo clippy --all-targets -- -D warnings` must pass.
- Do not change `capacity`'s behaviour, `admit`'s signature, or anything `speccheck_cmd::run`
  already prints.
- Every existing test in these files must still pass. If one now contradicts this spec, leave it
  failing rather than editing it, and name it in your handoff.

---


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
  measurement is `farmerbob_core::scope`, and it compares paths as exact strings.

  **A task declares a SET of files.** Most declare one. A task that legitimately spans
  several lists them all, and touching any of them is in scope; so is adding `pub mod y;` to
  the `lib.rs` beside each new file. A file outside the declared set is a departure, and a
  departure yields the verdict `OutOfScope`, which is not a pass.

  The set was a single file until 2026-09-19, and that was never a decision -- it was
  inferred from an incident. One arm changed 51, 52 and 51 files across three runs and was
  disqualified for wandering. It had not wandered: it had run `cargo fmt` on a
  rustfmt-dirty repository, and 51 is exactly the number of dirty files its changes
  intersected. The gate was tightened on the strength of a formatter artefact, and the
  formatter bug was fixed separately the same day.

  What that cost was not theoretical. A task adding one field to a struct could not pass if
  any other file in the same crate constructed it: fixing those callers was OutOfScope and
  leaving them was TESTS-FAIL. No legal move, and an arm lost a run to it.

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
