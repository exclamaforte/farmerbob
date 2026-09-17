<!-- fb:modifies crates/farmerbob-core/src/promote.rs -->
# Task: refuse a claim that is false about the file it names

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/promote.rs`. Do not change any other file.

Read the module first. It already decides which critique assertions earn execution, it performs
no I/O, and it never runs a test. You are extending that decision, and the no-I/O property is
part of the contract: every function you add takes source text as an argument and never opens a
file.

## Why

`promote` currently rejects a claim for being incomplete, unfalsifiable, a judgement, or a
duplicate. All four are properties of the claim in isolation. Nothing compares a claim against
the code it is about, so a claim can be flatly false and still be promoted to a test.

Three real incidents, all from 2026-09-17, all from critiques this harness produced:

**FABRICATION.** A critic filed two claims against `liveness.rs` quoting a return value verbatim:

    Cut::Keep { because: "output is too weak to justify ending a run" }

That string occurs zero times in the subject. The cited lines held a doc comment and two closing
braces. There is no such code path in the file. Measured behaviour was the opposite of the claim.

**PROJECTION.** A critic filed a claim against a rival's `freeze_suite`, quoting:

    line.replace("use super::", "use crate::")

That line is absent from the subject, which writes the correct form with the module name
interpolated. It is present, verbatim, in the CRITIC'S OWN file. It filed its own bug against
someone else.

**FALSE-ABSENCE.** A critic filed four claims alleging a timeout and a JSON emitter were missing
from an 1892-line file that contains both. Every line it cited fell in the first sixth.

The first two are decidable from text alone, and that is what this task builds. The third needs
more than this module has and is out of scope.

## Exact API

Keep every existing public item working unchanged: `Assertion`, `Rejection`, `PromotedTest`,
`promote`, `fingerprint`, `objective_leaks`.

Add two variants to `Rejection` and one function:

```rust
pub enum Rejection {
    // ... existing variants unchanged ...
    /// The claim's `actual` quotes text that appears in NEITHER the subject nor the
    /// critic's own source. The evidence does not exist anywhere.
    Fabricated {
        /// The first quoted fragment that could not be found, verbatim.
        quote: String,
    },
    /// The claim's `actual` quotes text absent from the subject but PRESENT in the
    /// critic's own source: the critic described its own code and named someone else.
    Projected {
        /// The first quoted fragment found in the critic and not the subject.
        quote: String,
    },
}

/// Check a claim against the source it is about, and the source of whoever filed it.
///
/// Returns `None` when the claim cannot be refuted this way, which is NOT the same as
/// the claim being true.
pub fn verify_quotes(a: &Assertion, subject: &str, critic: &str) -> Option<Rejection>;
```

A quoted fragment is a run of text between two `"` characters in the claim's `actual` field.

## The clause that matters most

**A claim with no quoted fragment returns `None`.** An `actual` written entirely in prose cannot
be checked this way, and "I could not check it" must not become "I checked it and found nothing
wrong". Same for an EMPTY `subject`: if the subject source was not supplied, every claim returns
`None`, never `Fabricated`. A missing haystack is not evidence that the needle is absent.

That distinction is the one this project keeps getting wrong, and a verifier that gets it wrong
is worse than no verifier, because it would discard true claims about files it failed to read.

Say this in `verify_quotes`'s doc comment in those terms.

## Falsifiable clauses

State each as a test.

1. `actual` quotes a fragment present in `subject` -> `None`.
2. `actual` quotes a fragment in NEITHER subject nor critic -> `Fabricated`, carrying that
   fragment verbatim.
3. `actual` quotes a fragment absent from `subject` but present in `critic` -> `Projected`.
   `Projected` must win over `Fabricated`: it is strictly more informative, because it says
   where the defect actually lives.
4. `actual` with no `"` at all -> `None`.
5. `subject` empty, `actual` quoting anything -> `None`, never `Fabricated`.
6. `critic` empty, quote absent from subject -> `Fabricated` (nothing to project from).
7. `actual` with several quotes, the FIRST present and a LATER one absent -> rejected on the
   later one, carrying that later fragment. One bad quote is enough.
8. A `Judgement`, not a `Claim` -> `None`. Judgements are never executed, so there is nothing
   to refuse; `promote` already rejects them as `NotAClaim`.
9. A quoted fragment that is only whitespace, or empty (`""`) -> ignored, not `Fabricated`.
   An empty quote is punctuation, not evidence.
10. Matching is EXACT, byte for byte. Do not trim, normalise whitespace, or lowercase. Two
    spellings are two fragments. Pin a test where the quote differs from the source only by
    internal whitespace and is correctly reported as absent, and say in the doc comment that
    exactness is deliberate: a verifier that guesses at equivalence silently permits fabrication.

## Boundaries, at N and at zero

- Zero quotes: `None` (clause 4).
- One quote: the ordinary case, both directions.
- N quotes where ALL are present: `None`.
- An unterminated quote -- an odd number of `"` characters -- must not panic and must not treat
  the trailing run as a fragment. Pin it.
- A fragment containing a `"` cannot occur by construction; state in the doc comment that nested
  quoting is not supported and why that is acceptable here.

## Superset status on every enumerated list

`Rejection` is a CLOSED set, like `Verdict` and unlike the refusal-pattern lists: it enumerates
the reasons this module refuses a claim, every caller matches it exhaustively, and that is the
point of the type. Do not add a hedging doc comment suggesting otherwise.

But say plainly, in `verify_quotes`'s doc, that the set of ways a claim can be WRONG is open and
this function decides only two of them. A claim that survives `verify_quotes` has not been shown
to be true; it has only not been caught by this test. FALSE-ABSENCE -- a claim alleging something
is missing that the file contains -- is a third known shape that this function does NOT detect.

## Composition of aggregate returns

Add:

```rust
/// Verify many assertions against one subject and one critic, in input order.
///
/// Returns `(index, rejection)` for each assertion refused, in ascending index order.
pub fn verify_all(assertions: &[Assertion], subject: &str, critic: &str) -> Vec<(usize, Rejection)>;
```

Its doc comment must state what it returns for an EMPTY slice and that the result is ordered,
and both must be pinned by tests. An empty input returns an empty vector, which means "nothing
was examined" -- and an empty result from a NON-empty input means "nothing was refused". Those
are different facts reached by the same value, so the doc must say that the caller is
responsible for knowing which it asked for.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Add no dependencies. No regex; a character scan is enough.
- No I/O anywhere. Source text arrives as arguments.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
- Do not make parameters generic. Concrete types only.
- Do not weaken or delete an existing test to make a new one pass.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment saying what you could not verify. Returning less with a stated reason is a correct
answer here and is scored as one. Given what this task is about, a submission that invents
behaviour it did not implement would be a particularly poor showing.

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
