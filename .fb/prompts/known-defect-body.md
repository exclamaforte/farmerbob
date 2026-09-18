<!-- fb:creates crates/farmerbob-core/src/known_defect.rs -->
# Task: the record says a defect exists and not what it is

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/known_defect.rs` and add `pub mod known_defect;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

A confirmed finding about the arm that WON can no longer be escalated -- the test fails against
the merged code, because the defect is real and unfixed -- so it is now recorded instead of
discarded. Four such findings were recovered this morning. Each entry reads:

    ## 2026-09-18T11:08:04-07:00 -- or-luna-pro on or-muse-spark
    or-luna-pro on or-muse-spark: FILE-AS-KNOWN-DEFECT -- test fails against merged code

    The escalated test failed against the merged reference, so it was not
    added to the suite. The finding was confirmed and is recorded here so
    it survives the retraction.

It says a defect exists, who found it, and nothing about what is wrong. The critic's own claim
-- the WHERE, the TRIGGER, the EXPECT and the ACTUAL -- is sitting in
`<logs>/<task>.claims.json` and is not carried across. A record that cannot tell a reader what
to fix has kept the provenance and lost the finding.

## Exact API

```rust
/// One confirmed finding that could not be escalated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownDefect {
    /// The arm that found it.
    pub critic: String,
    /// The arm it is about -- the one that was merged.
    pub subject: String,
    /// The claim's one-line statement, as the critic wrote it.
    pub claim: String,
    /// The critic's own WHERE / TRIGGER / EXPECT / ACTUAL lines, in that
    /// order, each without its label. A line the critic did not write is
    /// absent from the vector rather than present and empty.
    pub evidence: Vec<String>,
}

/// Render one entry for `.fb/known-defects/<task>.md`.
///
/// `stamp` is the caller's timestamp; this module has no clock.
pub fn render(task: &str, stamp: &str, d: &KnownDefect) -> String;

/// Parse the entries already in such a file, so a caller can tell a new
/// finding from one already recorded.
pub fn parse(file: &str) -> Vec<KnownDefect>;

/// Whether this defect is already present in `existing`.
///
/// Two entries are the same when critic, subject and claim all match
/// exactly. The evidence is not compared: a re-run that gathers more of it
/// is the same finding, and treating it as new would fill the file with
/// duplicates.
pub fn already_recorded(existing: &[KnownDefect], d: &KnownDefect) -> bool;
```

## Falsifiable clauses

1. `render` emits a markdown `## ` heading carrying the stamp, the critic and the subject, then
   the claim, then each evidence line. Pin the heading's exact shape, since `parse` must read
   what `render` writes.
2. **`render` then `parse` round-trips**: parsing a rendered entry yields an equal
   `KnownDefect`. Pin it as a property over an entry with evidence and one without.
3. A `KnownDefect` with EMPTY evidence renders without an evidence section and parses back with
   an empty vector. Absent evidence is not an empty line.
4. `parse` on a file with several entries returns them in file order.
5. `parse` on a file with no `## ` heading returns an empty vector, not an error. A file that is
   not this format has no entries; it is not a failure.
6. `already_recorded` compares critic, subject and claim EXACTLY and ignores evidence. Pin a
   case where the evidence differs and the answer is still true.
7. **A claim containing a backtick, a quote, a newline or a `##` at line start survives the
   round trip.** The critics' claims in this repository are full of code spans, and a `##`
   inside a claim must not be read back as a new entry. Say how you prevent it.

## Boundaries, at N and at zero

- An EMPTY file: `parse` returns an empty vector.
- One entry with no evidence: clause 3.
- `already_recorded` against an empty slice: false.
- An entry whose claim is the empty string: renders and parses back with an empty claim. It is
  degenerate, not invalid; say so rather than rejecting it.
- Two entries whose critic and subject match but whose claims differ: both are recorded, and
  `already_recorded` is false for the second.
- A stamp containing a `--`: the heading separator is `--`, so pin that a stamp carrying one
  still parses. This repository's stamps are RFC3339 and safe today; the next format may not be.

## Superset status on every enumerated list

The EVIDENCE LABELS are a CLOSED set of exactly four, in order: `WHERE`, `TRIGGER`, `EXPECT`,
`ACTUAL`. They are the four the critique contract already fixes. A critique line carrying any
other label is not evidence and is ignored -- say so, and say that ignoring beats guessing,
because a mis-parsed label would attribute one field's text to another.

## Composition of aggregate returns

`evidence` holds only the labels the critic actually wrote, in the fixed order above, never
padded to four. `parse` returns entries in file order. Say why `render` and `parse` must be
tested together rather than separately: each is only correct with respect to the other, and a
matched pair of bugs would pass two independent tests.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O, no clock. The file arrives as a `&str` and the stamp as an
  argument.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.

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
