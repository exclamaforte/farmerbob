<!-- fb:creates crates/farmerbob-store/src/migrate.rs -->
<!-- fb:modifies crates/farmerbob-store/src/lib.rs -->
# Task: embedded schema migrations

Create `crates/farmerbob-store/src/migrate.rs`, declare it from that crate's `lib.rs` with
`pub mod migrate;`. Change nothing else except that one `pub mod` line.

## Context

farmerbob's store is a single SQLite file holding runs, grades, leases and quota state. It
is opened by a long-lived daemon and by short-lived CLI invocations, sometimes at the same
time, and it must survive the binary being upgraded underneath it.

Migrations are therefore not a convenience. The rules that matter:

- A migration that has already run must never run again, even if the process crashed
  halfway through the previous startup.
- A database from a NEWER binary must be refused, not "migrated" backwards. Downgrading
  silently is how a schema loses columns and the data in them.
- The version and the schema change must commit together. A version bump that lands without
  its DDL leaves a database that claims to be migrated and is not.

**This module is pure planning: it decides WHAT to run and in what order. It performs no
I/O and opens no database.** The caller applies the plan inside a transaction. Keeping the
decision separable is what makes it testable without a filesystem.

## Exact API — implement these signatures verbatim

```rust
/// One forward-only schema step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// Strictly positive, unique, and applied in ascending order.
    pub version: u32,
    pub name: String,
    /// The DDL. Opaque here; this module never parses or executes it.
    pub sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// Two migrations share a version.
    DuplicateVersion(u32),
    /// A version of 0, which cannot be distinguished from "never migrated".
    ZeroVersion,
    /// The database is newer than this binary knows how to handle.
    DatabaseIsNewer { db: u32, binary: u32 },
    /// A migration is missing from the middle of the sequence the database has applied.
    GapInHistory { missing: u32 },
}

/// What the caller should execute, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Migration>,
    pub from_version: u32,
    pub to_version: u32,
}

impl Plan {
    /// True when the database is already current. An empty plan is a legitimate,
    /// common outcome and not an error.
    pub fn is_noop(&self) -> bool;
}

/// Decide what to run.
///
/// `available` need not be sorted. `current` is the database's recorded version,
/// 0 meaning a fresh database.
pub fn plan(available: &[Migration], current: u32) -> Result<Plan, PlanError>;

/// The highest version this binary knows. `None` when it knows none.
pub fn target_version(available: &[Migration]) -> Option<u32>;

/// Verify a database's applied history against this binary's migration set.
///
/// `applied` is the sorted list of versions the database records as applied.
pub fn check_history(available: &[Migration], applied: &[u32]) -> Result<(), PlanError>;
```

## Required behaviour

- `plan` returns only migrations with `version > current`, in ascending order, regardless of
  the order in `available`.
- A duplicate version is `DuplicateVersion`, reported for the lowest duplicated value so the
  error is deterministic.
- Version 0 is `ZeroVersion`: a fresh database records 0, so a migration numbered 0 could
  never be distinguished from one never applied.
- `current` greater than `target_version` is `DatabaseIsNewer`, never an empty plan. Treating
  it as a no-op lets an old binary run against a new schema.
- `check_history` returns `GapInHistory` naming the LOWEST missing version when `applied`
  skips a version at or below its own maximum. A database that applied 1 and 3 but not 2 is
  in a state no forward-only sequence can produce.
- An applied version this binary does not know about is `DatabaseIsNewer`, not a gap.
- An empty `available` with `current == 0` is a valid no-op plan, not an error.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No I/O, and no rusqlite types in this module's public API.
- Available deps: whatever farmerbob-store already declares. Add none.
- `cargo build -p farmerbob-store` and `cargo test -p farmerbob-store` must pass.

## Tests you must write

One per rule, named after it. At minimum: unsorted input plans in ascending order; a
duplicate version errors deterministically; version 0 is rejected; a newer database is
refused rather than no-opped; a gap names the lowest missing version; an unknown applied
version reads as newer, not as a gap; empty available with current 0 is a no-op.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p farmerbob-store` succeeds
- `cargo test -p farmerbob-store` passes, with at least one test that actually executes
- only the crate named in the task is modified

**Scored, in this order:**
1. **Conformance** — a test suite you will not see, derived from this spec, is run against
   your implementation. The assertions are hidden; the criteria are exactly what this
   document states.
2. **Panic-freedom** — no `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on
   any path reachable from input, outside `#[cfg(test)]`.
3. **`cargo clippy -- -D warnings` clean.**
4. **Test depth and generality** — number of distinct behaviours covered, not number of
   assertions. Your tests must be good enough to catch a bug in **any** correct-looking
   implementation of this spec, not only your own: test the behaviour the specification
   requires, not your particular implementation's internals. Asserting on exact error
   strings, private field names, or an output format the spec does not fix makes a test
   worthless.
5. **Documentation** — `///` on every public item.
6. **Structure** — coherent modules over one large file, where the crate warrants it.

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
