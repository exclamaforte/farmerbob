<!-- fb:modifies crates/farmerbob-core/src/quota.rs -->
# Task: a refusal should park the arm that was refused

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/quota.rs`. Do not change any other file.

Read `crates/farmerbob-core/src/limit_signal.rs` first -- `classify`, `parse_reset` -- and
`crates/farmerbob-core/src/outcome.rs` for `OutcomeClass`. Both already exist and you are
composing them, not rewriting them.

## The waste this stops

`sources.toml` has `parked_until`, and both `router::eligibility` and `fb-eligible.sh` honour it.
NOTHING EVER SETS IT from an observed refusal. So when a provider says no, every subsequent wave
dispatches into the same wall. Counted on 2026-09-17:

    wave36  or-hy3 4s, or-qwen38-flash 9s, or-nemotron-ultra 75s
    wave41  ifm-k2-horizon 11s, ifm-k2-think 13s
    wave51  ifm-k2-horizon 7s
    wave53  ifm-k2-horizon 18s, ifm-k2-think 19s
    wave55  ifm-k2-horizon 9s, ifm-k2-think 10s

Eleven dispatch slots spent on arms whose provider had already refused, hours earlier. The
classification is correct -- these are recorded as quota_limited, not as the model producing
nothing -- so the bandit is safe. What is wasted is slots and wall-clock.

## Exact API

```rust
/// Whether an arm should be parked after a finished run, and until when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Park {
    /// Do not park. The run tells us nothing about future availability.
    No,
    /// Park until this instant, in unix milliseconds, because the provider said so.
    Until { at_ms: u64, grounds: String },
    /// Park for a backoff because the provider refused without saying when it would relent.
    Backoff { until_ms: u64, grounds: String },
}

/// Decide whether a finished run should park its arm.
///
/// `now_ms` is the current instant; `log_head` is the run's own log, already ANSI-stripped.
pub fn park_after(
    class: OutcomeClass,
    log_head: &str,
    now_ms: u64,
    default_backoff_ms: u64,
) -> Park;
```

Use `limit_signal::parse_reset` for the stated instant. Do not write a second reset parser.

## Falsifiable clauses

1. `OutcomeClass::ArmResult` is `Park::No` whatever the log says. The arm ran; nothing about the
   provider was learned.
2. `QuotaLimited` with a log stating a reset -- `retry-after: 3600` -- is `Until`, at the instant
   `parse_reset` gives, with grounds quoting what was found.
3. `QuotaLimited` with NO stated reset is `Backoff { until_ms: now_ms + default_backoff_ms }`,
   and the grounds must say the provider did not state a time. Never `Until` with a guessed
   instant: an invented reset is worse than an admitted backoff, because the next reader cannot
   tell it from a real one.
4. `Infrastructure` is `Park::No`. A broken harness is not a provider refusing, and parking the
   arm would hide our own fault as the arm's unavailability.
5. `Cancelled` is `Park::No`. We killed it.
6. `Unknown` is `Park::No`. Nothing was determined, so nothing is concluded.
7. A reset instant already in the PAST relative to `now_ms` is `Park::No`, not `Until`. A window
   that has already closed does not park anything.

## Boundaries, at N and at zero

- `default_backoff_ms: 0` with no stated reset: state what you return and pin it. A zero backoff
  parks until now, which parks nothing; say whether that is `No` or a degenerate `Backoff`.
- A reset exactly equal to `now_ms`: decide, state, pin. The window closes at this instant.
- `now_ms + default_backoff_ms` overflowing `u64`: saturate, do not wrap.
- An empty `log_head` with `QuotaLimited`: `Backoff`, since the class is authoritative and the
  absent log merely fails to refine it.
- `now_ms: 0`: no underflow when comparing a past reset.

## Superset status on every enumerated list

`Park` is a CLOSED set of three. `OutcomeClass` is closed and yours to read, not to extend.

Say in `park_after`'s doc that the classes which DO park are a decision, not a discovery: today
only `QuotaLimited` parks, and a future class might justify it. And say plainly that this
function decides about ONE ARM from ONE RUN -- it does not know that one key serves thirteen
arms, and a caller that parks a whole provider is doing something this function did not
authorise.

## Composition of aggregate returns

If you add a function over several runs, its doc must state what it returns for an EMPTY slice
and whether the result is the earliest or latest park, both pinned. "No run justified a park" and
"no run was examined" must not be the same value.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No I/O and no clock: `now_ms` arrives as an argument.
- Do not change any existing signature in this module.
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
