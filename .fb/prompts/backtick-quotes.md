<!-- fb:modifies crates/farmerbob-core/src/promote.rs -->
# Task: look for evidence in the delimiter critics actually use

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/promote.rs`. Do not change any other file.

`verify_quotes` refuses a claim whose quoted evidence appears nowhere in the file it names
(`Fabricated`) or appears only in the critic's own file (`Projected`). It extracts candidate
fragments from text between DOUBLE QUOTES.

Critics in this harness write Markdown. They quote code in BACKTICKS. The detector is looking in
the wrong delimiter, and it has now missed two real projections because of it.

## The incident

pareto-tokens, 2026-09-17. codex-luna filed against glm-53-flash:

    ACTUAL: `Launcher::Unknown` (the code only examines `Source::launcher`, with a
            special-case fallback for the literal `codex-luna`).

That is a verbatim description of the CRITIC'S OWN implementation. The subject reads
`registry.cmd.get(arm)`, which is what its spec required. `fb promote pareto-tokens` returned
PROMOTED 2, REFUSED-BY-VERIFICATION 0: nothing was extracted, because every fragment is
backtick-delimited.

## Exact API

No signature changes. `verify_quotes` and `verify_all` keep their shapes, and `Rejection` gains
no variant. What changes is which fragments are considered:

> A quoted fragment is a run of text between two `"` characters, OR between two single backticks,
> in the claim's `actual` field.

## Falsifiable clauses

1. The incident: `actual` containing ``the code only examines `Source::launcher` `` , against a
   subject that does not contain that text and a critic that does, returns `Projected`.
2. A backtick fragment present in the subject returns `None`, exactly as a double-quoted one does.
3. Double-quoted fragments keep working unchanged. Every existing test in this module still
   passes; if one encodes the old behaviour, change it and say which and why.
4. **A triple-backtick fenced block is NOT a fragment.** Fenced blocks hold whole functions that
   legitimately appear in both files, and treating one as evidence would refuse true claims in
   bulk. Pin it with a fence containing text absent from the subject and assert `None`.
5. A single backtick inside a double-quoted fragment, or a double quote inside a backtick
   fragment, must not produce a fragment spanning the two delimiters. Pin both directions.
6. An unterminated backtick -- an odd count -- yields no fragment from the trailing run, and does
   not panic.
7. An empty backtick fragment ` `` ` is ignored, as an empty double-quoted one already is.
8. Matching stays EXACT, byte for byte. Do not trim a fragment's interior whitespace.

## Boundaries, at N and at zero

- Zero fragments of either kind: `None`, unchanged.
- Only backtick fragments, all present in the subject: `None`.
- Mixed: one double-quoted fragment present, one backtick fragment absent -> refused on the
  ABSENT one, carrying it verbatim.
- A fragment that is only whitespace, in either delimiter: ignored.
- Adjacent fragments with nothing between them: both extracted.

## Superset status on every enumerated list

The delimiters are a KNOWN SUBSET of how a critic may quote. Double quotes and single backticks
are what this harness's critics have actually used; a fragment marked some other way is simply
not extracted, which yields `None` and therefore no refusal. Say so, and say why that is the safe
direction: failing to extract costs a missed detection, while extracting something that is not
evidence would refuse a true claim.

`Rejection` stays CLOSED.

## Composition of aggregate returns

If you add a helper returning all fragments, its doc must state what it returns for input with no
fragments and whether the order is source order, both pinned. An empty list must not be readable
as "the claim had no evidence to check" by a caller that never looked.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no regex. A character scan is enough and matches the existing code.
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
