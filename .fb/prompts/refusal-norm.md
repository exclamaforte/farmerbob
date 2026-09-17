<!-- fb:modifies crates/farmerbob-core/src/limit_signal.rs -->
# Task: make provider-refusal detection survive colour and casing

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/limit_signal.rs`. Do not change any other file.

`crates/fb/src/objective.rs` holds a second, independent copy of this classification, already
repaired for the same defect. It is present in your worktree -- read it for reference, do not
edit it. This task is the version that belongs in core, where every future caller inherits it.

## The incident this comes from

On 2026-09-17 an OpenRouter key hit its spend cap. Thirteen arms were refused before they ran.
Three of them were dispatched that morning and died in 4, 9 and 75 seconds, and the harness
recorded every one as **NO-OP: the model produced nothing**. The models had never been asked.

`limit_signal::classify` exists to prevent exactly that, and its caller registered the right
pattern. It missed anyway. The log said:

```
\x1b[91m\x1b[1mError: \x1b[0mRate limit exceeded: free-models-per-day-high-balance.
```

The launcher colourises, and the reset sequence sits **between** the prefix and the message, so
the literal bytes are `Error: \x1b[0mRate limit exceeded`. Measured, on the real bytes:

| pattern                       | result          |
|-------------------------------|-----------------|
| `"rate limit exceeded"`       | Limited, caught |
| `"error: rate limit exceeded"`| Normal, MISSED  |

The anchored form is the one that misses. That is the worst possible shape for this defect,
because anchoring is the *correct* practice here: an agent discussing quotas in a task about
quotas must not read as one being refused, and `error: ` is what separates the two. The safe
choice is the one that breaks.

A refusal misread as a NO-OP is not a cosmetic loss. It is written into the bandit's posterior as
evidence that a model failed, and it is the reason this project keeps insisting that "measured
zero" and "not measured" must not be the same value.

## Exact API

Keep every existing public item working. `classify`, `parse_reset`, `SignalRules`,
`Classification` and their current signatures must not change shape — `crates/fb` calls them.

Add exactly this, public:

```rust
/// Strip ANSI escape sequences from `s`, returning owned text.
///
/// Removes CSI sequences (`ESC [` … final byte in `0x40..=0x7E`) and two-character
/// `ESC <byte>` sequences. Text containing no escapes is returned unchanged.
pub fn strip_ansi(s: &str) -> String;

/// The text `classify` actually matches against, exposed so a caller can see what was searched.
///
/// Currently: ANSI-stripped. Casing is NOT folded here — matching is already
/// case-insensitive and folding twice would be a second place to get it wrong.
pub fn normalise(s: &str) -> String;
```

`classify` must match its patterns and its exclusions against `normalise(output)`.

`Classification::Limited`/`Ambiguous` carry an `evidence` string. **`evidence` must be the
NORMALISED text, not the raw text.** A human reading a report should not be shown escape codes,
and an evidence string that still contains them cannot be compared against a pattern by anyone
reading the record later. Say so in a test.

## Falsifiable clauses

Each of these is a test the adjudicator will look for. State them as tests, not as prose.

1. `strip_ansi("\x1b[91m\x1b[1mError: \x1b[0mRate limit exceeded")` == `"Error: Rate limit exceeded"`.
2. `strip_ansi` on text with no escapes returns it unchanged, byte for byte.
3. `classify` with pattern `"error: rate limit exceeded"` against the VERBATIM colourised bytes
   above returns `Limited`. This is the regression; it currently returns `Normal`.
4. `classify` with the unanchored pattern `"rate limit exceeded"` still returns `Limited`
   against the same input. The fix must not trade one form for the other.
5. An exclusion pattern is matched against normalised text too: an exclusion that would fire on
   plain text still fires when the text is colourised.
6. `parse_reset` finds a reset instant in colourised text. It reads the same output and had the
   same exposure; if you leave it reading raw text, say why in a comment.
7. `evidence` on a `Limited` from colourised input contains no `\x1b`.

## Boundaries, at N and at zero

- **Zero patterns.** `SignalRules` with no patterns and no exit codes classifies everything
  `Normal`. It must NOT classify everything `Limited`, and it must not panic.
- **Empty output.** `classify(rules, 1, "", now)` with a limit exit code registered returns
  `Limited` with evidence `"exit code 1"` — the existing behaviour. Keep it.
- **Escape at the very end.** `strip_ansi("abc\x1b")` and `strip_ansi("abc\x1b[")` must return
  `"abc"` and must not panic or loop. A truncated escape at end-of-input is a real case: logs
  get cut off mid-sequence.
- **Escape only.** `strip_ansi("\x1b[0m")` == `""`.
- **N patterns.** With several patterns matching, `first_match_text` already picks one. Do not
  change which one it picks; pin the existing choice in a test so the next person cannot change
  it by accident.

## Superset status on every enumerated list

This matters more here than anywhere else in the codebase, so read it twice.

The set of ways a provider can say no is **open**. It is not enumerable and it will grow. This
project has already had to learn three distinct refusal strings, each time by discovering that
runs had been scored as arm failures for some unknown period.

So: wherever you enumerate, the enumeration must be marked as a **known subset, not a closed
set**, in the type and in the docs — not only in a comment. `Classification` must not imply that
`Normal` means "confirmed to be a genuine run". It means "no refusal marker was recognised",
which is a weaker statement, and the doc comment must say exactly that.

Do not add new patterns to any list. Adding patterns is what was tried before and it does not
address the shape of the problem. Your job is that the patterns callers already supply are
matched against text that can actually contain them.

## Composition of aggregate returns

If you add any function returning a collection or a summary over several inputs, state in its
doc comment what it does when the input is empty and whether the result is ordered, and pin
both in tests. An aggregate whose empty case is unstated is how "nothing matched" and "nothing
was examined" become the same value.

## Use the crate's own types

`farmerbob_core::measurement::Measurement<T>` is `Observed(T)` or `Missing(Absent)`, with no
`unwrap`, no `unwrap_or`, no `Default` and no `From<Option>`. If you introduce any function that
can fail to determine something, return `Measurement`, not a bare value with a sentinel. Do not
change existing signatures to do this.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Add no dependencies. `strip_ansi` is a small character-state loop; write it.
- `cargo build` and `cargo test` must pass for the whole workspace. Run them yourself.
- Do not make parameters generic. Concrete types only.
- Do not delete or weaken an existing test to make a new one pass. If an existing test encodes
  the bug, change it and say in the commit body which one and why.

## Do not fabricate

If some part of this cannot be done in your environment, say so plainly and leave it undone with
a comment explaining what you could not verify. Returning less with a stated reason is a correct
answer here and is scored as one. A previous submission in this series shipped a file that
emitted its script's exact output format with the numbers hardcoded, and passed build, scope,
lint and its own tests, because every other gate in this harness is a gate on form. This one is
checked by reading it.

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
