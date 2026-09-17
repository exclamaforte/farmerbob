<!-- fb:modifies crates/fb/src/doctor.rs -->
<!-- fb:reads fb-dispatch.sh -->
# Task: make `fb doctor` check the repository, not only the machine

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/doctor.rs`. Do not change any other file.

`fb doctor` checks the ENVIRONMENT -- cgroups, systemd, the GPU, disk headroom, agent CLIs on
PATH. Every one is about the machine. It has no check on the repository's own health, and that
is where this harness's last three outages came from. Read the existing `Check`/`Status` types
and the existing `check_*` functions first; you are adding to that pattern, not replacing it.

## The three incidents, all 2026-09-17

**Formatting drift disqualified arms for a year.** 53 of 77 `.rs` files carried pending rustfmt
diffs, 489 hunks, and nothing reported it. `cargo fmt` is ordinary practice; an arm that ran it
rewrote ~51 files it never meant to touch, and the scope gate counted every one as a departure.
One arm was disqualified from three consecutive tasks that way. The count was 53 and nobody knew.

**An arm the registry called verified could not be launched.** `sources.toml` declares `cmd` and
`args` for every arm; `fb-dispatch.sh` ignores them and keeps its own hardcoded launcher table.
claude-sonnet was re-enabled, dispatched, and returned `rc=127 unknown source` in 0 seconds,
which the harness recorded as the ARM producing nothing. Two copies of one fact, and the
authoritative-looking copy was the dead one.

**A stale spec wastes a whole wave.** A `fb:creates` spec whose target already exists cannot
succeed: every arm correctly no-ops and the harness scores them all as failures.

All three are cheap to detect and none was detected.

## Exact API

Keep `Status`, `Check`, `run` and every existing `check_*` unchanged. Add three checks, each a
private function returning `Check`, and include them in `run`'s output after the existing ones:

```rust
/// `cargo fmt --all --check` over the workspace.
fn check_rustfmt(repo: &Path) -> Check;

/// Every arm `sources.toml` marks dispatchable must have a launcher branch in fb-dispatch.sh.
fn check_launcher_coverage(repo: &Path) -> Check;

/// Every spec in .fb/prompts declares a target, and a `fb:creates` target must NOT exist.
fn check_spec_targets(repo: &Path) -> Check;
```

They take the repo root as an argument so they are testable against a fixture directory. Do not
read the real repository from a test.

## Falsifiable clauses

1. `check_rustfmt` on a tree with no pending diffs is `Ok`.
2. On a tree with pending diffs it is `Warn`, the message states HOW MANY FILES (not hunks), and
   `fix` is `Some("cargo fmt --all")`.
3. `check_rustfmt` when `cargo` cannot be run at all is **`Warn`, never `Ok`.** An unrunnable
   check has not passed. State this in its doc comment in those words.
4. `check_launcher_coverage` is `Ok` when every dispatchable arm appears in the launcher table.
5. When an arm is dispatchable and absent from the table, it is `Fail` and the message NAMES
   every such arm. This costs a whole run when it happens, so it is not a warning.
6. An arm the registry marks `disabled`, or one carrying `redundant_with`, is not dispatchable
   and its absence from the table is not a finding.
7. `check_spec_targets` is `Ok` when every spec declares a target and no `fb:creates` target
   exists on disk.
8. A spec whose `fb:creates` target already exists is `Warn`, naming the spec and the path.
   A spec declaring no target at all is also `Warn`, named separately: those are different
   faults and a reader must be able to tell them apart.
9. Every one of these returning a non-`Ok` status must still let `run` exit with its existing
   code for `Warn`, unchanged. Only `Fail` may change the exit code, and only if `run` already
   behaves that way -- do not alter `run`'s exit contract.

## Boundaries, at N and at zero

- A repo with ZERO `.rs` files: `check_rustfmt` is `Ok`, not `Warn`. Nothing to format is not a
  failure to format.
- A `sources.toml` with zero arms: `check_launcher_coverage` is `Ok`.
- A missing `sources.toml`, or a missing `fb-dispatch.sh`: `Warn`, with a message saying WHICH
  file was unreadable. Never `Ok` -- a check that could not run has not passed, which is the
  same rule as clause 3 and the reason this task exists.
- An empty `.fb/prompts` directory: `check_spec_targets` is `Ok`.
- Exactly one offending item, and many: the message must be correct for both. Pin both. A
  message reading "1 files" is a defect.

## Superset status on every enumerated list

`Status` is a CLOSED set -- `Ok`, `Warn`, `Fail` -- and stays closed. Do not add a variant.

Any list of arm-name prefixes you use to decide launcher coverage is a KNOWN SUBSET: the table
matches some arms by prefix (`ifm-*`, `or-*`) and some by exact name. An arm whose name you
cannot classify must be reported as UNKNOWN rather than silently assumed covered. Say so in the
doc comment. Assuming coverage is how claude-sonnet passed eligibility and then failed to launch.

## Composition of aggregate returns

If you add a helper returning a collection of offending names, its doc comment must state what
it returns for an EMPTY input and whether the order is sorted or source order, and both must be
pinned by tests. A caller cannot tell "nothing was wrong" from "nothing was examined" unless the
function says which it means.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies.
- Reuse `Check` and `Status`. Do not define a parallel result type.
- `cargo build -p fb` and `cargo test -p fb` must pass. Run them.
- Tests must not shell out to the real `cargo fmt` over the real repository, and must not read
  the real `sources.toml`. Take the repo root as an argument and build a fixture.

## Do not fabricate

If part of this cannot be done in your environment -- running `cargo fmt` from a test, say --
say so plainly, implement the closest honest thing, and name what you could not verify. Returning
less with a stated reason is correct here and is scored as one.

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
