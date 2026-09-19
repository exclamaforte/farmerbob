<!-- fb:creates crates/farmerbob-core/src/price_update.rs -->
# Task: prices are refreshed by a regex rewriting sources.toml in place

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/price_update.rs` and add `pub mod price_update;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## Why this exists

`fb-prices.sh` fetches OpenRouter's model catalogue and rewrites `price_in` and `price_out` in
`sources.toml` with a Python regex. The decision it makes -- which arms need updating, what the
new value is, and whether a change is safe to apply -- is pure, and it is currently entangled
with an HTTP call and an in-place file edit.

It matters because those prices feed the Pareto curve that is the point of this project. A
price silently rewritten wrong moves a point on the headline chart, and a regex over TOML is
how that happens.

## Exact API

```rust
use crate::measurement::Measurement;

/// What the registry holds for one arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    /// The arm's registry name.
    pub arm: String,
    /// The model id, e.g. `openrouter/inclusionai/ling-3.0-flash-vl:free`.
    pub model: String,
    /// Price per million input tokens, in millionths of a unit so this type
    /// stays `Eq`. `Missing` when the registry states none.
    pub price_in: Measurement<u64>,
    /// Price per million output tokens, same units.
    pub price_out: Measurement<u64>,
}

/// What the catalogue says, keyed the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quoted {
    /// The model id as the catalogue spells it.
    pub model: String,
    /// Catalogue price per million input tokens.
    pub price_in: u64,
    /// Catalogue price per million output tokens.
    pub price_out: u64,
}

/// One arm's price compared against the catalogue. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The registry and the catalogue agree.
    Agrees,
    /// They differ, and this is the new pair.
    Differs {
        /// New input price.
        price_in: u64,
        /// New output price.
        price_out: u64,
    },
    /// The registry states no price and the catalogue does. Filling a blank
    /// is not the same as correcting a figure.
    Fills {
        /// The input price to fill in.
        price_in: u64,
        /// The output price to fill in.
        price_out: u64,
    },
    /// The catalogue does not list this model. NEVER treated as free.
    Unquoted,
}

/// Compare one arm against the catalogue.
pub fn compare(reg: &Registered, catalogue: &[Quoted]) -> Change;

/// Every arm that needs an edit, in the order given. `Agrees` and
/// `Unquoted` are not edits and do not appear.
pub fn pending(regs: &[Registered], catalogue: &[Quoted]) -> Vec<(String, Change)>;
```

## Falsifiable clauses

1. Equal prices on both sides: `Agrees`.
2. Different prices: `Differs` carrying the CATALOGUE's pair, not the registry's.
3. Registry `Missing` on both prices and the catalogue quoting them: `Fills`.
4. The model absent from the catalogue: `Unquoted`, whatever the registry holds. **A model the
   catalogue does not list is not a free model**, and pinning this is the point: an arm whose
   price silently became zero would be ranked cheapest on the Pareto curve.
5. Registry `Missing` on ONE price and holding the other: this is `Fills` or `Differs` and not
   `Agrees`; pin which you chose and pin it for both orderings of which side is missing.
6. `pending` omits every `Agrees` and every `Unquoted`, and preserves the input order of the
   rest.
7. `compare` matches the model id EXACTLY. Pin that a catalogue entry differing only by a
   trailing `:free` suffix does NOT match, because those are different routes at different
   prices.

## Boundaries, at N and at zero

- An EMPTY catalogue: every arm is `Unquoted`, and `pending` is empty.
- An EMPTY `regs`: `pending` is empty.
- A catalogue price of ZERO with the registry also zero: `Agrees`. Zero is a price.
- A catalogue price of ZERO with the registry `Missing`: `Fills` with zero. Free is a fact and
  recording it is not the same as leaving it blank.
- The SAME model id appearing twice in the catalogue with different prices: the FIRST entry
  wins, and later duplicates are ignored. A catalogue is a list and a list can repeat, so this
  is pinned here rather than delegated.

## Superset status on every enumerated list

`Change`'s variants are a CLOSED set of exactly four: `Agrees`, `Differs`, `Fills`, `Unquoted`.
Adding a fifth is a defect, and your tests may not assert on anything outside them.

`Measurement` and `Absent` are `crate::measurement`'s.

## Composition of aggregate returns

`pending` returns one entry per arm needing an edit, in the order `regs` was given, with no
deduplication and no sorting: the caller writes the file and the order it writes in is its
business.

Say why clauses 3 and 4 must be tested together: both are cases where the registry holds no
usable price, and an implementation that treats an unquoted model as a blank to fill would write
a price the catalogue never stated. That is the defect that moves a point on the headline chart.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies, no I/O and no HTTP. The caller fetches; this compares.
- `cargo build -p farmerbob-core`, `cargo test -p farmerbob-core` and
  `cargo clippy -p farmerbob-core -- -D warnings` must pass; all three are clean on HEAD.
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
