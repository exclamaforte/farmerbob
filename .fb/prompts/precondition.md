<!-- fb:creates crates/farmerbob-core/src/precondition.rs -->
# Task: a queued spec stops being runnable the moment a winner merges

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.
Create `crates/farmerbob-core/src/precondition.rs` and add `pub mod precondition;` to
`crates/farmerbob-core/src/lib.rs`. Do not change any other file.

## What this is for

A task spec declares what it will touch, in an HTML comment at the top of the prompt:

    fb:creates crates/farmerbob-core/src/wtreap.rs      must NOT exist on the base
    fb:modifies crates/farmerbob-core/src/quota.rs      MUST exist on the base

Each is written inside an HTML comment: `<`, `!`, `--`, a space, the marker, a space, `--`,
`>`. This document deliberately does not spell that out anywhere below, because the FIRST
LINE of this file is a real declaration and the dispatcher greps the whole prompt. An
earlier draft printed the two examples above in their literal form and every arm refused
the task -- `declares creates:crates/farmerbob-core/src/wtreap.rs but it already exists on
the base` -- because the example was parsed as a declaration. Line 1 is the only literal
marker here; read it for the exact shape.

Specs are written days before they run and sit in a queue while other waves merge. A merge
moves the base under everything still queued. Both directions have already cost real dispatch:

- **Declared `modifies`, file absent.** wave58 declared `fb:modifies` for a file the spec
  itself was asking to create. Four arms, four `TASK-INVALID`, eleven seconds, four slots.
- **Declared `creates`, file now present.** A spec queued before the wave that creates the
  same file is invalid by the time it is launched, and the arm cannot tell whether it is
  supposed to write the file or the one already there.

The check exists today in `fb-dispatch.sh` as a `grep -oE` over the prompt and a `[ -f ]`
per path. It cannot be tested, and it is the same line-matching shape that has now produced
three separate bugs by matching a pattern inside a quotation of itself.

## Exact API

```rust
/// Which side of the base a declared path must be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// `fb:creates` -- the path must NOT exist on the base.
    Absent,
    /// `fb:modifies` -- the path MUST exist on the base.
    Present,
}

/// One declared path and what the spec requires of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// Repo-relative path, exactly as written in the prompt.
    pub path: String,
    /// What the marker requires.
    pub requirement: Requirement,
}

/// Every declaration in a prompt, in the order they appear.
pub fn declarations(prompt: &str) -> Vec<Declared>;

/// Whether a queued spec can still run against a base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// Every declaration holds.
    Satisfied,
    /// At least one does not. Every violation, in declaration order.
    Violated(Vec<Violation>),
    /// The prompt declares nothing, so nothing can be checked. NOT `Satisfied`:
    /// an unchecked spec and a checked one are different states and must not
    /// report the same value.
    Undeclared,
}

/// One declaration that does not hold against the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The declared path.
    pub path: String,
    /// What the spec required.
    pub required: Requirement,
    /// Whether the path is actually on the base.
    pub exists: bool,
}

/// Check a prompt's declarations against the paths present on the base.
///
/// `present` is the set of repo-relative paths that exist. Membership is an EXACT
/// string match; no normalisation, no prefix matching.
pub fn check(prompt: &str, present: &[&str]) -> Precondition;
```

## Falsifiable clauses

1. `declarations` over a prompt whose only line is a `fb:creates a/b.rs` marker is one `Declared { path: "a/b.rs",
   requirement: Absent }`. `fb:modifies` gives `Present`.
2. A prompt with both markers returns both, **in the order they appear in the text**, not
   grouped by requirement and not sorted.
3. `check` on a `fb:creates` path that IS in `present` yields `Violated` with one violation
   carrying `required: Absent, exists: true`.
4. `check` on a `fb:modifies` path that is NOT in `present` yields `Violated` with
   `required: Present, exists: false`. This is wave58 exactly.
5. A prompt with no marker yields `Undeclared`, and `declarations` returns an empty vector.
   **`Undeclared` is not `Satisfied`.** Say in the doc why: a spec nobody can check and a
   spec that passed its check are different states, and collapsing them is a check whose
   failure is indistinguishable from its success.
6. **A marker is recognised only at the start of a line**, after optional leading
   whitespace. A prompt that writes a `fb:creates x.rs` marker mid-sentence, after other words on the same line, declares
   nothing. Pin this. It is why the rule exists: three separate bugs in this project came
   from a scanner matching its own syntax quoted inside prose, and every prompt carries an
   appended rubric that discusses these markers by name.
7. **A marker inside a fenced code block is not a declaration.** A fence is a line whose
   trimmed text starts with three or more backticks; fences toggle. A marker between an
   opening and closing fence is documentation of the syntax, not a use of it. Pin both the
   ignored-inside case and the recognised-after-the-close case.
8. The path is the text between the marker word and ` -->`, trimmed. A marker with no path
   -- a `fb:creates` marker with nothing between it and the closing `-->` -- declares nothing and is not an error.
9. `Violated` lists EVERY violation, not the first. A spec that is wrong in two ways should
   say so once, not across two dispatches.

## Boundaries, at N and at zero

- An EMPTY prompt: `declarations` is empty and `check` is `Undeclared`.
- An empty `present` against a prompt declaring only `fb:creates`: `Satisfied`. Nothing
  exists, so nothing that must be absent is present. Pin this; it is the cold-start case
  and the obvious wrong answer is to treat an empty base as unknowable.
- An empty `present` against a prompt declaring any `fb:modifies`: `Violated`.
- The SAME path declared twice with the SAME requirement: two entries from `declarations`
  (it reports what is written) and, when violated, two violations. State this and pin it.
- The same path declared `fb:creates` AND `fb:modifies`: both are returned, and against any
  base exactly one of them is violated, because the path either exists or does not. Pin
  that the result is `Violated` for every possible `present`. This is a spec that cannot be
  satisfied, and saying so is more useful than picking a winner between the two markers.
- A path that is the empty string after trimming: clause 8, not a declaration.

## Superset status on every enumerated list

The MARKER WORDS are a CLOSED set of exactly two, `fb:creates` and `fb:modifies`. Anything
else after the comment opener and `fb:` -- including a future third marker -- is not a declaration and is
ignored, not an error: an older binary meeting a newer prompt must not refuse it, and must
not silently treat it as one of the two it knows. Say both halves in the doc.

`Requirement` and `Precondition` are CLOSED. The set of PATHS is open and nothing may key
off known file names.

## Composition of aggregate returns

`declarations` returns document order with duplicates preserved; it is a transcript of the
prompt, not a set. `Violated`'s vector is in declaration order, and is never empty --
`Violated(vec![])` must be unconstructible in practice, so state that `check` never returns
it and pin a test that `Satisfied` is what an all-holding prompt returns.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input,
  outside `#[cfg(test)]`.
- Add no dependencies. **No I/O and no `std::fs`**: the set of present paths arrives as an
  argument. A function that stats the filesystem cannot be tested against a base that does
  not exist yet, which is the entire situation this module is about.
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
