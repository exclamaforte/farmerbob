<!-- fb:creates crates/farmerbob-core/src/selection.rs -->
# Task: implement winner selection and loser sweeping

Create `crates/farmerbob-core/src/selection.rs`, declare it from `lib.rs` with
`pub mod selection;`. Change nothing else.

## Context

When N agents attack one task, farmerbob picks a winner, merges it, and sweeps the rest. The
losers are not garbage: every rejected candidate is a labelled negative, and the archive of
them is the corpus used to measure whether a reviewer can actually detect a defect. Sweeping
must preserve them.

Merging N independent implementations into one repo produces one conflict class overwhelmingly
more often than any other: **additive**. Two winners each add a `mod x;` line, a dependency, a
match arm. Neither is wrong and the union is correct, but git cannot know that.

**Pure logic: no I/O, no git invocation.** Diffs and conflicts are passed in as data.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CandidateRef(pub String);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchiveRef(pub String);

/// One conflicting region: what each side put there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Conflict {
    pub path: String,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Resolution {
    /// Both sides only INSERTED distinct lines; the union is correct. Lines are the merged
    /// set, deduplicated, in a deterministic order.
    Union { lines: Vec<String> },
    /// Anything else. A human or the planning agent decides.
    Escalate { reason: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SelectionError {
    /// The candidate's result was quarantined; promoting it needs an explicit override.
    Quarantined { candidate: CandidateRef },
    UnknownCandidate { candidate: CandidateRef },
}

/// Classify a conflict. Union only when BOTH sides are pure insertions of distinct lines:
/// neither side removed a line the other kept, and the two line sets are disjoint.
pub fn resolve(conflict: &Conflict) -> Resolution;

/// The archive ref a swept loser is preserved under: `refs/fb/archive/<task>/<candidate>`.
/// The candidate name is sanitised the same way a branch name would be.
pub fn archive_ref(task: &str, candidate: &CandidateRef) -> ArchiveRef;

/// Which candidates to sweep: everything except the winner. Returns them in a stable order.
/// Fails if the winner is not among the candidates.
pub fn sweep_list(
    candidates: &[CandidateRef],
    winner: &CandidateRef,
) -> Result<Vec<CandidateRef>, SelectionError>;

/// Whether a candidate may be promoted. `quarantined` marks results whose measurement was
/// contaminated; those need `force`.
pub fn may_promote(
    candidate: &CandidateRef,
    quarantined: &[CandidateRef],
    force: bool,
) -> Result<(), SelectionError>;
```

## Required behaviour

- `resolve` returns `Union` only when both sides are insertions and the line sets are
  disjoint; identical lines on both sides are NOT a union case, they are already agreed and
  must escalate as a real conflict elsewhere in the region.
- `Union` output is deterministic: same input, same order, every time.
- `archive_ref` sanitises like a git ref: no spaces, no `..`, no leading or trailing `/` or
  `.`, no `~^:?*[\`, no control characters, and it must not end in `.lock`.
- `sweep_list` never includes the winner and never loses a candidate.
- `may_promote` refuses a quarantined candidate unless `force`.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: two `mod` lines union; a modification on one side
escalates; identical lines on both sides escalate rather than union; `Union` ordering is
stable across runs; five nasty inputs to `archive_ref`; `sweep_list` with the winner absent
errors; a quarantined candidate needs `force`.

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
