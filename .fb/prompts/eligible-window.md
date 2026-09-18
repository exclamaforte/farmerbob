<!-- fb:modifies crates/fb/src/sources.rs -->
# Task: the expiry check reports and does not enforce, and the Rust path never looks

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/sources.rs`. Do not change any other file.

## Why this exists

An arm can be enabled for a fixed span; `sources.toml` records that as an `expires_at` field
beside the arm's `status`. One such window ran **one hour and forty-seven minutes** past its end
with the arm still dispatching.

`farmerbob_core::window` now exists and answers the question -- it was merged this morning and
has no caller. `crates/fb/src/sources.rs` does not contain the string `expires_at` anywhere, so
`Registry::eligible` checks the status, the redundant-paid-route rule and nothing else, and
returns `Eligibility::Ok` for an arm whose window closed hours ago. The only thing that looks at
`expires_at` is a Python heredoc inside `fb-status.sh`, and by design it REPORTS rather than
enforces: it prints `STILL ENABLED -- DISABLE IT` and dispatches the arm anyway.

So the guard exists, the report exists, and the path that actually decides has never been
connected to either.

## Exact API

`Eligibility` gains exactly one variant, and `eligible` gains exactly one parameter:

```rust
use chrono::{DateTime, Utc};

pub enum Eligibility {
    Ok,
    Disabled(String),
    NotDispatchable(String),
    RedundantPaidRoute { free_arm: String, price_in: f64, price_out: f64 },
    /// The arm's time box has closed, or its `expires_at` cannot be read.
    /// Carries the reason a human should see, non-empty.
    WindowClosed {
        /// `expires_at` exactly as the registry holds it.
        expires_at: String,
        /// Why this is not dispatchable, in prose. Never empty.
        why: String,
    },
}

impl Registry {
    /// Eligibility INCLUDING the time box. `now` is the caller's clock; this
    /// module has none.
    pub fn eligible_at(&self, arm: &str, now: DateTime<Utc>) -> Eligibility;
}
```

`eligible(&self, arm: &str)` keeps its current signature and its current behaviour, and gains a
doc line saying in terms that it is WINDOW-BLIND and that `eligible_at` is the one that is not.
Do not change its signature and do not have it call a clock: a function that silently reads the
system time is how this module would acquire the clock the whole design says it does not have.
`Registry::dispatchable(&self)` also keeps its signature and keeps calling `eligible`.

Migrating the twelve callers to `eligible_at` is a SEPARATE task and is not yours. Say in your
handoff that `eligible` remains reachable and window-blind.

`Source` gains the field it is missing:

```rust
    #[serde(default)]
    pub expires_at: Option<String>,
```

Use `farmerbob_core::window::{read, must_disable, State}`. Do not reimplement the parse, the
boundary or the fail-closed rule; all three are already decided there and duplicating them is
the defect this task exists to remove.

## Falsifiable clauses

1. An arm with no `expires_at` is unaffected: `eligible_at` returns exactly what `eligible` returned
   before, for every existing case. Pin at least the `Ok`, `Disabled` and `RedundantPaidRoute`
   paths.
2. An arm whose `expires_at` is in the future and whose status is dispatchable is `Ok`.
   `eligible` (no clock) returns `Ok` for a CLOSED window too, and that is correct for it: it is
   documented as window-blind. Pin that difference on one arm, so the two are never confused.
3. An arm whose `expires_at` has passed is `WindowClosed`. See clause 6 for how that interacts
   with a status that is not dispatchable; the two clauses are one total order, not two rules.
4. An arm whose `expires_at` cannot be parsed is `WindowClosed`. This follows `window`'s own
   rule and the reason must say the deadline was unreadable: assert that the prose mentions
   being unreadable and mentions the value, case-insensitively, and assert nothing about the
   exact sentence.
5. **`Disabled` outranks `WindowClosed`.** An arm that is explicitly turned off in the registry
   reports `Disabled` with its recorded reason even when its window has also closed, because the
   registry's explicit statement is the more specific fact and is what an operator set by hand.
6. The precedence is TOTAL and is exactly this, highest first:

       Disabled  >  WindowClosed  >  NotDispatchable  >  RedundantPaidRoute  >  Ok

   `Disabled` wins because it is what an operator set by hand. `WindowClosed` beats
   `NotDispatchable` and `RedundantPaidRoute` because a closed window is the harder stop and
   reporting a softer one would hide it. An arm with status `limited` and a closed window is
   therefore `WindowClosed`, not `NotDispatchable`. No two variants are ever both applicable
   once this order is applied.
7. `WindowClosed.why` is non-empty for every arm that returns it.

## Boundaries, at N and at zero

- `now` exactly at the instant: closed. `window::read` returns `Expired { seconds_over: 0 }` at
  that instant and `must_disable` is true, so this clause is already decided in core; assert the
  behaviour here rather than restating the rule.
- An `expires_at` that is the EMPTY STRING: it is present and unreadable, so `WindowClosed`, not
  "absent". A field set to the empty string is not the same as a field that is not set, and the
  two must not collapse.
- An arm absent from the registry: unchanged, `NotDispatchable("absent from the registry")`.
- An arm whose status is `disabled` AND whose window closed: `Disabled`. Clause 5.
- An arm whose window closed and which is also a redundant paid route: `WindowClosed`. Clause 6.
- An arm whose window closed and whose status is `limited`: `WindowClosed`, not `NotDispatchable`.
  Clause 6. This is the pair the precedence exists to decide.

## Superset status on every enumerated list

`Eligibility`'s variants are a CLOSED set of exactly five after this change: `Ok`, `Disabled`,
`NotDispatchable`, `RedundantPaidRoute`, `WindowClosed`. Adding a sixth is a defect, and your
tests may not assert on any variant outside these five.

`window::State`'s three variants are `farmerbob_core::window`'s and you define none of them.

## Composition of aggregate returns

`eligible_at` returns exactly one `Eligibility`; the precedence in clauses 5 and 6 is total and
leaves no pair of variants both applicable. State the full ordering explicitly in a doc comment
so a later reader does not have to derive it from the branch order.

Say why the `Disabled`-over-`WindowClosed` and `WindowClosed`-over-everything-else rules must be
tested against the SAME arm rather than separately: a precedence bug shows up only when two
conditions hold at once, and two tests that each arrange one condition would both pass against
an implementation that checks them in the wrong order.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. `chrono` and `farmerbob-core` are both already in
  `crates/fb/Cargo.toml`; verify that for yourself before writing `use chrono::...` rather than
  taking this line's word for it.
- **Nothing outside this file may break.** `eligible_at` is an ADDITION; `eligible` and
  `dispatchable` keep their signatures, so every existing caller still compiles.
  `cargo build -p fb` must succeed, and if it does not you have changed something you should
  not have.
- `cargo test -p fb` must pass and `cargo clippy -p fb -- -D warnings` must be clean. Run both
  and report what you saw.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares, and one line in the `lib.rs` beside it, and
  nothing else.** Not "only that crate" --
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
