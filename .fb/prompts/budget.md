<!-- fb:creates crates/farmerbob-core/src/budget.rs -->
# Task: admission arithmetic that accounts for what the machine is actually holding

Create `crates/farmerbob-core/src/budget.rs`, declare it from `lib.rs` with
`pub mod budget;`. Change nothing else except that one `pub mod` line.

## Context

Admission control decides how many agents may run at once. Getting it wrong in one direction
overcommits the machine; getting it wrong in the other wastes capacity that is sitting idle.
This project has done both.

The subtlety that caused real damage: `free` reports *available* memory, and on this machine
`/tmp` is a **tmpfs** — so files written there consume RAM and reduce that figure without
being any process's resident set. 4.6GB of dead scratch directories once made the admission
controller grant fewer slots than the machine could support, and nothing in its arithmetic
could see why. A budget that models only per-process caps is blind to the storage its own
tooling produces.

The second subtlety: a *limit* is not a *reservation*. systemd will happily accept ten scopes
each capped at 2GB on a machine with 8GB free. The cap contains an overrun inside one scope;
it does nothing about the sum.

**Pure logic: no I/O, no reading /proc.** The caller supplies observations.

## Exact API — implement these signatures verbatim

```rust
/// What the machine currently looks like. All figures in mebibytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    pub total_mib: u64,
    /// As reported by the OS. Already reduced by tmpfs contents.
    pub available_mib: u64,
    /// Bytes held in tmpfs mounts, which are RAM but belong to no process.
    pub tmpfs_mib: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Budget reserved per admitted run.
    pub per_slot_mib: u64,
    /// Held back for the orchestrator and one verification cycle.
    pub headroom_mib: u64,
    /// Never admit more than this many, whatever the arithmetic says.
    pub max_slots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Slots that may be granted right now.
    Grant { slots: usize },
    /// Nothing may be admitted, with the binding reason.
    Deny { reason: String },
}

/// How many slots the machine can support. `running` is the count already admitted.
pub fn admit(m: &Machine, p: &Policy, running: usize) -> Admission;

/// Memory that could be recovered by clearing tmpfs. This is the number that explains a
/// surprising denial, and it must be reported rather than silently absorbed.
pub fn reclaimable_mib(m: &Machine) -> u64;

/// The SUM of budgets already committed, which is what actually bounds the machine --
/// not any individual cap.
pub fn committed_mib(p: &Policy, running: usize) -> u64;

/// True when the policy could never admit even one slot on this machine, which is a
/// misconfiguration rather than a busy machine and must be distinguishable from one.
pub fn policy_is_impossible(m: &Machine, p: &Policy) -> bool;
```

## Required behaviour

- `admit` grants `min(max_slots - running, (available - headroom) / per_slot)`, floored at 0.
- A `per_slot_mib` of 0 is a misconfiguration: `Deny` naming it, never an infinite grant.
- `Deny` when `available_mib <= headroom_mib`, and the reason must state the shortfall in
  mebibytes.
- When `tmpfs_mib` exceeds `reclaimable_threshold` of 1024, a `Deny` reason must ALSO
  mention how much is reclaimable, because an operator seeing "out of memory" while 4GB of
  scratch files sit in tmpfs will draw the wrong conclusion.
- `running >= max_slots` is `Grant { slots: 0 }`, not `Deny`: the machine is busy, not
  broken, and those are different facts.
- `policy_is_impossible` is true when `headroom_mib + per_slot_mib > total_mib`.
- `committed_mib` saturates rather than overflowing.
- All arithmetic is checked or saturating; no underflow when headroom exceeds available.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: a full tmpfs produces a Deny that mentions
reclaimable memory; a busy machine grants 0 rather than denying; per_slot of 0 denies rather
than granting infinity; headroom above available does not underflow; an impossible policy is
distinguishable from a busy machine; committed_mib saturates.

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
