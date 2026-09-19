<!-- fb:modifies crates/farmerbob-core/src/cost.rs -->
# Task: the cost reader knows two launchers and the roster runs five

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/cost.rs`. Do not change any other file.

## Why this exists

`cost::Launcher` is the enum that decides how to read a token count out of a run log:

```rust
pub enum Launcher {
    Codex,    // prints `tokens used` then a comma-grouped count on the NEXT line
    Zcode,    // prints no token count. Recognised so the reason can say so precisely
    Unknown,  // anything else
}
```

The roster dispatches through five launchers -- `codex`, `zcode`, `agy`, `opencode` and
`claude` -- and three of them land in `Unknown`. The string `agy` does not appear anywhere in
this file, and `agy` is the launcher behind `gemini-38-flash`, which has been in almost every
wave this project has run.

`Unknown` is not wrong: it yields `Measurement::Missing`, and missing is the honest answer for a
launcher whose output nobody has read. But it makes three different facts identical -- "this
launcher prints no count", "this launcher prints one and we have not taught the reader to find
it", and "we have never heard of this launcher" -- and the whole point of `Launcher` is that
`Zcode` exists SO THAT the reason can say precisely which of those it is.

## Exact API

`Launcher` gains exactly two variants and nothing else changes shape:

```rust
pub enum Launcher {
    Codex,
    Zcode,
    /// `agy`: the Google-OAuth CLI. Prints no token count today.
    Agy,
    /// `opencode`, including every `or-*` arm that runs through it.
    /// Prints no token count today.
    Opencode,
    Unknown,
}
```

Both new variants behave exactly as `Zcode` does for the purpose of reading a count: there is
none, and the `Missing` reason says which launcher it was and that it reports none. Do not
invent a parse for output nobody has captured.

Whatever function maps a launcher NAME to this enum gains the two names. If no such function
exists, add one and pin it:

```rust
/// The launcher a run went through, from its registry name.
/// Unrecognised names are `Unknown`, which is a fact and not a failure.
pub fn launcher_from_name(name: &str) -> Launcher;
```

## Falsifiable clauses

1. `launcher_from_name` maps `"agy"` to `Agy` and `"opencode"` to `Opencode`.
2. It maps `"codex"` to `Codex` and `"zcode"` to `Zcode`, unchanged.
3. It maps any other string to `Unknown`, including the empty string.
4. Reading a token count for `Agy` and for `Opencode` yields `Measurement::Missing`, and the
   reason NAMES that launcher. Assert that the reason mentions the launcher's name,
   case-insensitively; do not assert the sentence.
5. The reason for `Agy` and the reason for `Opencode` are DIFFERENT strings. They are different
   facts and a caller reading a log must be able to tell them apart. Pin the difference, not the
   wording.
6. `Codex`'s existing parse is unchanged: pin the `tokens used` / next-line / comma-grouped
   behaviour exactly as it is today, including one log that has the marker and no following
   line.
7. Nothing in this module gains I/O, a clock or a dependency.

## Boundaries, at N and at zero

- The EMPTY string as a launcher name: `Unknown`. Clause 3.
- A name differing only in case, such as `"AGY"`: state whether you match case-sensitively and
  pin the answer you chose. The registry writes these names in lower case today, so either
  answer is defensible and the tests must agree with the code.
- A log of ZERO bytes for a `Codex` run: `Missing`, not `Observed(0)`. A run that logged nothing
  did not report zero tokens.
- A `Codex` log whose `tokens used` marker is the LAST line, with nothing after it: `Missing`.
  Clause 6 names this case; it is the one the existing parse most easily gets wrong.
- A count of exactly `0` printed after the marker: `Observed(0)`. Zero tokens reported is an
  observation, and this is the boundary where it differs from the case above.

## Superset status on every enumerated list

`Launcher`'s variants are a CLOSED set of exactly five after this change: `Codex`, `Zcode`,
`Agy`, `Opencode`, `Unknown`. Adding a sixth is a defect, and your tests may not assert on any
variant outside these five.

The launcher NAMES recognised are a CLOSED set of exactly four: `codex`, `zcode`, `agy`,
`opencode`. `claude` is deliberately absent -- it is disabled in this roster and nobody has
captured its output -- and adding it without a captured log would be inventing a fact. Say so
in your handoff.

`Measurement` and `Absent` are `crate::measurement`'s.

## Composition of aggregate returns

Every public function in this module keeps its current signature. The enum grows; nothing else
does.

Say why clauses 4 and 5 must be tested together: clause 4 alone passes against an implementation
that returns one shared reason for every countless launcher, which reintroduces exactly the
collapse this task removes -- three facts, one value. The difference is the content.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies and no I/O. A log arrives as a `&str`.
- Adding a variant to a public enum BREAKS every exhaustive match on it elsewhere in the
  workspace. That breakage is EXPECTED and you must leave it: fixing those callers is a
  departure from the declared scope and scores as one. Say in your handoff what you left broken.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass, and
  `cargo clippy -p farmerbob-core -- -D warnings` must be clean.

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
