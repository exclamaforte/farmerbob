<!-- fb:creates crates/farmerbob-core/src/rubric_carry.rs -->
# Task: two rules were added to the rubric this week and reached no arm at all

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/rubric_carry.rs`. Declare it in `lib.rs` with one
`pub mod rubric_carry;` line and change nothing else.

## Why this exists

Nothing reads `.fb/prompts/_rubric.md` at dispatch time. Every task spec carries its own COPY of
the rubric, appended when the spec was written. So a rule added to `_rubric.md` afterwards
reaches no arm: two were added last week -- the one about suites being run against other
implementations, and the one about not quoting a field the spec calls prose -- and both had been
added precisely because the defect had recurred. Both recurred again afterwards, against arms
that were never shown the rule.

That is farmerbob-vg0 turned inside out: disclose what is scored, into a file the arms never
see.

`fb-speclint.sh` now greps for this, comparing a spec's tail against the canonical file. The
comparison is the interesting part and it is three lines of shell. It has to tell apart three
states that a naive "does the spec contain the rubric" check folds into one: no rubric at all,
a STALE rubric, and a rubric that belongs to a DIFFERENT document. Only the second is fixable by
re-appending.

## Exact API

```rust
/// How a spec stands against the canonical rubric.
pub enum Carry {
    /// The spec ends with the canonical rubric, byte for byte after normalisation.
    Current,
    /// The spec carries an OLDER rubric: every line it has is a line the canonical
    /// rubric still has, in order, but the canonical rubric has lines it does not.
    /// Carries the canonical lines that are missing, in canonical order.
    Stale(Vec<String>),
    /// The spec carries text where the rubric belongs, and it is not a prefix of
    /// the canonical rubric. Re-appending will not fix it.
    Foreign,
    /// The spec carries no recognisable rubric at all.
    Absent,
}

/// Compare one spec against the canonical rubric.
///
/// `anchor` is a line that begins the rubric in both documents; text before the first
/// occurrence of `anchor` in `spec` is the spec's own body and is not compared.
pub fn carry(spec: &str, rubric: &str, anchor: &str) -> Carry;

/// The lines of `rubric` that `spec` does not carry, in canonical order.
///
/// Empty when the spec is `Current`. Defined for every `Carry`.
pub fn missing_lines(spec: &str, rubric: &str, anchor: &str) -> Vec<String>;
```

## Normalisation, pinned

Comparison is line by line. Before comparing, each line has its TRAILING whitespace removed, and
lines that are empty after that are dropped from BOTH sides. Nothing else is normalised: leading
whitespace is significant, case is significant, and no line is reflowed. This is pinned because
it is not the arm's to choose.

## Falsifiable clauses

1. A spec whose text after the anchor equals the rubric is `Current`.
2. A spec whose text after the anchor equals the rubric with trailing spaces added to some lines
   and blank lines inserted is ALSO `Current`. This is the normalisation clause and it must be
   pinned, or the check fires on every spec and is read by nobody.
3. A spec carrying the rubric's first K lines, for K less than the canonical line count, is
   `Stale`, carrying exactly the canonical lines from K onward, in order.
4. A spec carrying the rubric with one line CHANGED in the middle is `Foreign`, not `Stale`.
   A changed line is not a missing line, and re-appending cannot fix it. Pin clauses 3 and 4 as
   a pair against the same rubric: an implementation that computes a set difference calls both
   of them `Stale`, which is exactly the answer that sends someone to re-append and watch
   nothing change.
5. A spec containing the anchor followed by unrelated prose is `Foreign`.
6. A spec not containing the anchor at all is `Absent`.
7. `missing_lines` agrees with `carry`: empty for `Current`, equal to the payload for `Stale`.
   Pin the agreement, not just each function.
8. Only the FIRST occurrence of the anchor in the spec starts the comparison. Pin a spec that
   quotes the anchor line in its body and then carries the real rubric below it: the comparison
   begins at the quoted line, so the body text between the two occurrences is compared as if it
   were rubric, and the result is `Foreign`. That is the rule this spec chooses, deliberately,
   over a last-occurrence rule which fails differently and worse. Assert `Foreign` for that
   input, and name the limitation in your handoff.

## Boundaries, at N and at zero

- An EMPTY canonical rubric: every spec containing the anchor is `Current`, because there is
  nothing to be missing. Pin it; it is the degenerate case and the obvious implementation
  (compare lengths) gets it wrong.
- An EMPTY spec: `Absent`.
- A spec carrying ZERO rubric lines after the anchor -- the anchor is the last line: `Stale`
  with the whole rubric missing, not `Absent`. The anchor was found; that is the difference.
- A rubric of exactly ONE line, carried: `Current`. Not carried: `Stale` with that one line.
- A spec carrying the rubric TWICE: not pinned. Say which answer you return and do not assert
  on it.
- An empty `anchor` string: not pinned. Do not assert on it.

## Superset status on every enumerated list

`Carry` has EXACTLY four variants. Closed set. The four are mutually exclusive and one of them
always applies: there is no fifth "cannot tell".

The rubric's CONTENT is not this module's business. It does not know what a rubric says, does not
look for headings, and a `const` containing any part of the rubric text is a defect -- the whole
point is that the canonical text lives in one file and is passed in.

## Composition of aggregate returns

`Stale` carries lines in CANONICAL order, not spec order, not sorted, not deduplicated. If the
canonical rubric repeats a line, the repeat is carried as a separate element. Pin the order with
a rubric of at least four lines of which the last three are missing.

`missing_lines` is defined for `Foreign` and `Absent` too. The spec does NOT pin what it returns
in those two cases -- say what you chose in your handoff, and your tests may not assert on it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No filesystem access: both documents arrive as `&str`.
- `cargo test -p farmerbob-core` and `cargo clippy -p farmerbob-core -- -D warnings` must pass.
  Both are clean on HEAD as of this task, so any failure is yours.

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
