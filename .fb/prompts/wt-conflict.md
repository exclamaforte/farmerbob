<!-- fb:modifies crates/farmerbob-core/src/wtreap.rs -->
# Task: a listing that contradicts itself must not authorise a deletion

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Modify `crates/farmerbob-core/src/wtreap.rs`. Do not change any other file.

## The gap, found by a critic

`wtreap` decides which worktree directories may be deleted. It requires positive evidence on
both sides: unregistered, not live, and settled. Unregistered-and-not-settled is
`Indeterminate`, never `Reapable`, because a forgotten finished run and a run whose record we
lost produce the identical directory.

The spec pinned duplicates only for IDENTICAL entries: the same `dir` twice with the same
`registered` flag counts twice in `census` and appears once in `reapable`.

It said nothing about the same `dir` twice with CONTRADICTORY flags. Classification is per
entry, and `reapable` is pinned to return "exactly the dirs `disposal` calls `Reapable`", so a
listing where one observation says `registered: true` and another says `registered: false`
yields a name in `reapable` that git lists -- a directory unsafe to delete.

That is the one state this module exists to be careful about, and here the uncertainty is IN
THE INPUT rather than in a missing record. The module's own doctrine answers it: absence of
agreement is not evidence.

## The ruling

A `dir` appearing more than once with contradictory `registered` flags is `Indeterminate`,
whatever `live` and `settled` say, and is never `Reapable`.

## Exact API

Keep `Disposal`, `Worktree`, `split_key`, `disposal`, `reapable`, `Census` and `census` with
their current signatures and meanings. `disposal` still classifies ONE entry and does not
change. Add:

```rust
/// A directory name observed more than once with disagreeing registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The directory name that was observed inconsistently.
    pub dir: String,
    /// How many entries said git lists it.
    pub registered: u32,
    /// How many said it does not.
    pub unregistered: u32,
}

/// Every directory whose entries disagree about registration, sorted by
/// `dir`, without duplicates.
pub fn conflicts(wts: &[Worktree<'_>]) -> Vec<Conflict>;

/// `reapable`, with contradictory listings excluded.
///
/// This is what a caller that DELETES should use. `reapable` is kept
/// unchanged because it is the honest answer to "what did `disposal` say",
/// and the two differ exactly on the conflicts.
pub fn safe_to_reap(wts: &[Worktree<'_>], live: &[&str], settled: &[&str]) -> Vec<String>;
```

## Falsifiable clauses

1. A `dir` appearing twice, once `registered: true` and once `false`, appears in `conflicts`
   with `registered: 1, unregistered: 1`.
2. That same `dir` NEVER appears in `safe_to_reap`, whatever `live` and `settled` contain.
3. A `dir` appearing twice with the SAME flag is NOT a conflict. Identical duplicates were
   already pinned and stay pinned: counted twice by `census`, listed once by `reapable`.
4. `reapable` is UNCHANGED by this task. Pin a case where `reapable` contains a name and
   `safe_to_reap` does not, and say in the doc why both exist rather than replacing one.
5. `safe_to_reap` is a subset of `reapable`, always. Pin it as a property over a mixed listing,
   not only as prose.
6. A `dir` appearing three times, two `true` and one `false`, is a conflict with
   `registered: 2, unregistered: 1`. **Majority does not resolve it.** Say why: two observations
   agreeing is not evidence that the third was wrong, and deleting a live worktree on a 2-1
   vote is the outcome this module exists to prevent.
7. `conflicts` reports a directory whose name does not split on `--` exactly as it reports any
   other. Nameability and registration-agreement are independent, and a conflict in an
   unnameable dir is still a conflict.

## Boundaries, at N and at zero

- An EMPTY listing: `conflicts` empty, `safe_to_reap` empty.
- A listing with NO conflicts: `safe_to_reap` equals `reapable` exactly. Pin the equality.
- A listing where EVERY dir conflicts: `safe_to_reap` is empty and `conflicts` has one entry
  per distinct name.
- A `dir` appearing once: never a conflict. One observation cannot disagree with itself.
- `Conflict::registered + Conflict::unregistered` equals the number of entries carrying that
  name, and both fields are non-zero by construction. Say why each cannot be zero.

## Superset status on every enumerated list

`Disposal` keeps its CLOSED four variants; this task adds none. A conflict is not a fifth
disposal -- it is a property of the LISTING, not of a directory's state, and modelling it as a
`Disposal` variant would force `disposal`, which sees one entry, to report something only
visible across several. Say so in the doc.

## Composition of aggregate returns

`conflicts` is sorted by `dir`, one entry per distinct name, never containing a name that
appears fewer than twice. `safe_to_reap` is sorted, deduplicated, and a subset of `reapable`;
an empty result is a real answer meaning nothing is safe to delete, which is different from
"nothing is reapable" and should be said in the doc.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies. **No I/O and no `std::fs`**: this module never reads a directory and
  never deletes anything. It decides; the caller acts.
- Do not change `disposal`, `reapable`, `census` or `split_key`.
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
