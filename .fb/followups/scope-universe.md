# Follow-up turn: scope-universe

Your implementation was reviewed and **no material defects were found**. It scored PASS,
852 tests, clippy clean. Two follow-ups came out of the review. Both are small and both are
in files you already own. Do them in this same worktree.

## 1. `deleted_paths` must filter harness-owned paths at the source

`arm_changed` filters harness-owned paths from the changed-path list. `deleted_paths` lost
its `crates/` pathspec and gained no harness filter, so it now returns harness-provisioned
deletions like `CLAUDE.md` and `.beads/` entries.

This causes no wrong behaviour today, because `gone.contains(p)` is only ever asked about
paths that already came out of `arm_changed`, which excluded them. That is exactly why it
needs fixing now: the two functions apply different filtering strategies to the same
universe of git output, one at the producer and one at the consumer, and the invariant that
makes it safe is written down nowhere. A future caller of `deleted_paths` that does not
chain through `arm_changed` inherits the wrong set silently -- the same class of defect
(farmerbob-3owl) this task exists to close.

Apply `harness_owned` filtering inside `deleted_paths` so both path-discovery functions
exclude harness provisioning at the source.

FILE: crates/fb/src/score.rs

## 2. `harness_owned` needs direct unit tests

`harness_owned` is the single decision this task introduces, and it is tested only
indirectly through `assess_arm`, which shells out to git and cannot run without a real
repository. Spec clauses 1-4 are therefore pinned at the level of `evaluate_verdict` and
`assess`, not at the predicate. A mutation that flips the `.fb/handoff.md` exception, or
drops an entry from `HARNESS_OWNED`, would not be caught by the current suite.

Add direct unit tests for `harness_owned` covering clauses 1-4 and the boundaries:
the empty string, bare `.fb`, bare `.beads`, and a `./` prefix.

FILE: crates/fb/src/scope_cmd.rs

## Rules

Change only those two files. Keep the workspace rustfmt-clean and clippy-clean, and keep
every existing test passing. Do not restructure anything the review did not ask about.
