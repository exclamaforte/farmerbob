<!-- fb:creates crates/farmerbob-core/src/cred_shield.rs -->
# Task: isolating an agent's state also hides its credentials, and nothing notices

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/cred_shield.rs`. Declare it with one `pub mod cred_shield;`
line in `crates/farmerbob-core/src/lib.rs` and change nothing else.

## Why this exists

Every dispatched run gets private `XDG_DATA_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME`
directories, because N agents sharing one SQLite database deadlock (bead farmerbob-p3r). That
fix was correct and it broke a whole arm family, because opencode keeps its provider API keys in
`$XDG_DATA_HOME/opencode/auth.json` — so every `oc-*` run was handed an empty directory, found
no key, and the provider answered:

```text
Error: { "name": "UnknownError", "message": "Unexpected server error. Check server logs for details." }
```

Three arms across TWO vendors died that way in under eight seconds each. The message names
neither authentication nor us, so it read as the vendors being down, and it cost a day.

This is the project's recurring defect in its purest form: **a missing credential is
indistinguishable from a remote outage.** The cure is not to un-isolate. It is to know, before
launching, which credentials a launcher needs and whether the private environment will actually
contain them — and to be able to say so.

## Exact API

```rust
/// A credential file an agent launcher reads, expressed relative to the XDG
/// base directory it lives under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// Which XDG base the path is relative to.
    pub base: XdgBase,
    /// Path beneath that base, e.g. `"opencode/auth.json"`. Never absolute.
    pub rel: String,
}

/// The XDG base directory a credential lives under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdgBase {
    /// `$XDG_DATA_HOME`
    Data,
    /// `$XDG_CONFIG_HOME`
    Config,
    /// `$XDG_STATE_HOME`
    State,
}

/// What a launcher needs in order to authenticate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Needs {
    /// The launcher reads these files, all of which must be present.
    Files(Vec<Credential>),
    /// The launcher authenticates from the process environment, not from a
    /// file, so isolating XDG cannot break it.
    Environment,
    /// This module does not know how the launcher authenticates. NOT a claim
    /// that it needs nothing.
    Unknown,
}

/// What the named launcher needs.
///
/// Known launchers: `"opencode"` reads `XDG_DATA_HOME/opencode/auth.json`;
/// `"ifm"` and `"ori"` are `Environment`. Any other name is `Unknown`.
/// At least these; a caller may not assume the list is closed.
pub fn needs(launcher: &str) -> Needs;

/// Whether an isolated environment will actually satisfy a launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shielded {
    /// Every needed credential is present. Safe to launch.
    Satisfied,
    /// Credentials are needed and these are missing. Carries them in the
    /// order `needs` listed them. Launching WILL fail, and it will fail
    /// looking like something else.
    Missing(Vec<Credential>),
    /// Nothing is known about this launcher, so nothing is claimed. A caller
    /// must not read this as `Satisfied`.
    Unchecked,
}

/// Check a launcher against a private environment.
///
/// `present` answers whether a given credential exists in the environment the
/// run will actually get. This module never touches the filesystem.
pub fn check(launcher: &str, present: &impl Fn(&Credential) -> bool) -> Shielded;
```

## Falsifiable clauses

1. `needs("opencode")` is `Files(..)` containing exactly one `Credential` with
   `base: XdgBase::Data` and `rel: "opencode/auth.json"`.
2. `needs("ifm")` and `needs("ori")` are both `Environment`.
3. `needs("something-nobody-registered")` is `Unknown`. Pin that it is NOT `Environment` and
   NOT `Files(vec![])` — an empty requirement list is a claim that nothing is needed, and that
   is the exact lie this module exists to stop telling.
4. `check("opencode", &|_| false)` is `Missing`, and the missing list is the one credential
   from clause 1.
5. `check("opencode", &|_| true)` is `Satisfied`.
6. `check("ifm", &|_| false)` is `Satisfied` — an environment-authenticated launcher is not
   made unsafe by an empty XDG directory. Clauses 4 and 6 use the SAME `present` closure and
   must be pinned together; that pairing is the whole distinction.
7. `check("unregistered", &|_| true)` is `Unchecked`, not `Satisfied`. Ignorance and safety are
   different answers and a caller must be able to tell them apart.
8. `check` calls `present` only for credentials `needs` returned. Pin that for an `Environment`
   launcher `present` is never called.

## Boundaries, at N and at zero

- A launcher needing several files, with SOME present: `Missing` listing only the absent ones,
  in the order `needs` gave them. Pin the order.
- A launcher needing several files with ALL absent: every one is listed. `Missing` is not a
  flag that stops at the first failure.
- The empty launcher name `""` is `Unknown`, and `check` on it is `Unchecked`.

## Superset status on every enumerated list

The launcher table in `needs` is **at least these; a caller may not assume it is closed**, and
your tests may not assert that some other name is `Unknown` beyond the one unregistered-name
case clause 3 pins — a correct implementation may know about more launchers than this spec
names.

`Needs` and `Shielded` have exactly the variants listed and no others.

`XdgBase` has exactly three variants. `XDG_CACHE_HOME` is deliberately absent: a cache that can
be rebuilt is not a credential, and a credential found only in a cache is a bug elsewhere.

## Composition of aggregate returns

`Missing` carries a subsequence of what `needs` returned, in that same relative order, and
contains exactly the credentials for which `present` answered false. Neither more nor fewer.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. Do not read the filesystem or the process environment from this module.
- Derive `Debug, Clone, PartialEq, Eq` on every type this task defines.
- Create `crates/farmerbob-core/src/cred_shield.rs` plus the one `pub mod` line, and nothing else.
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

  **DO NOT RUN `cargo fmt`.** This is now the single most common way a good implementation
  loses its task, and it is worth its own line because it does not feel like a departure. This
  workspace is not uniformly formatted, `cargo fmt` has no `--check`-only habit to fall back
  on, and running it rewrites every file it disagrees with anywhere in the workspace.
  score-delta/codex-luna shipped a careful, correct-looking 272-line implementation of its
  actual target -- a `git archive` baseline, every failure represented as `Missing` -- and
  scored `OUT-OF-SCOPE` on 17 departures that were ENTIRELY reformatting: line-wrapped
  `assert!` calls in files the task never mentioned. Not one of them changed behaviour, and
  all 17 counted. Format the file you were given, by hand, and leave the rest of the workspace
  exactly as you found it.

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
