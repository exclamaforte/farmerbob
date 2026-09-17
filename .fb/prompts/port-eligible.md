<!-- fb:creates crates/fb/src/eligible.rs -->
<!-- fb:reads fb-eligible.sh -->
<!-- fb:differential fb-eligible.sh eligible -->
<!-- fb:case or-hy3 -->
<!-- fb:case or-mercury-25 -->
<!-- fb:case or-gemini-38-flash -->
<!-- fb:case claude-sonnet -->
<!-- fb:case no-such-arm -->
# Task: port fb-eligible.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/eligible.rs` and declare it from `crates/fb/src/main.rs` with
`mod eligible;`. Do not change any other file, and do not delete the shell script.

Expose `pub fn run_cmd(arm: &str) -> i32` returning the script's exit code, and wire a
subcommand `fb eligible <arm>` to it. Keep the registry path injectable -- take it as an
argument to the function that reads it -- so the logic is testable against a fake
`sources.toml` without touching the real one or the environment.

This script is SOURCED by the dispatcher and used as a predicate. Its exit code is the answer
(0 dispatchable, 1 not) and its stderr is the reason a human reads. Both are the contract.

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

Read this part carefully. It decided the last port in this series.

Your submission will be run against `fb-eligible.sh` ON REAL INPUT -- every arm in the live
`sources.toml`, plus names absent from it -- and the two outputs DIFFED. Exit code and the
exact stderr text are both part of the contract, because the dispatcher branches on the first
and a human reads the second.

"It builds and my tests pass" is not evidence that a port does its job. A port has an oracle
that almost nothing else in this project has -- the script it replaces -- and yours will be
run and diffed before it is considered.

A submission that SIMULATES the work rather than doing it is an automatic failure, even when
its output is perfectly shaped. The previous port in this series produced a 311-line file that
never invoked the tool it was supposed to drive, emitted the script's exact output format with
its numbers hardcoded to zero, and passed build, scope, lint and its own three tests. Every
other gate in this harness is a gate on FORM, and a constant satisfies all of them.

If some part of the job cannot be done in your environment, say so in plain words and return a
`Missing` carrying that reason. Returning `Missing` with a stated reason is a CORRECT answer
here and is scored as one. Fabricating a plausible value is not, and it is the single failure
this project is least able to detect and most damaged by.

## Watch for these in the script below

- The rate-limit check parses a reset timestamp. A timestamp that cannot be parsed must not
  silently become "not rate limited" -- that is the bug this whole harness keeps having.
- `status` is a small closed set. Model it as an enum, not a string, so an unrecognised status
  is a value you must handle rather than one that falls through a comparison.
- An arm missing from the registry and an arm present but disabled are DIFFERENT answers with
  different stderr. Do not collapse them.

## The script

```bash
# fb-eligible — refuse to dispatch an arm that must not be used. Sourced by the dispatcher.
#
# Every dispatch in this project was cost-blind: matrices were hand-written lists of arm
# names and fired, with nothing consulting price. That paid for Gemini-3.8-Flash and
# GLM-5.3-Flash through OpenRouter while IDENTICAL models were available free on a plan --
# and on verify-router the free agy route PASSED with 867 lines while the paid twin no-opped.
#
# The router's `- dollar_weight*cost` term exists to prevent exactly this. Until the router
# is doing the dispatching, this is the guard.   (bead farmerbob-15p)
#
#   fb_eligible <arm>  -> 0 if dispatchable, 1 otherwise (reason on stderr)
fb_eligible() {
  local arm="$1" repo=/home/gabe/Documents/farmerbob
  python3 - "$arm" "$repo/sources.toml" <<'PY'
import sys, tomllib
arm, path = sys.argv[1], sys.argv[2]
d = tomllib.load(open(path, 'rb'))['source']
v = d.get(arm)
if v is None:
    print(f"{arm}: not in the registry", file=sys.stderr); raise SystemExit(1)
st = v.get('status')
if st == 'disabled':
    print(f"{arm}: disabled -- {v.get('disabled_reason','no reason recorded')}", file=sys.stderr)
    raise SystemExit(1)
if st not in ('verified', 'untested'):
    print(f"{arm}: status={st}, not dispatchable", file=sys.stderr); raise SystemExit(1)
# An arm whose provider has refused it is not dispatchable until the stated reset. Without
# this, the scheduler keeps firing runs into a wall and the harness scores each refusal as
# an arm failure: gemini-38-flash read 50% complete while three of its runs never started.
# (bead farmerbob-h04)
parked = v.get('parked_until')
if parked:
    import datetime
    try:
        until = datetime.datetime.fromisoformat(parked)
        now = datetime.datetime.now(datetime.timezone.utc)
        if until > now:
            left = until - now
            print(f"{arm}: parked until {parked} ({left.days}d{left.seconds // 3600}h left) -- "
                  f"quota exhausted, not an arm failure", file=sys.stderr)
            raise SystemExit(1)
    except ValueError:
        print(f"{arm}: parked_until is not a valid timestamp: {parked!r}", file=sys.stderr)
        raise SystemExit(1)

# a paid arm may not run while its free equivalent is healthy
free = v.get('redundant_with')
if free and (v.get('price_in') or 0) > 0:
    fv = d.get(free, {})
    if fv.get('status') == 'verified':
        print(f"{arm}: ${v['price_in']}/{v['price_out']} but {free} is the same model, free and "
              f"verified -- use that (set FB_ALLOW_PAID_DUPES=1 to compare harnesses deliberately)",
              file=sys.stderr)
        raise SystemExit(1)
raise SystemExit(0)
PY
}

# Sourced for the function; executed for a one-shot check. Without this, running
# `bash fb-eligible.sh <arm>` defined the function, called nothing, and exited 0 --
# a smoke test that reported nine arms eligible while measuring none.  (farmerbob-vgn)
if [ "${BASH_SOURCE[0]}" = "$0" ]; then fb_eligible "${1:?arm}"; fi
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
