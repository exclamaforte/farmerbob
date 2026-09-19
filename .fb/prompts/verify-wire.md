<!-- fb:modifies crates/fb/src/promote.rs -->
<!-- fb:reads fb-promote.sh -->
# Task: make `fb promote` actually check a claim before promoting it

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/promote.rs`. Do not change any other file.

`farmerbob_core::promote::verify_quotes` and `verify_all` were merged today. They decide whether
a claim's quoted evidence exists in the file it names. NOTHING CALLS THEM. `fb promote` still
classifies every claim without ever opening the subject, which is the condition the whole
migration exists to end: logic in core, unreachable from the binary.

## What actually happened, so you know what this is for

On 2026-09-17 a critic filed two claims against `liveness.rs` quoting a return value verbatim:

    Cut::Keep { because: "output is too weak to justify ending a run" }

That string occurs zero times in the subject. There is no such code path. `fb promote`
classified both as TESTABLE and queued them for test generation, which spends a cycle proving
that invented code does not behave as invented. On an earlier task the same critic filed four
claims alleging code was missing from a file that contained it, and all four were promoted too.

## Exact API

Keep `pub fn run_cmd(task: &str) -> i32` and its current exit codes.

Add, public:

```rust
/// Where a claim's subject and critic sources are read from, so the wiring is testable
/// against a fake tree.
pub struct Sources {
    /// Worktree root holding `<task>--<arm>` directories.
    pub worktrees: PathBuf,
    /// The deliverable's repo-relative path, e.g. `crates/farmerbob-core/src/gate.rs`.
    pub target: String,
}

/// Read one arm's deliverable. `Missing` when the worktree or file is absent -- which is
/// NOT the same as the file being empty, and must not be collapsed into it.
pub fn read_deliverable(s: &Sources, task: &str, arm: &str) -> Measurement<String>;

/// Classify claims as now, then discard those refuted by their own subject.
///
/// Returns the surviving claims and, separately, every claim refused with the reason.
pub fn promote_verified(
    task: &str,
    s: &Sources,
) -> (Vec<PromotedTest>, Vec<(usize, Rejection)>);
```

`run_cmd` calls `promote_verified` and reports both counts.

## Falsifiable clauses

1. A claim whose quoted evidence appears in the subject survives, exactly as today.
2. A claim quoting text in neither subject nor critic is refused as `Fabricated` and does NOT
   appear in the promoted list.
3. A claim quoting text absent from the subject but present in the critic's own deliverable is
   refused as `Projected`.
4. **A claim whose subject deliverable could not be READ is promoted unchanged, never refused.**
   `read_deliverable` returning `Missing` must not become `Fabricated`. A file we failed to open
   is not evidence that a quote is absent from it, and refusing a claim on that basis would
   discard true findings about files the harness could not reach. This is the clause that
   matters; state it in the doc comment in those terms.
5. The critic's own deliverable being unreadable degrades `Projected` to `Fabricated` -- the
   quote is still absent from the subject -- but never the reverse, and never `None`.
6. Existing rejections (`Incomplete`, `NotFalsifiable`, `NotAClaim`, `Duplicate`) are applied
   BEFORE verification and keep their current behaviour and order.
7. The stdout summary reports refused-by-verification separately from the existing counts. It is
   a wire format other scripts grep, so add a line rather than changing an existing one.

## Boundaries, at N and at zero

- Zero claims: the same output shape as today, with a verification count of 0.
- Every claim refused: the promoted list is empty and the run still exits 0. "All claims were
  false" is a successful measurement, not a failure.
- A task with no worktrees at all: every claim is promoted unchanged (clause 4), and the summary
  says the subjects were unreadable rather than printing a silent zero.
- `Sources.target` empty: `read_deliverable` returns `Missing` with a stated reason.

## Superset status on every enumerated list

`Rejection` is a closed set and stays closed; you are adding no variants. But say in
`promote_verified`'s doc that verification decides only the shapes `verify_quotes` knows --
fabrication and projection -- and that a surviving claim has NOT been shown true. FALSE-ABSENCE,
a claim alleging something is missing from a file that contains it, is a known third shape this
does not detect.

## Composition of aggregate returns

`promote_verified`'s doc must state what both returned vectors contain for an EMPTY claim set,
and that the rejection vector is ordered by ascending index. An empty rejection vector from a
non-empty input means "nothing was refused"; from an empty input it means "nothing was
examined". Say that the caller is responsible for knowing which it asked.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Use `farmerbob_core::promote::{verify_quotes, verify_all, Rejection, Assertion, PromotedTest}`.
  Do not reimplement any of it.
- Use `farmerbob_core::measurement::Measurement` for anything that can fail to be determined.
- Add no dependencies. `cargo build -p fb` and `cargo test -p fb` must pass. Run them.
- Tests must not touch the real worktree root. Take paths as arguments; that is what `Sources`
  is for.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment saying what you could not verify. Returning less with a stated reason is correct here
and is scored as one. Given that this task is a fabrication detector, inventing behaviour you
did not implement would be a conspicuous way to fail it.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- only the crate named in the task is modified

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
   it. Count is the fallback, not the target:
   assertions. Your tests must be good enough to catch a bug in **any** correct-looking
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

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
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
