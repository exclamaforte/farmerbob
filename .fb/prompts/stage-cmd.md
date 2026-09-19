<!-- fb:creates crates/fb/src/stage_cmd.rs -->
# Task: the rule is in the crate and the shell still decides for itself

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/stage_cmd.rs` and add `mod stage_cmd;` to `crates/fb/src/main.rs`.
Do not change any other file.

## Why this exists

`farmerbob_core::stage_outcome` was merged to end one coercion: a stage that exits 0 and writes
no artefact did not run, and "I could not look at the artefact" is a third answer that neither
"ran" nor "produced nothing" covers. It has no caller.

`fb-pipeline.sh` still decides for itself, in shell:

```sh
if ! "$@" >> "$LOGS/$T.pipeline.log" 2>&1; then
  echo "  $name: FAILED"; return 1
fi
if [ ! -s "$artefact" ]; then
  echo "  $name: RAN BUT PRODUCED NOTHING at $artefact"; return 1
fi
```

`[ ! -s "$artefact" ]` is true when the file is missing, when it is empty, and when the shell
cannot stat it at all. Three facts, one branch. The comment above that line in the script says
the two cases "need different fixes" -- and the log still cannot tell them apart, so an operator
must stat the file by hand to know which bug to chase. (accepted follow-up, glm-53-flash on
stage-outcome)

This task gives the rule a caller, so the shell asks instead of deciding.

## Exact API

```rust
/// `fb stage <name> --rc <i32> --artefact <path>`
///
/// Prints one line and exits 0 when the stage may be signed, 1 when it may not.
pub fn run(args: &[String]) -> i32;
```

`run` takes the already-split command-line arguments AFTER the subcommand word, parses them,
stats the artefact itself, and calls `farmerbob_core::stage_outcome::classify` and
`may_sign`. It must not re-implement either rule.

THE GRAMMAR IS PINNED, exactly:

    <name> then the two flags, in either order

- `args[0]` is the stage NAME. It is required and it must not begin with `--`.
- `--rc <i32>` and `--artefact <path>` follow, each as TWO arguments. The `--flag=value` form is
  not accepted; that is clause 7.
- The two flags may appear in either order, and both are required.
- A flag given twice, an unrecognised flag, a flag with no value, or any argument after the
  last flag's value is clause 7.

Nothing else is legal input. This is stated because the first draft of this spec pinned only
"a stage name and two flags", which two implementations could read four different ways.

The artefact's size becomes the `Measurement<u64>` that `classify` takes:

- the file exists and its length is readable: `Measurement::observed(len)`, and `len` may be 0;
- the file does not exist: `Measurement::observed(0)`, because "not created" IS an observation
  that it holds no bytes;
- the metadata call fails with `ErrorKind::NotFound`: `Measurement::observed(0)`, exactly as
  above. **A BROKEN SYMLINK IS THIS CASE, not the next one.** `std::fs::metadata` follows
  symlinks and reports `NotFound` for a dangling one, so it is indistinguishable from a file
  that was never created and must be treated as such. The first draft of this spec listed a
  broken symlink as an instrument failure, which no implementation could have computed.
- the metadata call fails for ANY OTHER reason -- a permission error, an I/O error, a path
  component that is not a directory: `Measurement::instrument_failed(..)` with the OS error in
  the reason. This is the case the shell cannot express and the reason this command exists.

## Falsifiable clauses

1. `rc != 0` prints a line naming the stage and `exit 1`, whatever the artefact says. Pin a
   non-zero rc with a large healthy artefact.
2. `rc == 0` with a non-empty artefact prints a line naming the stage and exits 0.
3. `rc == 0` with an artefact that exists and is EMPTY exits 1, and the line says the stage
   produced nothing. Assert that it mentions producing nothing, case-insensitively; do not
   assert the sentence.
4. `rc == 0` with an artefact that DOES NOT EXIST exits 1 and reports the same
   `ProducedNothing` outcome as clause 3, because both are `Observed(0)`.
5. `rc == 0` with an artefact that cannot be stat-ed exits 1 and the line distinguishes it from
   clauses 3 and 4: it must mention that the artefact could not be inspected AND carry the OS
   reason. This is the whole point; pin it.
6. The exit status is `may_sign` negated, for every outcome: 0 exactly when the stage may be
   signed. Pin that no other combination of rc and artefact produces exit 0.
7. Malformed input -- a missing `--rc` or `--artefact`, an `--rc` that is not an integer, a
   repeated or unknown flag, a flag with no value, a first argument beginning with `--`, or a
   trailing argument -- prints a usage line **to stderr**, prints NOTHING to stdout, and exits
   2. Two is neither "may sign" nor "may not"; it is "you asked wrongly", and it is not a
   classification, which is why it does not use the stdout channel the classification owns.

## Boundaries, at N and at zero

- An artefact of exactly 0 bytes: clause 3. An artefact of exactly 1 byte: clause 2. Pin both
  sides.
- `--rc 0`: the only rc that can lead to exit 0.
- A NEGATIVE `--rc`, which is what a signal-killed process reports through some launchers:
  non-zero, so clause 1, and the value is preserved verbatim in the line.
- `args` EMPTY: clause 7, usage on stderr, exit 2.
- An artefact path that is a DANGLING SYMLINK: `Observed(0)`, so clause 4, not clause 5. Pin it,
  because it is the one case where the honest answer and the tempting answer differ.
- A stage `name` that is the empty string: degenerate, not invalid. It classifies and prints
  like any other; the line is still non-empty.
- An `--artefact` path that is a DIRECTORY: its metadata reads, and its length is whatever the
  filesystem reports. Say what you do and do not let your tests assert on the length, because
  it is the filesystem's answer and not this program's.

## Superset status on every enumerated list

`StageOutcome`'s variants are a CLOSED set of exactly five: `Ran`, `ProducedNothing`, `Failed`,
`Unknown`, `Skipped`. You define none of them, you add none, and your tests may not assert on
any outcome outside them. `Skipped` is never produced here -- this command is only called for a
stage that was attempted -- and saying so in your handoff is required.

The exit statuses are a CLOSED set of exactly three: `0`, `1`, `2`.

The flags accepted are a CLOSED set of exactly two: `--rc` and `--artefact`. An unrecognised
flag is clause 7.

## Composition of aggregate returns

`run` prints EXACTLY ONE line to stdout for every invocation that CLASSIFIES -- that is, every
invocation exiting 0 or 1 -- and returns exactly one status. The line and the status are two
renderings of one `StageOutcome` and can never disagree: whenever the status is 0 the line
reports `Ran`, and whenever it is 1 the line reports one of the other three. An invocation
exiting 2 classified nothing and writes nothing to stdout; its usage line goes to stderr.
Clause 7.

Say why clauses 3, 4 and 5 must be tested TOGETHER rather than separately: they are the three
facts the shell collapses into one branch, and a test of any one alone passes against an
implementation that collapses the other two. That collapse is the defect this command exists to
remove.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. `farmerbob-core` is already one.
- Tests that need a file must create it in a temporary directory they make and remove; do not
  read this repository's own logs, which are mutable state shared with a running harness.
- `cargo test -p fb` must pass and `cargo clippy -p fb -- -D warnings` must be clean. Both are
  clean on HEAD as of this task, so any failure is yours.
- `main.rs` gains the `mod stage_cmd;` line and nothing else. Wiring the subcommand into the
  dispatch match is a SEPARATE task and is not yours; say in your handoff that `run` has no
  caller yet.

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
