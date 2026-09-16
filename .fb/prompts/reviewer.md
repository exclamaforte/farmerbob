<!-- fb:creates crates/farmerbob-core/src/reviewer.rs -->
# Task: reviewer roles that produce evidence, not ratings

Create `crates/farmerbob-core/src/reviewer.rs`, declare it from `lib.rs` with
`pub mod reviewer;`. Change nothing else except that one `pub mod` line.

## Context

A cheap model asked to "rate this patch out of ten" produces a number that correlates with
prose fluency and nothing else. Asked instead to name a concrete input on which the patch
misbehaves, it produces something the harness can execute — and execution, not opinion, is
what reaches the score.

So reviewers occupy narrow roles, each emitting one kind of artefact:

- **Prosecutor** — finds a falsifiable failure. Its output is a candidate test.
- **Defender** — finds the strongest reading under which the patch is correct. Its output is
  a spec clause reference, which is how an over-fitted accusation gets vetoed.
- **Cartographer** — finds what the spec does not determine. Its output is an ambiguity
  report, which is worth more than either of the above, because it routes back to the task
  author instead of becoming a defect argued about later.

A reviewer that emits the wrong artefact for its role is not partially useful; it is noise,
and the type system should make that a rejection rather than a judgement call.

**Pure logic: no I/O, no model calls.**

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { Prosecutor, Defender, Cartographer }

/// What a reviewer emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artefact {
    /// A falsifiable failure: an input, the spec's expectation, the alleged behaviour.
    Accusation { input: String, expect: String, actual: String },
    /// A spec clause said to license the behaviour under attack.
    Defence { clause: String, rationale: String },
    /// A question the spec does not answer, with the readings it permits.
    /// At least two readings, or it is not an ambiguity.
    Ambiguity { question: String, readings: Vec<String> },
    /// An opinion. Always recorded, never scored, never executed.
    Opinion { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission { pub reviewer: String, pub role: Role, pub artefact: Artefact }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// The artefact does not match the role that emitted it.
    WrongArtefactForRole { role: Role },
    /// An accusation whose expect and actual are the same alleges nothing.
    NotFalsifiable,
    /// A required field is empty or whitespace only.
    Empty { field: String },
    /// An ambiguity offering fewer than two readings.
    NotAmbiguous,
    /// Contains a rating. Exactly these forms are rejected and no others:
    /// a digit followed by "/10", or any of the words "score", "rating", "grade".
    /// Matching is case-insensitive.
    Rated,
}

/// Accept what is executable evidence; reject the rest with a reason.
/// Order is preserved and one rejection never discards the batch.
pub fn triage(subs: &[Submission]) -> (Vec<&Submission>, Vec<(usize, Rejected)>);

/// Accusations that no Defence cites a clause against, i.e. the ones still standing.
/// Compared by the accusation's `input` and `expect`, ignoring case and outer whitespace.
pub fn unrebutted(subs: &[Submission]) -> Vec<&Submission>;

/// Every distinct question raised by a Cartographer, deduplicated case-insensitively
/// and returned sorted. This is the highest-value output of a review round.
pub fn open_questions(subs: &[Submission]) -> Vec<String>;
```

## Required behaviour

- A Prosecutor may emit only `Accusation` or `Opinion`; a Defender only `Defence` or
  `Opinion`; a Cartographer only `Ambiguity` or `Opinion`. Anything else is
  `WrongArtefactForRole`. `Opinion` is always permitted from every role.
- Check the role match BEFORE the artefact's own validity, so a Defender submitting a
  malformed accusation is told the role is wrong rather than that its accusation is broken.
- An `Accusation` whose `expect` equals `actual` after trimming and case-folding is
  `NotFalsifiable`.
- Any empty or whitespace-only required field is `Empty` naming that field.
- An `Ambiguity` with fewer than two readings is `NotAmbiguous`. Two identical readings
  count as one.
- `Rated` is checked on every variant's text, including `Opinion`. A reviewer must not
  route a score through prose.
- `unrebutted` returns accusations in input order and treats a `Defence` as rebutting an
  accusation when its `clause` is non-empty and its `rationale` quotes the accusation's
  `input`.
- Rejections carry the index into the input slice.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a Defender emitting an Accusation is
WrongArtefactForRole and NOT NotFalsifiable; every role may emit an Opinion; "8/10" and
"SCORE: high" are both Rated; two identical readings are NotAmbiguous; an accusation whose
expect equals actual is rejected; a Defence quoting the input rebuts; `open_questions` is
deduplicated and sorted; one bad submission does not discard the batch.

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
4. **Test depth and generality** — number of distinct behaviours covered, not number of
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
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
because a test asserted a case the specification never fixed.

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
