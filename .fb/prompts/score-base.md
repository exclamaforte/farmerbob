<!-- fb:modifies crates/fb/src/score.rs -->
# Task: an arm that commits its work is recorded as having done nothing

Rust workspace, already builds. Work only inside `crates/fb`.
Modify `crates/fb/src/score.rs`. Do not change any other file.

## Why this exists

Every line count in the scorer is taken against the worktree's own `HEAD`:

```rust
let numstat = git(wt, &["diff", "--numstat", "HEAD", "--", "crates/"]);
let tracked = git(wt, &["diff", "--name-only", "HEAD", "--", "crates/"]);
```

That measures UNCOMMITTED changes. An arm that commits its work leaves a clean tree, the
numstat is empty, and the run is recorded as `lines_added: 0` -- a NO-OP, with rc=0, a passing
build and a passing suite.

Yesterday glm-53-flash spent nineteen minutes on `partition-truth` and reported:

    Implementation complete. The work is committed as `bec2990` on
    `fb/partition-truth/glm-53-flash`, changing exactly one file:
    `crates/fb/src/crossx.rs` (+229/-100).

and the harness recorded `NO-OP rc=0 1136s +0 lines`. The work was real: 229 insertions, 100
deletions, verifiable with `git diff master` in that worktree. It was recovered by hand.

A genuine no-op and a committed implementation currently produce byte-identical scores. Doing
the ordinary thing with git makes the work invisible. (bead farmerbob-qv6u)

## Exact API

One private helper is added and the three `HEAD` call sites use it. Nothing public changes.

```rust
/// The revision a worktree's work should be measured against: the point its
/// branch left the base, so that committed and uncommitted work both count.
///
/// `None` when git refuses to answer, which is NOT the same as "no commits".
fn measure_base(wt: &Path) -> Option<String>;
```

`measure_base` returns the merge-base of `HEAD` and `master` -- `git merge-base HEAD master` --
and falls back to the string `"HEAD"` when that command SUCCEEDS but prints nothing, which is
what a worktree with no common ancestor does. It returns `None` only when git refuses.

The three call sites become, with `base` the value `measure_base` returned:

```rust
let numstat = git(wt, &["diff", "--numstat", &base, "--", "crates/"]);
let tracked = git(wt, &["diff", "--name-only", &base, "--", "crates/"]);
```

`git(dir, args) -> Option<String>` already exists and already distinguishes refusal from empty
output. Use it; do not add a second way to run git.

## Falsifiable clauses

1. A worktree whose work is UNCOMMITTED scores exactly as it does today. The merge-base of
   `HEAD` and `master` is `HEAD` itself when no commit has been made on the branch, so the diff
   is unchanged. Pin that the two are equal for the uncommitted case.
2. A worktree whose work is COMMITTED on its branch scores the same `lines_added` as the
   identical work left uncommitted. This is the defect; pin it as an equality between two
   arrangements of the same content, not as a fixed number.
3. A worktree with BOTH a commit and further uncommitted edits counts both, once each. Pin that
   the total equals the sum, and that no line is counted twice.
4. `measure_base` returns `None` when git refuses, and the existing rule then holds unchanged:
   `lines_added` is `Measurement::Missing`, never `Some(0)`. The doc comment on `git` at the top
   of this file explains what that rule cost when it was got wrong; do not weaken it.
5. `changed_paths` and the crate set are computed from the same base as the line count. A file
   changed in a commit is a changed path. Pin one case where a committed change is in scope and
   one where it is out of scope, because the scope gate reads this list.
6. Nothing public in this module changes signature.

## Boundaries, at N and at zero

- ZERO commits on the branch and a clean tree: `lines_added: Some(0)`, a genuine no-op. This is
  the case that must still work, and it is the one the fix could break.
- ZERO commits and a dirty tree: unchanged from today.
- ONE commit and a clean tree: the committed lines. The defect case.
- A branch with no common ancestor with `master`: `git merge-base` succeeds and prints nothing,
  so the base is `"HEAD"` and behaviour falls back to today's. Say so rather than treating an
  empty answer as a refusal.
- Git refusing (the pruned `.git/worktrees/<name>` case, 103 of 181 worktrees historically):
  `Missing`. Clause 4.

## Superset status on every enumerated list

The git subcommands this module may run are a CLOSED set of exactly four: `diff --numstat`,
`diff --name-only`, `ls-files --others --exclude-standard`, and `merge-base`. Adding a fifth is
a defect, and your tests may not assert on any other git invocation.

`Measurement` and `Absent` are `farmerbob_core::measurement`'s and you define neither.

## Composition of aggregate returns

`lines_added`, `changed_paths` and the crate set are three readings of ONE diff and must be
taken against ONE base. A count from the merge-base beside a path list from `HEAD` would let a
file be counted and not listed, which is how a scope verdict and a line count come to disagree
about the same run.

Say why clauses 1 and 2 must be tested as a PAIR: clause 2 alone passes against an
implementation that always diffs against `master` and so counts another arm's merged work as
this arm's, and clause 1 alone passes against today's broken code. Only together do they pin
the merge-base.

## Rules

- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` reachable from input, outside
  `#[cfg(test)]`.
- Add no dependencies.
- Tests that need a git repository must CREATE one in a temporary directory and `git init` it.
  Do not read this repository's own worktrees; they are mutable state shared with a running
  harness.
- `cargo test -p fb` must pass.
- **The clippy criterion is NOT MEASURABLE on this crate and no candidate will be ranked by it.**
  `cargo clippy -p fb -- -D warnings` fails on errors that predate this task, fixing them is out
  of scope, and the bar is therefore unreachable for everyone equally. Report that your own file
  adds none. (bead farmerbob-hm4j)

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- **you change the file the task declares and nothing else** -- plus, ONLY when the task
  CREATES a new file, the one `pub mod y;` line in the `lib.rs` beside it. A task that MODIFIES
  an existing file touches one file and no other. Not "only that crate" --
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
