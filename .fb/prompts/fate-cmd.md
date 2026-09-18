<!-- fb:creates crates/fb/src/fate_cmd.rs -->
# Task: the decision exists and nothing can ask it

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/fate_cmd.rs` and add `mod fate_cmd;` to `crates/fb/src/main.rs`.
Do not change any other file.

## Why this exists

`farmerbob_core::finding_fate::decide` was merged to say what should happen to a confirmed
critique finding. Nothing calls it, and the shell that would call it has no way to ask.

Today `fb-escalate.sh` escalates a finding, runs it against the merged reference, and on failure
prints:

    glm-53-flash: VETOED against the merged reference -- retracting

The veto is right to refuse a failing test into a green suite. But the finding was ACCEPTED by
the adjudicator, is real, and then exists nowhere but a bead somebody wrote by hand. The better
the critic, the more likely its finding is about the arm that won, and the more certainly it is
discarded.

This command is the bridge: shell asks, core decides, shell acts. Writing the decision a second
time in shell is how this harness ended up with four copies of its verdict logic.

## Exact API

```rust
/// Decide the fate of a task's findings and print one line per finding.
///
/// `veto` is what happened when the escalated test ran against the merged
/// reference: `passed`, `failed`, or `did-not-run`.
///
/// Returns a process exit code: 0 when every finding was decided, 2 when the
/// task has no adjudication record or no claims to decide, 1 on an I/O or
/// parse failure.
pub fn run_cmd(task: &str, veto: &str) -> i32;
```

Wired as `fb fate <task> --veto <passed|failed|did-not-run>`. Everything else in the file is
private.

## What it must read

- `.fb/adjudicated/<task>` — its first line begins `MERGED <arm>`, naming the arm that won.
  That arm is the merged subject.
- `<logs>/<task>.claims.json` — `{"claims": [...]}`, each claim carrying at least `critic` and
  `subject`. A claim's `subject` is the arm the finding is ABOUT.

For each claim, `subject_merged` is `claim.subject == merged_arm`, and the fate comes from
`finding_fate::decide`.

## Falsifiable clauses

1. A claim whose `subject` equals the merged arm and `--veto passed` prints `ESCALATE`.
2. The same claim with `--veto failed` prints `FILE-AS-KNOWN-DEFECT` and the reason
   `finding_fate` supplies.
3. The same claim with `--veto did-not-run` prints `UNVERIFIABLE`, never either of the above.
4. A claim whose `subject` is NOT the merged arm prints `NO-SUBJECT` for every `--veto` value.
5. **The decision is `finding_fate::decide`'s, imported and called.** Do not reimplement the
   table. A test must pin that all six input combinations agree with `decide` called directly.
6. An unrecognised `--veto` value returns 2 and decides nothing. It is NOT treated as
   `did-not-run`: guessing which of three states the caller meant is how a harness reports a
   fault it never observed.
7. A missing `.fb/adjudicated/<task>` returns 2 and says which file was missing. An
   unadjudicated task has no merged arm, so no fate can be decided -- and 2 rather than 0,
   because "decided nothing" must not read as "everything was fine".
8. A missing or unparseable claims file returns 2 and says which.

## Boundaries, at N and at zero

- A claims file with an empty `claims` array: return 2 and say there was nothing to decide.
  Zero findings decided is not success.
- An adjudication whose first line does not begin `MERGED `: return 2 naming the file. Some
  records begin `VOID` or `SUPERSEDED`, and those tasks have no merged arm.
- A claim missing `subject` or `critic`: skip it, print that it was skipped and why, and do NOT
  let it change the exit code of the others. Say why skipping beats guessing.
- N claims all about the merged arm: N lines, one per claim, in the order the file lists them.
- The same critic appearing twice: two lines. This reports findings, not critics.

## Superset status on every enumerated list

The VETO VALUES are a CLOSED set of exactly three: `passed`, `failed`, `did-not-run`. Anything
else is clause 6. `finding_fate::Fate` is CLOSED at four and defined in `farmerbob-core`; **use
it, imported.** The FIELDS read from a claim are a known subset -- a claim may carry more and
that is not an error.

## Composition of aggregate returns

One line per claim in input order, plus one line per skipped claim. The exit code reflects
whether the COMMAND could decide, not what it decided: a task whose every finding is
`FILE-AS-KNOWN-DEFECT` still exits 0, because deciding that is the command working.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. `serde_json` and `crate::paths` are available.
- Tests build the two input files in a temp directory; they may not require a real task.
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
