<!-- fb:creates crates/farmerbob-core/src/suite_match.rs -->
# Task: a verification that matched nothing reports that it passed

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/suite_match.rs` and add `pub mod suite_match;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`fb-escalate.sh verify <task>` runs the escalated conformance suite by filtering `cargo test` to
the module name. On a real escalated file it printed:

    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out

Zero tests ran and it reported `ok`. The file exists, contains a real module
(`escalated_testout_codex_luna`) and a real test; the filter matched none of it. The instrument
could not find what it was asked to check and said everything was fine.

That is this project's recurring bug, sitting inside the anti-invalid-test check -- the thing
whose entire job is to stop a bad test being trusted. It is one of three reasons that 303
findings across 265 critiques produced twelve permanent tests.

## Exact API

```rust
/// What a filtered `cargo test` run actually established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    /// Tests matched the filter and all of them passed.
    Passed {
        /// How many ran.
        ran: u32,
    },
    /// Tests matched and some failed.
    Failed {
        /// How many ran.
        ran: u32,
        /// How many failed.
        failed: u32,
    },
    /// The filter matched NOTHING. Not a pass: the instrument could not find
    /// what it was asked to check.
    MatchedNothing {
        /// How many tests existed but were filtered out, when the runner says.
        filtered_out: Option<u32>,
    },
    /// The output could not be read as a test result at all.
    Unreadable,
}

/// Read a filtered `cargo test` run's output.
pub fn read(output: &str) -> Verified;

/// Whether this result may be treated as the suite having been checked.
///
/// True ONLY for [`Verified::Passed`] and [`Verified::Failed`] -- both mean
/// tests ran. `MatchedNothing` and `Unreadable` mean nothing was established.
pub fn was_checked(v: &Verified) -> bool;
```

## Falsifiable clauses

1. `test result: ok. 3 passed; 0 failed; ...` is `Passed { ran: 3 }`.
2. `test result: FAILED. 2 passed; 1 failed; ...` is `Failed { ran: 3, failed: 1 }`. `ran` is
   passed plus failed; say so, because the runner never prints a total.
3. **`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out` is
   `MatchedNothing { filtered_out: Some(1440) }`, NOT `Passed { ran: 0 }`.** This is the clause
   the module exists for. Pin the exact line above.
4. The same line with no `filtered out` count is `MatchedNothing { filtered_out: None }`. An
   absent count is not zero.
5. **`0 passed; 0 failed` is `MatchedNothing` in every case**, with or without a
   `filtered out` clause. There is no `Passed { ran: 0 }`: a run in which nothing executed
   established nothing, and that is the whole point of the module. Pin both spellings.
6. `was_checked` is false for `MatchedNothing` and `Unreadable`. Pin all four variants.
7. Empty output is `Unreadable`, never `Passed`. Pin the empty string.
8. Output with several `test result:` lines -- a multi-crate run -- is read from the LAST one,
   because the filter applies to the final crate under test. Say so and pin it.

## Boundaries, at N and at zero

- `0 passed; 0 failed` WITH a non-zero `filtered out`: `MatchedNothing`, clause 3.
- `0 passed; 0 failed` with `0 filtered out`: also `MatchedNothing`, with
  `filtered_out: Some(0)`. A suite where nothing exists and nothing ran established nothing
  either. Pin it; it is the case a reader expects to differ and it does not.
- `1 passed; 0 failed`: `Passed { ran: 1 }`.
- `0 passed; 1 failed`: `Failed { ran: 1, failed: 1 }`.
- A `filtered out` count that does not parse as a number: `MatchedNothing { filtered_out: None }`
  rather than `Unreadable` -- the result line was read, only the count was not.
- Counts larger than `u32::MAX`: SATURATE at `u32::MAX`. Pinned, not delegated. A count that
  large is already nonsense and the variant it lands in is what matters, not the exact number;
  silently wrapping would turn a huge count into a small one and change the variant.

## Superset status on every enumerated list

`Verified` is a CLOSED set of four. The runner's OUTPUT FORMAT is an OPEN subset -- cargo's text
is not a stability guarantee -- so an unrecognised shape is `Unreadable` and never `Passed`. Say
both halves, and say that falling to `Unreadable` is the safe direction because it means
"nothing established" rather than "everything fine".

## Composition of aggregate returns

`Failed::ran` equals passed plus failed and is always at least `failed`; say why. `Passed::ran`
is at least one by construction, since zero is clause 3's case -- state that rather than
guarding against it.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O. The output arrives as a `&str`.
- `cell_record` and `testout` already read cargo output for OTHER questions -- whether a build
  failed, and which tests failed. This one answers a third: did anything run at all. Do not
  change either, and do not duplicate their parsing; if you find you need `failed_tests`, import
  it.
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
