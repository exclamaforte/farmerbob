# Follow-up turn: critic-first-class

Scored PASS, +4 tests, clippy clean, 230 lines. gemini-38-flass reviewed it and filed one
CLAIM and two design judgements. Your worktree and session are intact; continue from there.

## 1. My spec contradicted itself, and you implemented the half I wrote second

Clause 7 said a target that does not exist "contributes a NAMED absence to the
concatenation". The Composition section said return `Missing` if any target "could not be
read". You implemented the second. The critic cited the first. Both were faithful to a spec
that disagreed with itself, and that is my fault, not yours.

The rule, now corrected in the spec:

- A declared target that **does not exist** contributes a named absence and the result stays
  `Observed`. That a file was never written is a fact about the candidate and is exactly
  what a critic needs to see. Refusing to show the rest because of it hides the work that
  WAS done.
- A declared target that exists but **cannot be read** — git refused, permissions, a broken
  worktree — makes the whole result `Missing`. That is an instrument failure, and a partial
  view presented as a whole is the defect this task is about.

Implement that split and pin both directions.

## 2. An unmodified target dumps the whole file at the critic

The critic's second judgement, and it is right: when a target exists, is tracked, and has no
diff, `patch_for`'s empty-diff fallback treats it as `=== NEW FILE ===` and emits the entire
contents. In a multi-deliverable task that bloats the patch the critic reads.

An unmodified tracked file should say so in one line, not reproduce itself.

## 3. Four of seven clauses have no test

You pinned zero-timeout, refusal-vs-decline rendering, empty target sets, and
JUDGEMENTS-without-CLAIMS. Missing:

- clause 1 — `run_critic` passes `Some(Scope { .. })` and `runtime_max_s` equals the
  `timeout_s` given. This is the whole point of the task; it must not be able to regress
  silently.
- clause 2 — a timeout yields `TimedOut`, not `Declined`.
- clause 3 — a provider refusal yields `Refused` carrying the evidence verbatim.
- clause 6 — two targets concatenate, in declared order, each under a header naming its path.

## What NOT to do

The critic's judgement about systemd SIGTERM (exit 143, empty output under `--quiet`)
misclassifying a timeout as `NotLaunched` is correct and is NOT yours to fix: it needs
`launch.rs` to report a termination cause instead of a diagnostic string, and `launch.rs` is
outside your declared scope and is being changed by another task right now. Leave the
substring matching as it is. I have filed it.

Likewise leave `scope_cmd.rs` alone; its `.next()` bug is filed separately.

## Rules

Change only `crates/fb/src/critique.rs` and `crates/fb/src/adjudicate_cmd.rs`. Keep the
workspace rustfmt-clean and clippy-clean and every existing test passing. Run
`timeout 120 cargo test -q -p fb` while you work.
