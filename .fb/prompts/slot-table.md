<!-- fb:creates crates/farmerbob-core/src/slots.rs -->
# Task: implement slot admission control

Create `crates/farmerbob-core/src/slots.rs`, declare it from `lib.rs` with `pub mod slots;`.
Change nothing else.

## Context

farmerbob runs agents concurrently on one machine. Each occupies a slot carrying a resource
budget. The hard lesson this encodes: **a per-run memory cap is not admission control.**
systemd will happily accept six scopes whose limits sum to more than the machine has, because
no single one violates its own cap — and the machine dies while every run is "within budget".

Two further facts that shape the API. Budget against **available** memory, not total: a desktop
has an irreducible resident set, so assuming a dedicated machine over-commits. And leave
headroom for the orchestrator's own work — an orchestrator that confines its children while
its own verification runs unbounded has just moved the leak.

**Pure logic: no I/O, no clock.** Memory figures are passed in.

## Exact API — implement these signatures verbatim

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SlotIndex(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunRef(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Budget {
    pub memory_mb: u64,
    pub cpu_quota_percent: u32,
    pub tasks_max: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Admission {
    Admitted(SlotIndex),
    /// Refused with the reason, which must name the numbers so a human can act on it.
    Queued { reason: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Machine {
    /// Currently available memory, not total.
    pub available_mb: u64,
    /// Reserved for the orchestrator and its own verification work.
    pub headroom_mb: u64,
}

#[derive(Debug, Default)]
pub struct SlotTable { /* your fields */ }

impl SlotTable {
    pub fn new(budget: Budget) -> Self;
    /// Admit `run` if the SUM of active budgets plus this one still fits under
    /// `available_mb - headroom_mb`. Otherwise queue it with a reason.
    pub fn admit(&mut self, run: RunRef, machine: &Machine) -> Admission;
    /// Free the slot held by `run`. Unknown runs are a no-op.
    pub fn release(&mut self, run: &RunRef) -> Option<SlotIndex>;
    pub fn active(&self) -> usize;
    pub fn committed_mb(&self) -> u64;
    /// How many slots this machine supports right now, from measured figures.
    pub fn capacity(&self, machine: &Machine) -> usize;
    /// Which run holds a slot, if any.
    pub fn holder(&self, slot: SlotIndex) -> Option<&RunRef>;
}
```

## Required behaviour

- Admission is bounded by the **sum** of active budgets, never by slot count alone.
- `capacity` is `(available_mb - headroom_mb) / budget.memory_mb`, saturating at zero when
  headroom exceeds available. It must never divide by zero.
- Slot indices are reused: releasing slot 1 and admitting again gives slot 1, not slot 3.
- Admitting the same `run` twice returns the slot it already holds rather than a second one.
- A `Queued` reason names the committed total, the request, and the ceiling.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- No overflow or divide-by-zero on any input, including `memory_mb == 0` and
  `headroom_mb > available_mb`.
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass.

## Tests you must write

One per rule, named after it. At minimum: six runs at 6G each on an 18G machine do not all
admit; `capacity` is zero rather than a panic when headroom exceeds available;
`memory_mb == 0` does not divide by zero; slot indices are reused after release; admitting
the same run twice yields one slot.

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
