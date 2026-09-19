<!-- fb:modifies crates/farmerbob-core/src/cost.rs -->
# Task: seven cost bugs are one bug -- `Option<f64>` cannot say why it does not know

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/cost.rs`. Do not change any other file.

This task fixes SEVEN filed bugs at once. They are listed below with their bead ids, and
they are one bug wearing seven coats: the cost axis carries `Option<f64>`, so every way of
not knowing a cost collapses into the same `None`, and a `None` that reaches a display
becomes `free` or `$0.0000`.

The Pareto curve of cost against ability is this project's headline deliverable. Its cost
axis is currently unable to distinguish "this run was free" from "nobody measured this run".

## The seven

1. **farmerbob-b52 (P1)** — a metered arm with no recorded price is displayed as free.
   `(None if quota == 'plan' else price_in)` collapses two different `None`s: a plan arm is
   genuinely $0 per run, a metered arm with no price is UNKNOWN. Both print `free`.
2. **farmerbob-6hc5 (P0, closed by triage but the class is open)** — the frontier crowned an
   arm for being unmeasurable. An arm with no cost telemetry sorts to the cheapest position.
3. **farmerbob-xsa (P0)** — the price model has no cache-read dimension. Measured against the
   provider's own dashboard the total is ~10% off **with mixed sign** (gemini -15%, luna
   +5%), which rules out a missing store or dropped sessions and points at prices rather than
   token counts. Cache reads run 4x-12x fresh input depending on the arm, so a small
   per-token error there dominates one arm's bill and barely touches another's — and can
   reorder arms that are close.
4. **farmerbob-tb92 (P1)** — cost accounting under-reports actual OpenRouter spend by ~19%
   against `/api/v1/auth/key` ground truth.
5. **farmerbob-lzae (P1)** — `crates/fb/src/pareto.rs` keeps a PRIVATE duplicate of the
   launcher mapping, so `cost::launcher_from_name` is unreachable in production and the Agy
   launcher added for that purpose changed nothing. Found independently by two critics.
6. **farmerbob-t7er (P3)** — `cost::Launcher` does not know agy, so a plan arm's token count
   is unavailable. For a plan arm the scarce resource is the LIMIT, not dollars; without
   tokens there is no way to predict a cap.
7. **farmerbob-5gh4 (P1)** — an observed provider refusal does not park the arm. Every wave
   re-spends slots on an arm the provider has already refused.

Numbers 1, 2, 3 and 4 are all "a number appeared where an absence belonged". 5 and 6 are
"the launcher table has two implementations and production uses the wrong one". 7 is "an
observation that should change state does not".

## Exact API

`farmerbob_core::pricing::Billing` ALREADY exists with exactly the five states this needs,
including a variant whose own doc comment says `This must never read as free`:

```rust
pub enum Billing { Free, Metered { .. }, MeteredUnpriced, Subscription, Unknown }
```

`cost.rs` does not use it. That is bug 1, in one sentence.

```rust
use crate::measurement::Measurement;
use crate::pricing::Billing;

pub struct RunCost {
    pub arm: String,
    pub task: String,
    /// What this run cost, or why that is not known. REPLACES `Option<f64>`.
    /// `Missing(NotAttempted)` is a run nobody measured; `Observed(0.0)` is a
    /// run that genuinely cost nothing. They are different facts.
    pub usd: Measurement<f64>,
    /// How this arm is billed. A `Subscription` run has a real marginal cost
    /// of zero; a `MeteredUnpriced` run does not, and must never be shown as
    /// free.
    pub billing: Billing,
    pub tokens: Measurement<u64>,
    pub completed: bool,
    pub counts_for_arm: bool,
}

pub struct ArmCost {
    pub arm: String,
    pub runs: u32,
    pub completed: u32,
    /// Total spend, or why it is not known. An arm with ANY unmeasured
    /// counted run is `Missing`, never a partial sum presented as a total.
    pub usd: Measurement<f64>,
    pub billing: Billing,
    pub tokens: Measurement<u64>,
    pub unmeasured_runs: u32,
}

impl ArmCost {
    pub fn completion_rate(&self) -> Measurement<f64>;
    pub fn usd_per_completion(&self) -> Measurement<f64>;
}

/// True only when every counted run has an observed cost AND the billing is
/// one whose zero is real. A `Subscription` arm is fully measured at zero;
/// a `MeteredUnpriced` arm is not measured at all.
pub fn fully_measured(arm: &ArmCost) -> bool;

pub fn aggregate(runs: &[RunCost]) -> Vec<ArmCost>;

/// Arms on the cost/ability frontier. An arm that is not `fully_measured` is
/// NOT A CANDIDATE: it cannot be beaten on an axis it does not occupy.
pub fn frontier(arms: &[ArmCost], epsilon: f64) -> Vec<String>;

pub fn totals(arms: &[ArmCost]) -> (Measurement<f64>, u32, u32);
```

**The launcher table becomes total.** `Launcher` gains `Agy`, and `launcher_from_name`
recognises it:

```rust
pub enum Launcher { Claude, Codex, Gemini, Ori, Opencode, Agy, Unknown }

/// The one launcher table. `crates/fb/src/pareto.rs` keeps a private copy
/// today (bead farmerbob-lzae); this is the copy that must survive, and a
/// name it does not recognise is `Unknown` rather than a guess.
pub fn launcher_from_name(name: &str) -> Launcher;
```

**Cache reads become a named dimension rather than a silent error:**

```rust
/// Tokens a run consumed, by billing class.
pub struct Tokens {
    pub input: Measurement<u64>,
    pub output: Measurement<u64>,
    /// Tokens served from the provider's prompt cache, billed at a different
    /// rate. `Missing` when the launcher does not report them -- which is
    /// most of them, and is exactly why the cost axis is ~10% off.
    pub cache_read: Measurement<u64>,
}

pub fn tokens_from_log(launcher: Launcher, log: &str) -> Tokens;
```

`tokens_from_log` KEEPS its name and its launcher argument and changes its return type.
A launcher that reports no cache-read figure yields `cache_read: Missing(NothingToMeasure)`,
which is the honest answer and the one that makes bug 3 visible instead of silent.

## Falsifiable clauses

1. A run with `billing: Billing::Subscription` and `usd: Observed(0.0)` is fully measured.
   A run with `billing: Billing::MeteredUnpriced` is NOT, whatever its `usd` says. Pin both
   over one arm each; this pair IS bug 1 and the pinning is the fix.
2. `usd_per_completion` on an arm with `usd: Missing(..)` is `Missing`, never `0.0` and never
   a partial sum. Pin that the `Absent` reason survives.
3. **The headline.** An arm that is not `fully_measured` does not appear in `frontier`, even
   when its `usd` would place it cheapest. Pin it with an unmeasured arm whose nominal cost
   is the lowest in the field: it must be absent from the result while a measured, more
   expensive arm is present. That is bug 2, and it crowned a real arm.
4. `totals` returns `Missing` when NOTHING was measured, and `Observed(sum)` when everything
   was. Pin the boundary: one measured run among unmeasured ones does not make the total
   observed, because a partial sum presented as a total is a wrong number, not a partial one.
5. `launcher_from_name("agy")` is `Launcher::Agy`, and an unrecognised name is
   `Launcher::Unknown`. Pin that `Unknown` is returned rather than any other variant — a
   guess here silently misattributes an arm's whole bill.
6. `tokens_from_log` for a launcher that reports no cache figure gives
   `cache_read: Missing(..)` while `input` and `output` may be `Observed`. Pin that one
   missing dimension does not suppress the other two: they answer different questions.
7. Every function above is pure. None reads the filesystem, the environment, a clock or a
   network.

## Boundaries, at N and at zero

- An arm with zero runs: `completion_rate` is `Missing(NothingToMeasure)`, not `0.0`. A rate
  over no trials is undefined, and `0.0` would rank it as the worst arm in the field.
- An arm with runs but zero completions: `completion_rate` is `Observed(0.0)`. This and the
  case above must be pinned together — they are the two that look identical today.
- `usd_per_completion` with `completed == 0`: `Missing(NothingToMeasure)`, not infinity and
  not zero.
- `frontier` over an empty slice: empty. Over a single fully-measured arm: that arm.
- `frontier` where NO arm is fully measured: EMPTY, not the whole field and not the
  cheapest-looking one. Pin it; an empty frontier is an honest answer to "we measured
  nothing".
- `epsilon` at exactly zero and negative: keep whatever `effective_epsilon` already does and
  do not change it. Say in your handoff what it does.

## Superset status on every enumerated list

`Launcher` has exactly these seven variants. `Billing` is `pricing::Billing` and you may not
redefine or extend it — it lives in another file and that file is out of scope.

`Tokens` has exactly three fields. Do not add a `total`: a caller that wants one adds them,
and a stored total is a fourth place for the three to disagree.

The seven bugs above are the whole of what this task fixes. Number 7 (farmerbob-5gh4,
parking on an observed refusal) is the one that CANNOT be finished here: setting
`parked_until` writes to `sources.toml`, which is out of scope. Provide the decision only —
a pure function that says whether an observation warrants a park — and say in your handoff
that the writing half is unfiled work:

```rust
/// Whether an observed run justifies parking the arm, and until when.
///
/// `Observed(reset)` carries the provider's own stated reset, in seconds from
/// the observation, when the log states one. `Missing` means the run failed
/// for some other reason and the arm is fine.
pub fn park_for(launcher: Launcher, exit_code: i32, log: &str) -> Measurement<u64>;
```

Use `crate::limit_signal::parse_reset` for the interval rather than writing a second parser.

## Composition of aggregate returns

`aggregate` returns one `ArmCost` per distinct arm in the input, sorted by arm name
ascending, with `runs` counting every input row for that arm and `counts_for_arm == false`
rows excluded from `completed` and from `usd`. Nothing is dropped: an arm all of whose runs
are uncounted still appears, with `runs > 0` and `usd: Missing`.

`frontier` returns arm names, cheapest first, ties broken by completion rate descending and
then by name, and contains only fully-measured arms.

`totals` returns `(spend, completions, counted_runs)` where the two counts are always
observed — they are counts of rows, which are never unknown — and only `spend` can be
`Missing`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Add no dependencies. No I/O of any kind.
- Reuse `crate::measurement::Measurement`, `crate::measurement::Absent`,
  `crate::pricing::Billing` and `crate::limit_signal::parse_reset`. Defining your own
  version of any of them is a scored defect.
- Callers in `crates/fb` WILL stop compiling — `pareto.rs`, `objective.rs` and `select.rs`
  all read `ArmCost.usd` as an `Option`. That breakage is EXPECTED and you must leave it;
  reaching out to fix it is a scope departure. List in your handoff every caller you saw
  break, because that list is the next task.
- The base you are given is rustfmt-clean; `cargo fmt` will touch your file and nothing else.
- Change `crates/farmerbob-core/src/cost.rs` and nothing else, plus `.fb/handoff.md`.
- RUN `cargo clippy -p farmerbob-core --all-targets -- -D warnings` BEFORE you finish.


## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus `.fb/handoff.md`, which
  the Handoff section below REQUIRES you to write and which is exempt from this rule; plus,
  ONLY when the task
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

  **`.fb/handoff.md` is the one exception, and it OVERRIDES the task's own Rules section.**
  Every spec's Rules says "change <target> and NOTHING else"; the Handoff section below then
  requires you to write `.fb/handoff.md`. Read literally those contradict, and a critic caught
  it: "one correct implementation must leave it untouched to obey the Rules, while another must
  write it to satisfy the Gate". Write the handoff. It is exempt, it has always been exempt,
  and the scope measurement already excludes it. Nothing else is.

  **`cargo fmt` is safe, and this line used to say the opposite.** The base you are given is
  rustfmt-clean -- `cargo fmt --all -- --check` exits 0 on it -- so running the formatter
  rewrites your file and nothing else. Run it if you want it.

  It was not always so, and the history is why this paragraph exists rather than a bare
  permission. The workspace drifted dirty because every merge takes ONE file from ONE arm in
  that arm's style and nothing normalised it afterwards. An arm that then ran a formatter had
  every dirty file its build touched rewritten, and the scope gate counted each one as a
  departure. One arm lost four runs that way, at 51, 52, 51 and 17 departures; the 51 was
  exactly the number of rustfmt-dirty files its changes intersected. The arms were doing
  ordinary Rust and the repository was wrong.

  So if `cargo fmt` DOES touch a file you did not change, the base has drifted again. Revert
  that file, keep your own, and say so in your handoff -- that sentence routes a harness bug
  back where it belongs instead of costing you the run.

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

## A "derive these traits" instruction applies to types YOU define

If a spec says to derive `Debug, Clone, PartialEq, Eq` "on every type in the Exact API", it
means every type the task DEFINES. An Exact API block also NAMES types it imports --
`Measurement`, `Absent`, `Verdict`, `BuildVerdict` -- and you can neither define nor change
those. Read literally the two instructions contradict, and codex-luna reported exactly that
before implementing: "the derive rule requires the implementation to derive those traits on
`Measurement` and `Absent`, but the adjacent rule says those existing types must not be
defined".

Derive on what you write. If an imported type lacks a trait your tests need, say so in your
handoff and test around it; do not edit the other file.

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
