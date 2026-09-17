<!-- fb:modifies crates/farmerbob-core/src/promote.rs -->
# Task: flag a critic that reviewed only the top of the file

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/promote.rs`. Do not change any other file.

Read the module first. It already refuses claims for being incomplete, unfalsifiable, a
judgement, a duplicate, fabricated or projected. You are adding a SEVENTH signal, and it is
deliberately not a refusal.

## The incident

On 2026-09-17 a critic filed four claims against an 1892-line file, each alleging something was
missing that the file contains: a 300-second timeout (present, line 1137, with a cancellable
killer thread) and a custom JSON emitter (present, line 715, written precisely for the reason
the claim said was unhandled). Every line number it cited fell below 330 -- the first sixth.

It had read the opening of the file and inferred the rest was absent. All four claims were
promoted and queued for test generation.

This is the third named critic failure. FABRICATION (evidence that exists nowhere) and PROJECTION
(evidence from the critic's own file) are already detected by `verify_quotes`. Neither catches
this one, because the quoted evidence is real -- the critic quotes code that genuinely is on the
lines it read. What is false is the INFERENCE that nothing relevant lies below.

## Why this is a suspicion and not a rejection

A claim from a partial read may still be TRUE. Reading the first sixth of a file and finding a
genuine defect there is ordinary reviewing. Refusing such claims would discard real findings, and
this project has already lost a correct arm once to an adjudicator's invented defect.

So this signal describes the CRITIC's coverage, not the claim's truth. It must not remove a claim
from the promoted list. Say this in the doc comment, in those terms.

## Exact API

Keep every existing public item unchanged: `Assertion`, `Rejection`, `PromotedTest`, `promote`,
`fingerprint`, `objective_leaks`, `verify_quotes`, `verify_all`. `Rejection` gains NO variant.

Add, public:

```rust
/// How thoroughly a critic's citations cover the file they are about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    /// Citations reach past the head of the file, or there is not enough evidence to judge.
    Unremarkable,
    /// Every cited line sits in the head of a large file: the critic appears to have read
    /// the opening and inferred the rest. The claims may still be true.
    HeadOnly {
        /// The largest line number cited.
        deepest_cited: u32,
        /// Total lines in the subject.
        subject_lines: u32,
    },
}

/// Judge how far a critic's citations reach into the file.
pub fn coverage(cited_lines: &[u32], subject_lines: u32) -> Coverage;
```

## The three thresholds, and they are yours to defend

`coverage` returns `HeadOnly` only when ALL of these hold. Define each as a named `const` with a
doc comment giving its reason; a bare number in an `if` is not acceptable here.

- **At least 3 citations.** One or two cannot establish a pattern; a reviewer who finds one real
  defect on line 40 and says so has done nothing wrong.
- **The subject is at least 400 lines.** Clustering in the first quarter of a 60-line file is
  meaningless -- there is no "rest of the file" to have missed.
- **Every cited line is at or below 25% of the file.** The real incident was 330 of 1892, which
  is 17%.

## Falsifiable clauses

1. The real case: `coverage(&[67, 74, 152, 310], 1892)` is `HeadOnly { deepest_cited: 310,
   subject_lines: 1892 }`.
2. Citations spread through the file -- `&[67, 900, 1500]`, 1892 -- is `Unremarkable`.
3. A single deep citation among shallow ones is enough to clear it: `&[67, 74, 1800]`, 1892 is
   `Unremarkable`. One line read at the bottom proves the critic got there.
4. Two citations, however shallow, are `Unremarkable`.
5. A 300-line subject is `Unremarkable` whatever the citations.
6. An empty `cited_lines` is `Unremarkable`, NOT `HeadOnly`. No citations is not evidence of a
   shallow read; it is no evidence at all. This is the clause that matters -- state it in the
   doc comment in those words.
7. `subject_lines == 0` is `Unremarkable`. A subject whose length is unknown cannot be judged.
8. A cited line GREATER than `subject_lines` -- a stale or wrong citation -- is `Unremarkable`,
   and must not panic or compute a ratio above 1.

## Boundaries, at N and at zero

- Exactly 3 citations: eligible. Exactly 2: not.
- Exactly 400 lines: eligible. 399: not.
- A citation exactly at 25% of the file: still `HeadOnly`. At 25% plus one line: not. Pin both
  sides; a boundary the spec leaves open is how three earlier tasks were decided.
- `cited_lines` containing 0, or duplicates: neither panics, and both behave as the same set.

## Superset status on every enumerated list

`Coverage` is a CLOSED set of two, like `Verdict` and `Rejection`. It stays closed.

But say in `coverage`'s doc that the ways a critique can be shallow are OPEN, and this function
recognises exactly one of them -- citations clustered in the head. A critic that reads the whole
file and reasons badly about it is not detected here and is not meant to be.

## Composition of aggregate returns

If you add any function over several critics or several claim sets, its doc must state what it
returns for an EMPTY input and whether the order is input order, pinned by tests. "No critic was
shallow" and "no critic was examined" must not be the same value.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. Integer arithmetic only -- no floats for the ratio, and no overflow at
  `u32::MAX`. Pin a test at the maximum.
- Do not change `Rejection`, and do not make `coverage` remove anything from a promoted list.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment saying what you could not verify. Returning less with a stated reason is correct here and
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
