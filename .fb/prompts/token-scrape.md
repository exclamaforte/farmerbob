<!-- fb:modifies crates/farmerbob-core/src/cost.rs -->
# Task: recover token counts the launchers already print

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/cost.rs`. Do not change any other file.

Read `cost.rs` first: `RunCost`, `ArmCost`, `aggregate`, `frontier`, `fully_measured`. You are
adding an input to that machinery, not changing it.

## Why this blocks the project's headline claim

`fb pareto` ranks arms on cost against capability. Today it says:

    ARM               COMPLETE    N    TOTAL $   $/SUCCESS   TOKENS  UNPRICED   FRONTIER
    glm-53-flash          100%   17        n/a         n/a      n/a     17/17
    codex-luna             97%   32        n/a         n/a      n/a     32/32
    or-nemotron-ultra     100%    5     0.0000      0.0000   875053       -     <= pareto
    ifm-k2-horizon         50%    4     0.0000      0.0000  5314558       -     <= pareto
    ifm-k2-think           33%    3     0.0000      0.0000   711557       -     <= pareto

The two arms with the most runs and the highest completion cannot appear on the frontier at all,
because nothing priced them. The frontier is held instead by arms at 50% and 33%, purely because
they are free and were measurable. That is not a result about capability; it is an artefact of
which launchers happen to write a cost store.

The board is right to refuse to invent a number -- it says so: "Absence of a bill is not a bill of
zero." The fix is to recover the number, not to assume it.

## What the launchers actually print

`codex exec` ends a run with two lines, verbatim from
`logs/slots-cmd--codex-luna.log`, and exactly once per run:

    tokens used
    130,826

`zcode` (the glm-53-flash route) prints no token line at all. That is not a gap in this task; it
is a fact about that launcher, and the correct answer for it is `Missing` with a stated reason,
never `0`.

## Exact API

```rust
/// A launcher whose log format this module can read.
///
/// A KNOWN SUBSET of the launchers in use, not a closed set. An unrecognised launcher is
/// [`Launcher::Unknown`] and always yields `Missing`, never a zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    /// `codex exec`: prints `tokens used` then a comma-grouped count on the NEXT line.
    Codex,
    /// `zcode`: prints no token count. Recognised so the reason can say so precisely.
    Zcode,
    /// Anything else.
    Unknown,
}

/// Total tokens a run reported, read from its own log.
///
/// `Missing` distinguishes "this launcher does not report tokens" from "it reports them and
/// this run did not" -- different facts with different reasons, and collapsing them is how
/// an unmeasured arm becomes a free one.
pub fn tokens_from_log(launcher: Launcher, log: &str) -> Measurement<u64>;
```

Use `crate::measurement::Measurement`. Do not add a field to `RunCost` or change `aggregate`.

## Falsifiable clauses

1. `Codex` on `"...\ntokens used\n130,826\n..."` returns `Observed(130826)`. The comma grouping
   is part of the format.
2. `Codex` on a log with no `tokens used` line returns `Missing`, with a reason naming the
   launcher and the absence.
3. `Zcode` on ANY log returns `Missing`, with a reason saying that launcher reports no count --
   not "not found", which would imply it might have.
4. `Unknown` on a log that HAPPENS to contain `tokens used\n130,826` returns `Missing`. We do not
   know that string means the same thing for an unrecognised launcher.
5. A count with no comma -- `"tokens used\n4096"` -- parses to `Observed(4096)`.
6. `tokens used` as the LAST line, with nothing after it, returns `Missing` and does not panic.
7. A line merely containing the words, such as `"the tokens used by the arm were many"`, is NOT
   a report: the marker must be the whole trimmed line. Pin it.
8. Two reports in one log -- a retry -- returns the LAST. State why in the doc comment: the final
   figure is the run's total, and an earlier one describes an attempt that was superseded.

## Boundaries, at N and at zero

- `Observed(0)`: a run that genuinely reported zero tokens is a measurement, not an absence, and
  must not become `Missing`.
- An empty log returns `Missing`.
- A count of `u64::MAX` parses; a count that OVERFLOWS `u64` returns `Missing` with a reason,
  never a wrapped value.
- A count with stray whitespace around it parses.
- A negative or non-numeric payload -- `tokens used` followed by `n/a` -- returns `Missing`.

## Superset status on every enumerated list

`Launcher` is deliberately OPEN in meaning while closed in type: `Unknown` is the escape, and the
doc must say that adding a variant is expected as launchers are added. This is the opposite of
`OutcomeClass` and `Verdict`, which are closed decisions; say in one line why this one differs,
so the distinction survives the next reader.

The marker strings you match are a known subset of one launcher's output and may change when it
is upgraded. Say so.

## Composition of aggregate returns

If you add a function over several logs, its doc must state what it returns for an EMPTY slice
and whether the order is input order, both pinned by tests. "No run reported tokens" and "no run
was examined" must not be the same value.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No I/O: the caller supplies the log text.
- Do not change any existing signature. `crates/fb` must still build.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here and
is scored as one -- and on this task especially, since the whole point is refusing to invent a
number for an arm that did not report one.

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
