<!-- fb:modifies crates/fb/src/critique.rs -->
# Task: the path that fans out is the one without state isolation

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/critique.rs`. Do not change any other file.

## Why this exists

Three implementers dispatched concurrently once all hung. Diagnosed: they held the same files
open — `~/.local/share/opencode/opencode.db` and its `-shm`/`-wal` — and the variable that
mattered was concurrency against one shared SQLite database. Not the network, not the cgroup.
(bead farmerbob-p3r)

`fb-dispatch.sh` was fixed. Every dispatched run gets

```sh
FBSTATE="$HOME/.local/share/farmerbob/state/$RUN"
... -E XDG_DATA_HOME="$FBSTATE/data" -E XDG_STATE_HOME="$FBSTATE/state" \
    -E XDG_CACHE_HOME="$FBSTATE/cache" ...
```

and 495 such directories exist. `fb critique` was not fixed. Its `launch` shells out to
`fb_launch`, which sets none of those, so every critique run uses the shared store — currently
275MB — and `fb critique` starts up to `FB_SLOTS` arms CONCURRENTLY against it. The path that
was isolated runs one arm; the path that was not is the one that fans out.

It has not deadlocked again. Nothing prevents it.

The same gap is why critic and prover spend pools into one store and cannot be attributed per
arm (bead farmerbob-wuu). One cause, two symptoms.

## Exact API

`launch`'s signature gains the run identity it needs:

```rust
fn launch(arm: &str, text: &str, worktree: &Path, run: &str) -> Measurement<()>;
```

and sets three environment variables on the `Command` before spawning:

```rust
XDG_DATA_HOME  = <state_root>/<run>/data
XDG_STATE_HOME = <state_root>/<run>/state
XDG_CACHE_HOME = <state_root>/<run>/cache
```

where `<state_root>` is `$HOME/.local/share/farmerbob/state`, matching what `fb-dispatch.sh`
already uses. The three directories are CREATED before the command runs; a launch into a
directory that does not exist is the failure this is meant to prevent, not cause.

`run` is the per-critique identity. Its exact spelling is NOT pinned beyond being unique per
(task, critic) pair and stable across a retry — say in your handoff what you chose.

## Falsifiable clauses

1. Two `launch` calls with DIFFERENT `run` values receive different `XDG_DATA_HOME` values. Pin
   that they differ; do not pin the strings.
2. Two calls with the SAME `run` receive the same value. Stable, not random per call.
3. All three variables are set, and all three point INSIDE that run's directory. Pin that
   `XDG_STATE_HOME` and `XDG_CACHE_HOME` are not left to inherit the caller's environment —
   inheriting is exactly the current behaviour and it is the defect.
4. The three directories exist after `launch` is called. Pin it.
5. `launch`'s return value is unchanged in shape: it still reports a `Measurement<()>` and still
   distinguishes a failed spawn from a non-zero exit as it does today. Pin one existing case.
6. No other behaviour of `fb critique` changes: the derangement, the patch selection, the
   critique files written. Pin the arm-pairing for a three-candidate field before and after.

## Testing this

Spawning a real agent in a test is not acceptable — it costs money and needs the network.
Structure the change so the ENVIRONMENT IS COMPUTED by a function you can call directly, and the
spawning stays in `launch`. Test the computation. Say in your handoff what remains untested
because it only happens at spawn time.

If you cannot separate them without changing `launch`'s signature beyond what is pinned above,
say so in your handoff and test what you can rather than skipping the clause silently.

## Boundaries, at N and at zero

- An EMPTY `run` string: not pinned. Say what you did and do not assert on it.
- A `run` containing a path separator: this becomes a directory name. Say in your handoff
  whether you rejected, sanitised or passed it through, and do not assert on it — but DO say,
  because a `..` in that position escapes the state root.
- The state root not existing at all: it is created. Pin that a first-ever run works.

## Superset status on every enumerated list

EXACTLY three environment variables are set. Adding a fourth — `HOME`, `TMPDIR` — is out of
scope for this task even if it would help; say so in your handoff instead.

This task does not touch `fb-launch.sh`, `fb-dispatch.sh`, or `prove.rs`. `fb prove` has the
same defect and is a separate task; note it in your handoff.

## Composition of aggregate returns

`launch` returns what it returns today. The isolation is a side condition on the environment the
child receives, not a change to the result, and clause 5 is the statement that a caller's
handling is unaffected.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies.
- Change `crates/fb/src/critique.rs` and NOTHING else.
- RUN `cargo clippy -p fb --all-targets -- -D warnings` BEFORE you finish, including over your
  tests. `cargo test -p fb` may need `cargo build -p fb` first: a test in this crate shells out
  to `target/debug/fb` and reads a stale binary otherwise.

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
