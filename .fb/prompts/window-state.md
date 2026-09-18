<!-- fb:creates crates/farmerbob-core/src/window.rs -->
# Task: an arm enabled "for five hours" is a promise nothing in this harness keeps

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/window.rs` and add `pub mod window;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

Arms are sometimes enabled for a fixed span: a paid route opened for five hours, a free tier
that resets at a stated instant. `sources.toml` records that as an `expires_at` field beside
the arm's `status`.

Nothing enforced it. One such window ran **one hour and forty-seven minutes** past its end with
the arm still dispatching, and the only thing that caught it was the orchestrator happening to
read a clock. The rule now lives in a Python heredoc embedded in `fb-status.sh`, where it is
untested, unreachable from any other reader, and duplicated the moment a second caller needs
it.

It also contains a decision nobody wrote down. An `expires_at` that does not parse is treated
as EXPIRED -- which is right, because an unreadable deadline is not a licence to keep running --
but that is a judgement sitting in a `except ValueError` branch of a shell heredoc.

This module is that rule, in the crate, with the judgement stated.

## Exact API

```rust
use chrono::{DateTime, Utc};

/// One time-boxed arm, as the registry holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boxed {
    /// The arm's registry name.
    pub arm: String,
    /// The `expires_at` field exactly as written, unparsed.
    pub expires_at: String,
    /// Whether the registry currently has this arm enabled.
    pub enabled: bool,
}

/// What a window is doing at one instant. Exactly these variants and no
/// others. Says nothing about whether the arm is enabled: that is a fact
/// about the registry, not about the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The instant has not been reached.
    Open {
        /// Whole seconds remaining. Always strictly positive.
        seconds_left: i64,
    },
    /// The instant has been reached or passed.
    Expired {
        /// Whole seconds since it passed. Zero at the instant itself.
        seconds_over: i64,
    },
    /// `expires_at` could not be read as an instant.
    Unparseable,
}

/// Read one window against a clock. This module has no clock of its own;
/// `now` is the caller's.
pub fn read(expires_at: &str, now: DateTime<Utc>) -> State;

/// Whether this arm must be disabled now.
///
/// `enabled` is the registry's current setting. An arm that is already
/// disabled needs no action however long ago its window closed.
pub fn must_disable(s: &State, enabled: bool) -> bool;

/// One line per arm, for a human reading `fb-status`.
pub fn report(boxed: &[Boxed], now: DateTime<Utc>) -> Vec<String>;
```

## Falsifiable clauses

1. `read` returns `Open { seconds_left }` when `now` is strictly before the instant, and
   `seconds_left` is the whole seconds between them, truncated toward zero.
2. `read` returns `Expired { seconds_over }` when `now` is at or after the instant.
3. **AT the instant exactly** -- `now` equal to `expires_at` to the second -- the answer is
   `Expired { seconds_over: 0 }`, not `Open { seconds_left: 0 }`. A window that has run out is
   out. `Open` therefore never carries a `seconds_left` of zero, and that is pinned in the type's
   own documentation above.
4. `read` returns `Unparseable` for any `expires_at` it cannot read as an instant, and NEVER an
   error and never a panic. The empty string is unparseable.
5. A timestamp carrying no timezone offset is read as UTC. Pin it. The registry has held both
   forms and the naive one must not become `Unparseable`.
6. `must_disable` is true when the arm is `enabled` and its state is anything other than `Open`.
   In particular it is TRUE for `Unparseable`: an unreadable deadline is not a licence to keep
   running, and the alternative -- treating it as open -- is the failure this module exists to
   prevent.
7. `must_disable` is false for every state when `enabled` is false.

## Boundaries, at N and at zero

- `now` exactly at the instant: `Expired { seconds_over: 0 }`. Clause 3.
- One second before: `Open { seconds_left: 1 }`. One second after: `Expired { seconds_over: 1 }`.
- Sub-second differences: `seconds_left` and `seconds_over` are WHOLE seconds truncated toward
  zero, so 1500ms before the instant is `Open { seconds_left: 1 }` and 500ms before is
  `Open { seconds_left: 0 }` -- which clause 3 forbids. Resolve it the way clause 3 requires:
  any `now` strictly before the instant is `Open`, and when truncation would yield zero the
  value reported is 1, so `Open` never carries zero. Pin both sub-second cases.
- An empty `expires_at`: `Unparseable`.
- An `expires_at` far in the past: `Expired`, with the full elapsed seconds, not a clamp.
- `report` on an EMPTY slice: an empty vector, not one line saying there are no windows.
- A `Boxed` whose `arm` is the empty string: degenerate, not invalid. It reads and reports like
  any other.

## Superset status on every enumerated list

`State`'s variants are a CLOSED set of exactly three: `Open`, `Expired`, `Unparseable`. Adding
a fourth is a defect, and your tests may not assert on any state outside these three.

The timestamp formats you accept are the second form: **at least RFC 3339 with an offset, and
RFC 3339 with no offset read as UTC; accepting more is neither required nor penalised.**
Because it is a superset, **your tests may not assert that any other format is unparseable** --
a rival that also reads, say, a bare date is not wrong. Test the two required forms and test the
empty string, which clause 4 pins.

## Composition of aggregate returns

`report` returns exactly one line per element of `boxed`, in the order given, never reordered,
never filtered, and never merged: an arm whose window is open still gets a line, because a
report that lists only problems cannot be read as a list of what is time-boxed. Every line
contains that arm's name. What else a line says is prose and is not pinned; assert that the
name appears and that the line is non-empty, and assert nothing about the wording.

Say why `read` and `must_disable` must be tested together rather than separately: `must_disable`
is only meaningful against the states `read` can produce, and a matched pair of bugs -- a `read`
that returns `Open` at the instant and a `must_disable` that trusts `Open` -- would pass two
independent tests and reproduce the hour and forty-seven minutes exactly.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies and no I/O. `chrono` is ALREADY a dependency of this crate; use it. The
  timestamp arrives as a `&str` and the clock as an argument.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.

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
