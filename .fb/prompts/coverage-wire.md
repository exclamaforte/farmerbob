<!-- fb:modifies crates/fb/src/promote.rs -->
# Task: report a critic that only read the top of the file

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/promote.rs`. Do not change any other file.

`farmerbob_core::promote::coverage` was merged today. It decides whether a critic's citations all
sit in the head of a large file -- the signature of a reviewer who read the opening and inferred
the rest was absent. NOTHING CALLS IT. This is the same gap `verify_wire` closed for
`verify_quotes` an hour earlier: logic in core, unreachable from the binary.

`fb promote` already parses each claim's `WHERE` field into a `Claim`. It never extracts a line
number from it and never counts the subject's lines.

## The incident

A critic filed four claims against an 1892-line file, each alleging something was missing that
the file contains. Its WHERE fields cited lines 67, 74, 152 and 310 -- the first sixth. All four
were promoted.

## Exact API

Keep `run_cmd`'s signature, its exit codes, and `promote_verified` unchanged. Add, public:

```rust
/// The line number named by a `WHERE` field, when it names one.
///
/// Accepts the shapes critics actually write, at least:
///   "crates/fb/src/doctor.rs:1000"        -> Some(1000)
///   "crates/fb/src/doctor.rs:1000:12"     -> Some(1000)
///   "crates/fb/src/promote.rs:~872"       -> Some(872)
///   "crates/farmerbob-core/src/x.rs:283-286" -> Some(283)   // the first of a range
///   "ladder-area"                         -> None
pub fn cited_line(where_field: &str) -> Option<u32>;

/// Coverage per critic for one task, keyed by critic arm, in sorted order.
pub fn critic_coverage(task: &str, s: &Sources) -> BTreeMap<String, Coverage>;
```

`run_cmd` calls `critic_coverage` and prints one line per critic whose coverage is `HeadOnly`.
Print nothing for `Unremarkable` critics: a report that lists every critic teaches the reader to
skim it.

## Falsifiable clauses

1. `cited_line` returns the line for each shape in the doc comment above, and `None` for a WHERE
   naming no number.
2. `cited_line("file.rs:0")` is `Some(0)`. Zero is a real line number to have written, and
   `coverage` already treats it as an ordinary citation.
3. A WHERE containing a number that is not a line -- `"the 300-second timeout"` -- returns
   `None`. Only a number in a `path:line` position counts. Pin this; it is the case that makes
   a naive "first integer in the string" implementation wrong.
4. `critic_coverage` groups by CRITIC, not by subject. A critic filing claims against two
   different files is judged per subject and reported under its own name.
5. A critic whose claims carry no parseable WHERE at all yields `Unremarkable`, never `HeadOnly`.
   No citations is not evidence of a shallow read. This is the clause that matters; say it in
   the doc comment in those words.
6. A subject whose deliverable cannot be read yields `Unremarkable` for every critic of it. An
   unreadable subject has no line count, and a coverage judgement without one is a guess.
7. **`critic_coverage` never changes which claims are promoted.** Run the same task with and
   without it: the promoted list and the rejection list are identical. Pin this as a test.
8. The printed line names the critic, the subject, the deepest cited line and the subject's
   length, so a reader can judge the call without re-deriving it.

## Boundaries, at N and at zero

- A task with no critiques: `critic_coverage` returns an EMPTY map, and `run_cmd` prints nothing
  extra and keeps its current exit code.
- One critic, one claim: `Unremarkable` (one citation cannot establish a pattern).
- A critic that is also a subject: judged in both roles independently, no self-reference problem.
- `Sources.target` empty, or a worktree missing: `Unremarkable`, per clause 6.

## Superset status on every enumerated list

The WHERE shapes in `cited_line`'s doc are a **known subset, marked "at least these"**. A shape
you do not recognise returns `None`, which yields `Unremarkable` -- the safe direction, because
an unrecognised citation must not be read as a shallow one. Say so in the doc comment, and do
not let your tests assert on shapes outside the list.

`Coverage` is closed. Do not add a variant.

## Composition of aggregate returns

`critic_coverage` returns a `BTreeMap`, so its order is sorted by critic name -- say so. Its doc
must state what an EMPTY map means: no critic was judged, which is NOT "no critic was shallow".
Those are different facts and the caller must be able to tell them apart from the value alone.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Use `farmerbob_core::promote::{coverage, Coverage}`. Do not reimplement the thresholds.
- Add no dependencies. `cargo build -p fb` and `cargo test -p fb` must pass. Run them.
- Tests take paths as arguments and must not read the real worktree root or the real logs.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here and
is scored as one.

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
