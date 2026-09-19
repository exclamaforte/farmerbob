<!-- fb:creates crates/farmerbob-core/src/speclint_shape.rs -->
# Task: a check that only recognises the sentences it has already seen

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/speclint_shape.rs`. Declare it in `lib.rs` with one
`pub mod speclint_shape;` line and change nothing else.

## Why this exists

A task spec that asks the IMPLEMENTER to decide something, and then asks them to pin their own
decision, produces N implementations that pin N different decisions. The cross-examination
matrix then comes back with every off-diagonal cell failing, and the field disagreed about
nothing except a value the specification declined to fix.

`fb-speclint.sh` greps for this. Its first version matched a LITERAL — `state (what|which|
whether) ... and pin it` — which caught the exact four sentences that had already caused damage
and nothing else. The fifth spelling walked straight through: `field-shape` shipped "State what
you do with it and pin the answer you chose", eight words from the banned phrase, clean per
speclint, and two arms returned it as a delegation defect before either had written a line of
code. A check that only recognises the sentences it has already seen is a check that only works
on defects that have stopped happening.

It is now SHAPE-based, and the shape is the whole of the defect: a second-person decision verb
followed, in the same sentence, by an instruction to pin.

A second rule sits beside it. Asking the arm to pin a FORMAT the spec did not fix is the same
defect wearing different clothes: `known-defect-body` said "Pin the heading's exact shape, since
`parse` must read what `render` writes", three arms pinned three different claim-line shapes,
and every off-diagonal cell failed because clauses 4, 5 and 7 all required `parse` to be tested
on hand-written text — and hand-written text can only be written in one arm's format.

"Pin both sides of that boundary" and "pin it as a property" are FINE: they ask for a test of a
value the spec has already fixed. What is not fine is asking the arm to fix the value.

This logic is in a shell function built from two `grep -nEi` invocations. It is untested, and
the regexes are the only statement of the rule.

## Exact API

```rust
/// A lint a spec's text triggers.
pub struct Lint {
    /// 1-based line number in the text as given.
    pub line: usize,
    /// Which rule fired.
    pub rule: Rule,
    /// The offending sentence, trimmed of leading and trailing whitespace.
    pub sentence: String,
}

/// The rules this module checks.
pub enum Rule {
    /// A second-person decision verb and an instruction to pin, in one sentence.
    /// The spec is delegating a choice and then asking the arm to fix it.
    DelegatedDecision,
    /// An instruction to pin a FORMAT — a shape, wording, ordering or separator —
    /// that the specification has not itself fixed.
    DelegatedFormat,
}

/// Lint one spec's text.
///
/// Fenced code blocks are skipped: a spec quoting an offending sentence inside
/// ``` fences is showing an example, not issuing an instruction.
pub fn lint(text: &str) -> Vec<Lint>;

/// The decision verbs that trigger [`Rule::DelegatedDecision`].
///
/// EXACTLY these and no others: "state", "say", "choose", "decide", "resolve",
/// "declare", "determine". Matching is case-insensitive and on whole words.
pub fn decision_verbs() -> &'static [&'static str];

/// The format nouns that trigger [`Rule::DelegatedFormat`].
///
/// EXACTLY these and no others: "shape", "format", "layout", "wording",
/// "spelling", "ordering", "order", "separator", "delimiter", "prefix",
/// "encoding". Case-insensitive, whole words.
pub fn format_nouns() -> &'static [&'static str];
```

## What a sentence is, pinned

A sentence ends at `.`, `!` or `?`, or at the end of the text. A newline does NOT end a
sentence: the rule is about a verb and a pin appearing together in one instruction, and specs
are hard-wrapped at 100 columns, so a rule that stopped at newlines would miss most of them.
This is pinned because it is not the arm's to choose.

A `Lint`'s `line` is the line on which the sentence STARTS.

## Falsifiable clauses

1. "State what you do with it and pin the answer you chose" fires `DelegatedDecision`. This is
   the exact sentence that walked through the literal check; pin it verbatim.
2. "State (what|which|whether) ... and pin it" fires — the original literal. Pin at least one.
3. A decision verb with NO "pin" in the same sentence does not fire. "State your assumption in
   the handoff." is clean.
4. "pin" with no decision verb in the same sentence does not fire. "Pin both sides of that
   boundary." is clean, and so is "Pin it as a property."
5. A decision verb in one sentence and "pin" in the NEXT does not fire. Pin this explicitly: it
   is the boundary between the rule and a false positive, and a naive implementation scanning
   the whole text fires on every spec ever written.
6. A decision verb and "pin" separated by a NEWLINE but inside one sentence DOES fire. Clauses
   5 and 6 must be tested as a pair: they differ only in whether a `.` intervenes.
7. "Pin the heading's exact shape" fires `DelegatedFormat`.
8. "Pin both sides of that boundary" does not fire `DelegatedFormat`. "Boundary" is not a format
   noun.
9. A sentence inside a ``` fenced block fires nothing, under either rule.
10. A sentence that triggers BOTH rules yields TWO `Lint`s for that line, one per rule, and the
    order between them is not pinned — say in your handoff which you emit first and do not
    assert on it.
11. Matching is case-insensitive: "STATE ... PIN IT" fires.
12. Matching is on whole words: "restated" does not contain the verb "state" for this purpose,
    and "reorder" does not contain "order". Pin one of each.

## Boundaries, at N and at zero

- EMPTY text: no lints.
- A text with no sentence terminator at all: the whole text is ONE sentence, and the rule
  applies to it. Pin it.
- A sentence containing the verb twice and "pin" once: ONE lint, not two. The lint is per
  sentence per rule, not per match.
- An unterminated fenced block — ``` opened and never closed: everything after it is inside the
  fence. Pin this; it is the degenerate case and a naive toggle gets it right only by accident.
- A line that is exactly ``` with nothing else: opens or closes a fence. Nested fences are NOT
  pinned — say in your handoff what you did and do not assert on it.

## Superset status on every enumerated list

`decision_verbs()` and `format_nouns()` are EXACTLY the lists given, and no others. These are
closed sets, and your tests MAY assert that a word outside them does not fire — that is the
point of pinning them as closed. `Rule` has exactly two variants.

Morphological variants are NOT included: "stating", "states", "decided" do not fire. The rule
is the bare verb, whole-word, case-insensitive. This is pinned; do not stem.

## Composition of aggregate returns

`lint` returns every lint in the text, ordered by `line` ascending. Two lints on the same line
keep the order clause 10 leaves unpinned. The vector is not deduplicated across lines: the same
sentence appearing twice in a spec is two lints.

A `Lint`'s `sentence` is the offending sentence trimmed of leading and trailing whitespace, with
its interior — including newlines — left exactly as it was. Do not reflow it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. In particular do NOT add `regex`: this crate does not depend on it and a
  new dependency is a scope violation. Hand-written scanning is expected.
- No filesystem access: the text arrives as `&str`.
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
