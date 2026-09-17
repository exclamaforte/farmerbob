<!-- fb:modifies crates/farmerbob-core/src/outcome.rs -->
<!-- fb:reads fb-objective.sh -->
# Task: move run-outcome classification into core, beside the signal it uses

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/outcome.rs`. Do not change any other file.

Read `crates/farmerbob-core/src/limit_signal.rs` and `crates/fb/src/objective.rs` first.
`outcome.rs` already owns `OutcomeClass` and the rules about which outcomes count toward a
posterior. The DECISION that produces an `OutcomeClass` from a finished run lives in
`crates/fb/src/objective.rs`, in the binary, next to the I/O.

## Why

That decision has been wrong four times this week and each repair happened in the binary:

- a launcher refusal was scored as the model producing nothing, four distinct strings so far
  (individual quota, rate limit, key limit, token limit);
- the haystack was never lowercased, so no capitalised launcher error ever matched;
- patterns anchored on `error: ` were defeated by a colour escape landing between the prefix
  and the message;
- and once fixed, an arm that QUOTED a pattern in its summary was scored as having been refused.

`strip_ansi` already moved to core and `objective.rs` delegates to it. The classification should
follow, so the rules and the signal live together and a fifth incident is repaired once.

## Exact API

Keep `OutcomeClass`, `Rescue`, `Outcome`, `posterior_delta`, `needs_triage`, `class_census` and
every existing signature unchanged. Add, public:

```rust
/// What a finished run looked like, as facts rather than conclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFacts {
    /// Process exit status. `None` when the run was never reaped.
    pub exit_code: Option<i32>,
    /// Lines added to the declared deliverable. `None` when never measured.
    pub lines_added: Option<u32>,
    /// The head of the run's own log, already ANSI-stripped by the caller.
    pub log_head: Option<String>,
    /// An explicit class the harness recorded, which is authoritative when present.
    pub declared: Option<OutcomeClass>,
}

/// Classify one finished run.
pub fn classify(facts: &RunFacts, rules: &crate::limit_signal::SignalRules) -> OutcomeClass;
```

Use `limit_signal::{classify as classify_signal, SignalRules, Classification}` for refusal
detection. Do NOT define a second pattern list in this module.

## The decision order, fixed here so it is not rediscovered

1. `declared` wins outright when present. The harness knows things this function does not.
2. Exit 143 or 137 is `orchestrator_cancelled`. We killed it; that is not the arm.
3. Exit 127 is `infrastructure`. "Command not found" is always the harness and never the model,
   which was never reached.
4. A refusal recognised by `limit_signal` is `quota_limited`.
5. **The shape rule.** A non-zero exit, ZERO lines added, and a log-head line BEGINNING `error:`
   means the launcher failed before the agent ran. All three are required. The line alone is not
   enough -- an agent may print an error while working -- and the zero-lines condition is what
   stops a genuinely failing arm being excused.
6. Otherwise `arm_result`.

## Falsifiable clauses

1. `declared: Some(Infrastructure)` returns `Infrastructure` whatever else the facts say.
2. Exit 143 -> `orchestrator_cancelled`; exit 137 likewise.
3. Exit 127 -> `infrastructure`, even with lines added.
4. A log head whose line begins `error: rate limit exceeded`, with exit 1 and 0 lines ->
   `quota_limited`.
5. **An arm that QUOTED a pattern mid-sentence is `arm_result`.** Verbatim from the real
   incident: `Done. Provider-refusal detection ... (`error: rate limit exceeded`) now matches.`
   with exit 0 and 396 lines added. Matching must be anchored to the START of a line.
6. The shape rule with an UNKNOWN refusal string -- `error: monthly ceiling reached` -- is NOT
   `arm_result`. The set of ways a provider says no is open; a fifth string will arrive.
7. An arm that wrote 240 lines and then exited 1 with an `error:` line is `arm_result`. It was
   given its chance and failed; loudness is not a refusal.
8. Exit 0 with zero lines and an `error:` line is `arm_result` -- an ordinary NO-OP that must
   keep counting against the arm.
9. `exit_code: None` -- never reaped -- is NOT `arm_result`. Choose a class, justify it in the
   doc comment, and pin it.

## Boundaries, at N and at zero

- `log_head: None`: classify on the exit code alone; never `quota_limited`, because nothing was
  read. An absent log is not evidence of a clean run.
- `log_head: Some("")`: same as `None` for matching purposes, and must not panic.
- `lines_added: None` (never measured) is NOT `Some(0)`. The shape rule requires a measured zero;
  say which way you resolved an unmeasured count and pin it.
- Exit code `i32::MIN` and `i32::MAX`: no panic, falls through to the ordinary rules.
- A `SignalRules` with no patterns: nothing is ever `quota_limited` by rule 4, and rule 5 still
  applies.

## Superset status on every enumerated list

`OutcomeClass` is CLOSED; do not add a variant.

The exit codes named above -- 143, 137, 127 -- are a **known subset** of the codes that mean the
harness rather than the arm. Say so in the doc comment. A code you do not recognise falls to the
ordinary rules rather than being guessed at.

## Composition of aggregate returns

If you add a function over several runs, its doc must state what it returns for an EMPTY slice
and whether the order is input order, both pinned by tests. "No run was refused" and "no run was
examined" must not be the same value.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. No I/O: the caller supplies the log head, already stripped.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run them.
- `crates/fb` will NOT break -- you are adding, not changing. If it does, you have changed an
  existing signature; do not.

## Do not fabricate

If part of this cannot be done in your environment, say so plainly and leave it undone with a
comment naming what you could not verify. Returning less with a stated reason is correct here and
is scored as one.

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
