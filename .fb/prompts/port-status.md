<!-- fb:creates crates/fb/src/status.rs -->
<!-- fb:reads fb-status.sh -->
<!-- fb:differential fb-status.sh status -->
<!-- fb:case -->
# Task: port fb-status.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/status.rs` and declare it from `crates/fb/src/main.rs` with
`mod status;`. Do not change any other file, and do not delete the shell script.

Expose `pub fn run_cmd() -> i32` and wire a subcommand `fb status` to it. Keep every path it
reads injectable -- take them as arguments to the function that does the work -- so the logic
is testable against a fake tree without environment variables and without touching the real
logs.

This script REPORTS. Its stdout is the contract, line for line, because a human reads it and
because the autopilot greps it.

## Why this is being ported

This harness measures AI coding agents. Eleven of the sixteen defects found in it last week
were in its own shell scripts, and every repair that came BACK came back in bash: three
repairs each caused a second occurrence of the bug they fixed. Repairs that changed a *type*
in the Rust crate have not recurred once. Bash cannot express the distinction this harness
depends on most -- "measured zero" versus "not measured" -- because it has an empty string and
a `0` and nothing else.

So this is not a tidying exercise. The specification below is the existing script, and your
job is to reproduce what it does while making the states it confuses *unrepresentable*.
## The specification IS the script

It is reproduced in full at the bottom. Its behaviour is the contract, including its exit
codes and its stdout format, because other scripts parse both. Where a comment in the script
explains WHY something is the way it is, that reason is part of the contract -- those comments
record real incidents and the behaviour they describe must survive the port.
## Non-negotiable: distinguish absent from zero

The crate already has the types for this and you MUST use them rather than invent your own:

- `farmerbob_core::measurement::Measurement<T>` -- `Observed(T)` or `Missing(Absent)`, where
  `Absent` is `NotAttempted`, `InstrumentFailed`, `NothingToMeasure` or `Untrusted`. It has no
  `unwrap`, no `unwrap_or`, no `Default` and no `From<Option>`, deliberately.
- `farmerbob_core::gate::{judge, Observation, Verdict}` for any pass/fail verdict.

Wherever the script computes a number from a command that can fail, the Rust must return
`Measurement`, not a bare value with a sentinel. Every `|| echo 0`, every `2>/dev/null`
swallowing an error, every `${x:-0}` in the script below is a place where a failure currently
becomes a zero. Find them and make each one `Missing` with a stated reason.
## How this port will be judged

Your submission will be run against `fb-status.sh` on the live tree and the two outputs DIFFED,
line for line. That is an unusually strict oracle for a reporting tool and it is deliberate:
a report that is subtly wrong is worse than one that fails, because nothing announces it.

"It builds and my tests pass" is not evidence that a port does its job.

A submission that SIMULATES the work rather than doing it is an automatic failure, even when its
output is perfectly shaped. A previous port in this series shipped a file that never invoked the
tool it was meant to drive, emitted the script's exact format with the numbers hardcoded to zero,
and passed build, scope, lint and its own tests. Every other gate here is a gate on FORM and a
constant satisfies all of them.

If part of the job cannot be done in your environment, say so in plain words and return a
`Missing` carrying the reason. That is a CORRECT answer and is scored as one. Fabricating a
plausible number is not.

## Two failures this task is specifically exposed to

Both come from ports already adjudicated in this series. Neither was caught by building, linting
or unit tests, and both are cheap to avoid:

- **Reachability.** One arm wrote 462 good lines and never added `mod <name>;` to main.rs. The
  compiler never saw the file, everything passed, and the subcommand did not exist. Declare the
  module AND wire the subcommand, then run `./target/debug/fb status` yourself before you finish.
- **Paths that depend on where you were started.** One arm read a config by relative path where
  the script uses an absolute one; from any other directory it failed for every input. Resolve
  the repo and the log directory explicitly. Do not assume the current directory.

## Counting is where this script will bite you

It reports counts over files that may not exist. Every place the shell writes `2>/dev/null`,
`|| echo 0`, `${x:-0}` or `wc -l` on a glob that matched nothing is a place where an ERROR
becomes a ZERO. A task with no runs and a task whose log directory is unreadable must not
produce the same number. That distinction is the entire reason this port exists.

## The script

```bash
#!/usr/bin/env bash
# fb-status — everything the orchestrator needs to decide what to do next, in one call.
# Written because two hours were lost to an idle machine while finished results sat unread.
set -uo pipefail
. /home/gabe/Documents/farmerbob/fb-target.sh
cd /home/gabe/Documents/farmerbob
LOGS="$HOME/.local/share/farmerbob/logs"

live=$(systemctl --user list-units --type=scope --no-legend 2>/dev/null \
       | grep -o 'fb-[a-z0-9-]*--[a-z0-9_-]*' | sed 's/^fb-//' | sort -u)
nlive=$(printf '%s' "$live" | grep -c . || true)
waves=$(pgrep -cf "fb-admit\.sh" 2>/dev/null | head -1); waves=${waves:-0}

echo "== live =="
if [ "$nlive" -eq 0 ]; then echo "  no agents running (dispatcher processes: $waves)"
else printf '%s\n' "$live" | sed 's/^/  /'; fi

echo "== tasks with results, by adjudication state =="
for s in "$LOGS"/*.score.json; do
  [ -f "$s" ] || continue
  t=$(basename "$s" .score.json)
  # a task is merged when its declared file exists on master
  f=$(fb_target "$t"); verb=$(fb_target_verb "$t")
  # A task is not adjudicable until the SUBJECTIVE tier has run. The critique/promote/prove
  # stages silently stopped happening for 15 tasks when the parallel wave path replaced
  # `fb trial`, and nothing reported their absence because the objective numbers kept
  # flowing. A missing stage must be visible here.  (bead farmerbob-k9f)
  #
  # An explicit adjudication record wins over any inference. A task can be finished WITHOUT
  # a merge -- probe's twelve candidates were all Indeterminate because git had lost their
  # worktrees, so there was no winner to merge and never will be. Inferring state from the
  # filesystem had no way to say that, and the board went on demanding adjudication of a
  # task already adjudicated.  (bead farmerbob-13p)
  if [ -f ".fb/adjudicated/$t" ]; then
    state="$(head -1 ".fb/adjudicated/$t")"
  # "the declared file exists" is evidence of a merge only for a `creates` task. For a
  # `modifies` task the file exists BEFORE any work is done, so existence proves nothing
  # and would report MERGED for every unstarted edit task.
  elif [ "$verb" = creates ] && [ -n "$f" ] && [ -f "$f" ]; then state="MERGED"
  elif [ ! -f "$LOGS/$t.claims.json" ]; then state="** NEEDS CRITIQUE **"
  else state="** NEEDS ADJUDICATION **"; fi
  n=$(python3 -c "import json;d=json.load(open('$s'));print(sum(1 for r in d if r.get('verdict')=='PASS'))" 2>/dev/null || echo '?')
  printf '  %-16s %-26s %s passing\n' "$t" "$state" "$n"
done

echo "== queue =="
for p in .fb/prompts/*.md; do
  b=$(basename "$p" .md); case "$b" in _*) continue ;; esac
  [ -f "$LOGS/$b.score.json" ] && continue
  # A `creates` spec whose target ALREADY EXISTS is dead: dispatch's precondition rejects it,
  # or worse it runs and every arm correctly no-ops, measuring nothing. Merging a winner is
  # what kills them -- bandit-route was authored against a base where router.rs did not exist
  # and has been unrunnable ever since router.rs was merged. Listing it as available work
  # invites spending four arms on a guaranteed no-op.  (bead farmerbob-m71)
  tgt=$(fb_target "$b" 2>/dev/null)
  if [ "$(fb_target_verb "$b" 2>/dev/null)" = creates ] && [ -n "$tgt" ] && [ -f "$tgt" ]; then
    echo "  unspent spec: $b   ** STALE: $tgt already exists, a creates-task would no-op **"
  else
    echo "  unspent spec: $b"
  fi
done
ls .fb/wave*.tsv 2>/dev/null | while read -r w; do
  n=$(awk 'NF' "$w" | wc -l)
  done_n=0
  while IFS=$'\t' read -r t _ _; do [ -f "$LOGS/$t.score.json" ] && done_n=$((done_n+1)); done < "$w"
  printf '  %-18s %s/%s tasks scored\n' "$(basename "$w")" "$done_n" "$n"
done

echo "== bd ready (leaf tasks only) =="
bd ready 2>/dev/null | grep -v '\[epic\]' | head -8 | sed 's/^/  /'
```

## Rules

- Public entry point: `pub fn run_cmd(...) -> i32`, returning the process exit code, with
  arguments matching the script's positional parameters in order.
- **Wire it as an `fb` SUBCOMMAND in `main.rs`** -- a `Command` variant with the script's
  parameters as arguments, and a dispatch arm calling `run_cmd`. A module that only declares
  `mod x;` compiles, passes its own tests, and is unreachable from the binary, which is the
  exact condition this whole migration exists to end.

  This requirement is here because two critics found its absence independently, reviewing
  different subjects: "the module is shipped as `#[allow(dead_code)] mod crossx;` with no
  subcommand wired -- the feature is unreachable except by calling `run_cmd` directly, a merge
  hazard", and "the implementation can compile and have unit coverage while remaining
  inaccessible to users of the public `fb` interface". An earlier version of this spec asked
  only for the module declaration, so every candidate complied and every candidate was
  unreachable. The defect was mine.
- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Available deps in `crates/fb`: farmerbob-core, clap, anyhow, serde, serde_json, toml,
  directories, rusqlite. Add none.
- `cargo build` and `cargo test` must pass for the whole workspace. Run them yourself.
- Do not make parameters generic. Concrete types only.

When done, briefly state what you implemented and which failure-becomes-zero paths you found.
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
