<!-- fb:creates crates/farmerbob-core/src/confinement.rs -->
# Task: confinement that is verified, never assumed

Create `crates/farmerbob-core/src/confinement.rs`, declare it from `lib.rs` with
`pub mod confinement;`. Change nothing else except that one `pub mod` line.

## Context

A refactor once dropped the `systemd-run` wrapper from the dispatcher. Eleven agents ran with
no memory cap, no CPU quota and no pids limit, and **nothing failed** — the runs completed,
were scored, and entered the leaderboard. It was found only by reading `/proc/<pid>/cgroup`
by hand and noticing the path was the caller's, not a per-run scope.

That is the shape: a scope that never materialised is invisible, because its absence looks
exactly like its presence from everywhere except the cgroup itself. A confinement *request*
proves nothing. Only membership does.

A second trap, learned separately: identifying a run's processes by name or command line
matches things that are not the run. `pgrep -f "$worktree"` once matched the dispatcher's own
`git worktree add`, whose cgroup is the caller's, and reported the desktop session's 13.8 GB
as the agent's peak under a 2 GB cap. The cgroup is the authority on membership; asking any
other oracle is asking the wrong question.

**Pure logic: no /proc, no /sys, no process inspection.** Observations are supplied.

## Exact API — implement these signatures verbatim

```rust
/// What the harness asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub unit: String,
    pub memory_max_mib: Option<u64>,
    pub cpu_quota_pct: Option<u32>,
    pub tasks_max: Option<u32>,
}

/// One process seen during the run, with the cgroup path it was actually in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub pid: u32,
    /// Full unified-hierarchy path, e.g. "/user.slice/.../fb-task--arm-123.scope".
    pub cgroup: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confinement {
    /// Every observed process was inside the requested unit.
    Verified { unit: String, pids: usize },
    /// Processes were observed, none inside the requested unit. The refactor case.
    Absent { unit: String, found_in: Vec<String> },
    /// Some inside, some outside. Worse than Absent: the budget applied to part of the run.
    Partial { unit: String, inside: usize, outside: usize },
    /// Nothing was observed at all, so nothing can be concluded.
    Unobserved,
}

impl Confinement {
    /// Only `Verified`. `Unobserved` is NOT confined and NOT unconfined.
    pub fn is_verified(&self) -> bool;
    /// True when the run's resource figures may be attributed to it. False for everything
    /// except `Verified`, because a figure from a foreign cgroup is not this run's.
    pub fn may_attribute_resources(&self) -> bool;
}

/// Decide from what was seen. A process counts as inside when its cgroup path CONTAINS the
/// unit name as a path component.
pub fn verify(req: &Request, seen: &[Seen]) -> Confinement;

/// Cgroup paths observed that are not the requested unit, deduplicated and sorted. These
/// are the foreign cgroups, and a resource figure read from one is not this run's.
pub fn foreign_cgroups(req: &Request, seen: &[Seen]) -> Vec<String>;

/// Which caps the request omitted. An uncapped dimension is unbounded, and a run that is
/// "confined" on memory alone can still take the machine down on pids.
pub fn uncapped(req: &Request) -> Vec<&'static str>;
```

## Required behaviour

- A process is inside iff its cgroup path contains the unit name **as a whole path
  component**, delimited by `/` or the end of the string. A unit named `fb-a` must NOT match
  a cgroup ending in `fb-abc.scope`.
- Empty `seen` is `Unobserved`, never `Absent`. Observing nothing and observing only foreign
  processes are different facts, and the first blames nobody.
- `Partial` is returned whenever at least one process is inside and at least one is outside,
  and it reports both counts.
- `may_attribute_resources` is true only for `Verified`. This is the rule that would have
  caught the 13.8 GB reading.
- `foreign_cgroups` excludes the requested unit, deduplicates, and sorts.
- `uncapped` returns the names of absent caps from exactly this set and no others:
  `"memory_max_mib"`, `"cpu_quota_pct"`, `"tasks_max"`, in that order.
- An empty unit name means nothing can match: `Absent` when processes were seen,
  `Unobserved` when none were.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: `fb-a` does not match `fb-abc.scope`; empty `seen`
is Unobserved not Absent; a mixed run is Partial with both counts; `may_attribute_resources`
is false for Partial and Unobserved; `foreign_cgroups` dedupes and sorts; `uncapped` lists
omissions in the stated order; an empty unit with processes seen is Absent.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
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

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
because a test asserted a case the specification never fixed.

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
