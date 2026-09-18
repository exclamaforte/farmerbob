<!-- fb:modifies crates/farmerbob-core/src/precondition.rs -->
# Task: pin what a marker IS, not only where one may appear

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/precondition.rs`. Do not change any other file.

## Why this exists

`precondition.rs` (merged separately) decides whether a queued spec can still run against the
base. It pinned WHERE a declaration may appear -- only at the start of a line, never inside a
fenced code block -- and never pinned WHAT A DECLARATION IS. Four independent findings, one
cause:

- The four-arm field split on whether a marker followed by trailing prose on the same line
  declares anything. Two said yes, two said no, both self-consistent.
- A `strip_suffix` over a line carrying two markers captured the intermediate markup into the
  path: everything from the first path through the SECOND marker's word became one path.
- One implementation accepted a TAB after `fb:` while rejecting a tab before the marker word.
  Exactly one of those is wrong under any single grammar.
- One rejected a marker with no space after the comment opener, which is valid HTML.

This task rules all of it. **A ruling has already been made on the first**, in the adjudication
that merged the module: trailing text means the line is PROSE and declares nothing. The
remaining three are open and you are implementing the ruling below, not choosing it.

### Notation, and why this document never prints a marker

A declaration is an HTML comment: the opener `<`, `!`, `--`, the marker, the closer `--`, `>`.
Below, `OPEN` means the three-character comment opener and `CLOSE` means the two-character
closer followed by `>`. This document does not print either literally, because the FIRST LINE
of this file is a real declaration and the dispatcher scans the whole prompt. A previous spec
printed its own examples and every arm refused the task. Line 1 is the only literal marker
here; read it for the exact shape.

## The grammar, pinned

A line declares if and only if, after removing leading whitespace, it matches EXACTLY:

    OPEN , one or more spaces , "fb:" , WORD , one or more spaces , PATH , one or more spaces , CLOSE

and nothing follows CLOSE except whitespace.

- `WORD` is `creates` or `modifies` and nothing else.
- `PATH` is one or more characters containing no whitespace and not containing CLOSE.
- Every `one or more spaces` is SPACES ONLY. A tab is not a space here.

## Exact API

Keep every existing signature. Add:

```rust
/// Why a line that looks like a declaration is not one.
///
/// Reported so a spec author sees the difference between "you wrote no marker" and
/// "you wrote a marker I refused", which are otherwise indistinguishable from
/// [`Precondition::Undeclared`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// A tab appeared where the grammar requires spaces.
    Tab,
    /// Text other than whitespace followed the closing delimiter.
    TrailingText,
    /// The marker word was neither `creates` nor `modifies`.
    UnknownWord,
    /// No whitespace between the comment opener and `fb:`.
    NoSpaceAfterOpener,
    /// The line carries more than one marker.
    MultipleMarkers,
}

/// Lines that look like declarations and were refused, with the line number
/// (1-based) and the reason. In line order.
pub fn rejections(prompt: &str) -> Vec<(u32, Rejected)>;
```

`declarations` and `check` keep their signatures and their meaning, and now follow the grammar
above exactly.

## Falsifiable clauses

1. A line that is exactly a well-formed marker, with any amount of leading whitespace, declares.
2. **Trailing text after CLOSE declares nothing**, and `rejections` reports `TrailingText`.
   Trailing WHITESPACE is not trailing text: it declares normally and is not rejected.
3. **A tab anywhere the grammar says spaces declares nothing**, and reports `Tab`. Pin a tab in
   each of the three positions separately: after the opener, after `fb:WORD`, and before CLOSE.
4. **No whitespace between the opener and `fb:` declares nothing**, and reports
   `NoSpaceAfterOpener`. This is valid HTML and is still refused: the space is what separates a
   declaration from ordinary markup, and admitting the tight form widens the surface a prose
   example can hit.
5. **A line carrying two markers declares NOTHING AT ALL** -- not the first, not both -- and
   reports `MultipleMarkers` once. Say in the doc why: the path is delimited by CLOSE, so on a
   two-marker line every reading of "the path" is a guess, and a guessed path is exactly what
   this module exists to refuse.
6. `fb:` followed by a word that is neither `creates` nor `modifies` declares nothing and
   reports `UnknownWord`. It is NOT an error: an older binary meeting a newer prompt must not
   refuse it.
7. **`rejections` and `declarations` never name the same line.** Pin this over a prompt mixing
   good and bad lines: a line is one or the other, never both and never neither-when-it-looked-
   like-a-marker.
8. A line with no OPEN at all is neither declared nor rejected. Silence is for text that never
   looked like a marker.
9. Clause 6 and clause 7 of the ORIGINAL spec still hold and must not regress: a marker not at
   line start declares nothing, and a marker inside a fenced code block declares nothing.
   **A rejected-looking line inside a fence is not reported by `rejections` either** -- a fence
   means "this is documentation", and reporting a refusal there would flag every spec that
   documents the syntax.
10. **A tilde fence (`~~~`) opens and closes a fenced block exactly as three backticks do.**
    The original spec named backticks only. This pipeline's own prompts use fenced blocks, and
    a tilde-fenced example containing realistic markers would otherwise be read as live
    declarations -- which is the failure that refused a whole wave, one delimiter over. A fence
    opened with backticks is closed only by backticks, and one opened with tildes only by
    tildes.

## Boundaries, at N and at zero

- An EMPTY prompt: `declarations` empty, `rejections` empty, `check` is `Undeclared`.
- A prompt whose ONLY marker-shaped lines are all rejected: `check` is `Undeclared`, not
  `Satisfied` and not `Violated`. Nothing was declared, so nothing was checked. Pin this: it is
  the case where a spec author typed a tab and would otherwise be told everything is fine.
- A line that is exactly OPEN and CLOSE with nothing between: not a marker, not a rejection.
- A PATH containing CLOSE is impossible by the grammar; pin that such a line rejects rather
  than truncating the path at the first CLOSE.
- A fence opened and never closed: everything after it is inside the fence, to end of prompt.
- Exactly ONE space in each position is the minimum; two or more are equally valid. Pin both.

## Superset status on every enumerated list

`Rejected` is a CLOSED set of five. `WORD` is a CLOSED set of two. The FENCE DELIMITERS are a
closed set of two, backticks and tildes, each at three or more characters. The set of PATHS is
open and nothing may key off known file names.

## Composition of aggregate returns

`rejections` returns `(line_number, reason)` in ascending line order, one entry per rejected
line, with 1-based line numbers. A line with two independent faults reports the FIRST in the
order the enum declares, and you must say so. `declarations` keeps its existing contract:
document order, duplicates preserved.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O.
- Do not change the signature or meaning of `declarations`, `check`, `Declared`, `Requirement`,
  `Precondition` or `Violation`. Extend; do not reshape.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
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
