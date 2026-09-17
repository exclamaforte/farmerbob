<!-- fb:creates crates/fb/src/objective.rs -->
<!-- fb:reads fb-objective.sh -->
# Task: port fb-objective.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/objective.rs` and declare it from `crates/fb/src/main.rs` with
`mod objective;`. Do not change any other file, and do not delete the shell script.

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

## One thing the previous attempt got wrong, and one thing I got wrong

This task has been attempted once. Two divergences were recorded. Only one was real.

1. **REAL: the `crates` field is renamed on the way out.** fb-objective.sh line 123 writes
   `"crates": e.get("crates_touched")` -- it reads `crates_touched` from the score record and
   emits it as `crates`. The previous port passed the raw name through, and `objective.json`
   is what `fb pareto`, `fb select` and the bandit's posteriors read, so a renamed field is a
   silent wire-format break. Reproduce every field name the script emits, exactly.

2. **NOT REAL: the row count.** The previous attempt was recorded as emitting 220 rows where
   "the script emits 137", and voided for it. It was not compared against the script. It was
   compared against a STALE `objective.json` that had not been regenerated since several
   tasks ran. Running the script produces 225 rows, the same as the port. The arm was right
   and the adjudicator was wrong.

   The lesson is for whoever writes the differential, not for you: compare against the
   script's output **produced now**, never against an artefact lying on disk. An artefact is
   a record of the last time something ran, not a statement of what the thing does.

## Required: a differential test

Add `#[cfg(test)]` tests, and include at least one that pins the OUTPUT FORMAT byte-for-byte
against a worked example taken from the script -- the exact column widths, the exact literal
words. Other scripts grep this output; a format change is a silent breakage.

You cannot run the shell script from a unit test, so do not try. Pin the format with literal
expected strings.

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

## The script

```bash
#!/usr/bin/env bash
# fb-objective — the objective tier: every machine-computed metric, per candidate, per task.
#
# No model opinion anywhere. This is the reward signal for autoresearch; the subjective tier
# runs only where this table fails to discriminate.   (bead farmerbob-ops)
set -uo pipefail
LOGS="$HOME/.local/share/farmerbob/logs"
WT="$HOME/.local/share/farmerbob/worktrees"
REPO=/home/gabe/Documents/farmerbob
python3 - "$LOGS" "$WT" "$REPO" "${1:-}" <<'PY'
import json, os, sys, glob, subprocess, tomllib
logs, wtroot, repo, only = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]

reg = tomllib.load(open(f"{repo}/sources.toml", "rb"))["source"]

def jload(p):
    try: return json.load(open(p))
    except Exception: return None

# A run the provider refused is NOT a run the arm failed. Scoring a quota block as a
# NO-OP charges a model for its vendor's billing policy: gemini-38-flash read 50% on the
# leaderboard while two of its four runs never started ("error: Individual quota reached.
# ... Resets in 153h54m42s").   (bead farmerbob-h04)
#
# Patterns are ANCHORED to the launcher's own error prefix. An unanchored search for
# "quota" would fire on the limit-detect task, whose spec text is about quotas -- the
# classifier must not be fooled by an agent discussing the thing it is being tested on.
LIMIT_PATTERNS = (
    "error: individual quota reached",          # agy
    "error: quota exceeded",
    "error: rate limit exceeded",
    "error: 429",
    "usage limit reached",                      # codex
)

# A run the LAUNCHER refused is not a run the arm failed. opencode auto-rejects a tool
# call touching what it considers an external directory -- including the run's own worktree
# under ~/.local/share/farmerbob -- and the agent stops there. Three wave5 runs across three
# arms died this way and were scored as "produced nothing".   (bead farmerbob-1bd)
def permission_killed(rec):
    path = rec.get("log")
    if not path or not os.path.exists(path):
        return False
    try:
        with open(path, errors="replace") as fh:
            body = fh.read(200_000).lower()
    except OSError:
        return False
    return ("permission requested: external_directory" in body
            and "rejected permission to use this specific tool call" in body)


# A credential the harness got wrong, or an endpoint rejecting the HARNESS's request shape,
# says nothing about the model. IFM rejects opencode's replayed `reasoning_content` field
# with wrong_api_format, which killed runs before shims/ifm-proxy.py existed; a stale API
# key killed another. Both were scored as the arm producing nothing.
INFRA_PATTERNS = (
    "incorrect api key provided",
    "wrong_api_format",
    "is unsupported\",",              # reasoning_content property rejection
    "invalid_request_error",
    "401 unauthorized",
)

def harness_failed(rec):
    path = rec.get("log")
    if not path or not os.path.exists(path):
        return False
    try:
        with open(path, errors="replace") as fh:
            head = "".join(fh.readline() for _ in range(12)).lower()
    except OSError:
        return False
    return any(p in head for p in INFRA_PATTERNS)

def quota_blocked(rec):
    """True when the run's own log opens with a provider refusal."""
    path = rec.get("log")
    if not path or not os.path.exists(path):
        return False
    try:
        with open(path, errors="replace") as fh:
            head = "".join(fh.readline() for _ in range(5)).lower()
    except OSError:
        return False
    return any(pat in head for pat in LIMIT_PATTERNS)

# per-task aggregates the harness already produced
score   = {os.path.basename(p).split('.')[0]: jload(p) for p in glob.glob(f"{logs}/*.score.json")}
crossx  = {os.path.basename(p).split('.')[0]: jload(p) for p in glob.glob(f"{logs}/*.crossx.json")}
defects = {os.path.basename(p).split('.')[0]: jload(p) for p in glob.glob(f"{logs}/*.defects.json")}
conform = {}   # parsed from run records where present

rows = []
for task, entries in sorted(score.items()):
    if only and task != only: continue
    if not entries: continue
    for e in entries:
        arm = e["source"]
        rec = jload(f"{logs}/{task}--{arm}.json") or {}
        src = reg.get(arm, {})
        wt  = f"{wtroot}/{task}--{arm}"

        # static, from the worktree
        panics = clippy = None
        tgt = None
        for cand in glob.glob(f"{wt}/crates/*/src/*.rs"):
            pass
        cx = (crossx.get(task) or {}).get(arm)
        df = ((defects.get(task) or {}).get("arms") or {}).get(arm)

        # portability denominator must come from the crossx run's OWN arm count, not from
        # the score file -- different runs covered different candidate sets, which produced
        # negative "3/3 minus 3" values.
        cxall = crossx.get(task) or {}
        n = max(len(cxall) - 1, 0)
        rows.append({
            "task": task, "arm": arm,
            "verdict": e.get("verdict"),
            "tests": e.get("tests_run"),
            "clippy": e.get("clippy"),
            "lines": e.get("lines"),
            "crates": e.get("crates_touched"),
            "portability": (None if not cx or not n else
                            f"{max(n - cx['api_incompatible_with'], 0)}/{n}"),
            "defect_sens": (None if not df else
                            (f"{df['caught']}/{df['comparable']}" if df['comparable'] else None)),
            "secs": e.get("duration_s"),
            "mem_mb": rec.get("mem_peak_mb"),
            "price": (None if src.get("quota") == "plan"
                      else src.get("price_in")),
            "outcome": rec.get("outcome_class") or
                       ("task_invalid" if e.get("verdict") == "TASK-INVALID" else
                        "orchestrator_cancelled" if rec.get("rc") in (143, 137) else
                        "quota_limited" if quota_blocked(rec) else
                        "infrastructure" if permission_killed(rec) or harness_failed(rec) else
                        "unknown" if rec.get("verdict") is None else "arm_result"),
        })

# UNANIMITY INDICTS THE TASK, NOT THE FIELD. When every arm on a task wrote zero lines,
# the overwhelmingly likely cause is that the deliverable already existed at the base commit
# (farmerbob-m71) or the spec was unreachable -- not that four independent models each chose
# to do nothing. gpu-lease: all four arms, 0 lines, identical test counts.
from collections import defaultdict as _dd
_by_task = _dd(list)
for _r in rows:
    _by_task[_r["task"]].append(_r)
for _t, _rs in _by_task.items():
    _arm = [x for x in _rs if x["outcome"] == "arm_result"]
    if len(_arm) >= 2 and all((x.get("lines") or 0) == 0 for x in _arm):
        for x in _arm:
            x["outcome"] = "task_invalid"

hdr = f"{'TASK':<14}{'ARM':<22}{'VERDICT':<11}{'TESTS':>6}{'CLIPPY':>7}{'LINES':>7}{'PORT':>7}{'DEFECT':>8}{'SECS':>6}{'MEM':>6}{'$/1M':>7}  OUTCOME"
print(hdr); print("-" * len(hdr))
for r in rows:
    print(f"{r['task']:<14}{r['arm']:<22}{str(r['verdict']):<11}"
          f"{str(r['tests'] if r['tests'] is not None else '-'):>6}"
          f"{str(r['clippy'] if r['clippy'] is not None else '-'):>7}"
          f"{str(r['lines'] if r['lines'] is not None else '-'):>7}"
          f"{str(r['portability'] or '-'):>7}"
          f"{str(r['defect_sens'] or '-'):>8}"
          f"{str(r['secs'] if r['secs'] is not None else '-'):>6}"
          f"{str(r['mem_mb'] or '-'):>6}"
          f"{('free' if r['price'] is None else format(r['price'], '.2f')):>7}"
          f"  {r['outcome']}")

out = f"{logs}/objective.json"
json.dump(rows, open(out, "w"), indent=1)
print(f"\n{len(rows)} candidate-runs -> {out}")

# where does the objective tier fail to discriminate?
from collections import defaultdict
bytask = defaultdict(list)
for r in rows:
    if r["outcome"] == "arm_result" and r["verdict"] == "PASS":
        bytask[r["task"]].append(r)
print("\nwhere the objective tier does NOT separate candidates (subjective tier needed):")
for t, rs in sorted(bytask.items()):
    if len(rs) > 1:
        print(f"  {t:<14}{len(rs)} candidates pass the gate")
PY

```


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
4. **Test depth and generality** — measured directly where possible, by injecting known
   defects and by running your suite against rival implementations. Where neither
   measurement could be taken, the number of distinct behaviours you covered stands in for
   it. Count is the fallback, not the target:
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
