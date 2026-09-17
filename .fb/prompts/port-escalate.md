<!-- fb:creates crates/fb/src/escalate.rs -->
<!-- fb:reads fb-escalate.sh fb-escalate-graft.py -->
# Task: port fb-escalate.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/escalate.rs` and declare it from `crates/fb/src/main.rs` with
`mod escalate;`. Do not change any other file, and do not delete the shell script.

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
# fb-escalate — promote a CONFIRMED finding into the task's permanent conformance suite,
# with provenance, and credit the critic that found it.
#
# The objective tier is not fixed. A critic's hypothesis becomes an executed test
# (farmerbob-ljq, done); a CONFIRMED one should become part of the gate that judges every
# future candidate on that task (farmerbob-mqr). Each round then sharpens the next, and the
# subjective tier shrinks as judgement turns into execution.
#
# Two disciplines, both enforced here:
#
#   REFERENCE VETO. An escalated test must PASS against the merged reference. One that fails
#   is encoding the critic's misreading of the spec rather than a defect -- this project has
#   already seen a critic "confirm" a test asserting sensitivity() should be None where the
#   spec makes it Some(0.0). Confirmation against a losing candidate is not enough.
#
#   PROVENANCE. Every escalated test records the critic that found it, the candidate it was
#   found on, and the task. A bad test must be traceable and retirable, and the arm that
#   contributed a good one must be creditable.
#
#   fb-escalate.sh verify <task>          run the suite against the merged reference
#   fb-escalate.sh credit <task> <critic> <subject> <n>   record a contribution
#   fb-escalate.sh ledger                 print the credit ledger
set -uo pipefail
. /home/gabe/Documents/farmerbob/fb-target.sh
cd /home/gabe/Documents/farmerbob
export PATH="$HOME/.cargo/bin:$PATH"
LEDGER=.fb/credits.json
REPO_ESC=/home/gabe/Documents/farmerbob
CMD="${1:?verify|credit|ledger}"

case "$CMD" in
  crossx)
    # Cross-examination finds defects that never pass through a CLAIM, so they never reached
    # the permanent suite: outcome's validation order, resume's due_now ordering and gate's
    # one-line explain were all found this way and all discarded after deciding a single
    # adjudication. The test already exists and already passes against the merged winner, so
    # escalating it costs nothing and sharpens the gate for every future candidate.
    T="${2:?task}"
    tgt=$(fb_target "$T")
    [ -n "$tgt" ] || { fb_target_why "$T"; exit 2; }
    crate=$(awk -F/ '{print $2}' <<< "$tgt")
    CX="$HOME/.local/share/farmerbob/logs/$T.crossx.json"
    [ -s "$CX" ] || { echo "$T: no crossx"; exit 0; }
    WTR="$HOME/.local/share/farmerbob/worktrees"
    suite=".fb/conformance/$T.rs"
    finder=$(python3 -c "
import json
d=json.load(open('$CX'))
b=[(a,v) for a,v in d.items() if v.get('suite_discriminating') and (v.get('discovery') or 0)>0]
b.sort(key=lambda kv:-kv[1]['discovery'])
print(b[0][0] if b else '')")
    [ -n "$finder" ] || { echo "  no discriminating suite"; exit 0; }
    marker="cx_${T//-/_}_${finder//-/_}"
    grep -q "$marker" "$suite" 2>/dev/null && { echo "  already escalated from $finder"; exit 0; }
    echo "  discriminating suite: $finder"
    python3 "$REPO_ESC/fb-escalate-graft.py" "$WTR/$T--$finder/$tgt" "$suite" "$marker" "$finder" "$T" || exit 0
    python3 "$REPO_ESC/fb-escalate-graft.py" --into "$suite" "$tgt" "$marker"
    n=$(cargo test -p "$crate" "$marker" 2>&1 | grep -cE "^test .*$marker.*ok$")
    if [ "${n:-0}" -eq 0 ] || ! cargo test -p "$crate" "$marker" >/dev/null 2>&1; then
      echo "  VETOED against the merged reference -- retracting"
      python3 "$REPO_ESC/fb-escalate-graft.py" --retract "$suite" "$tgt"
      exit 0
    fi
    echo "  kept: $n test(s) pass against the merged winner"
    ./fb-escalate.sh credit "$T" "$finder" "crossx" "$n" >/dev/null
    exit 0
    ;;
  auto)
    # Harvest every CONFIRMED proof into the task's permanent suite. fb-prove already wrote
    # the test, ran it against the subject, and ran it against the merged reference -- the
    # artefact exists and was simply being discarded. Escalation keeps it.
    T="${2:?task}"
    tgt=$(fb_target "$T")
    [ -n "$tgt" ] || { fb_target_why "$T"; exit 2; }
    crate=$(awk -F/ '{print $2}' <<< "$tgt")
    PROOFS="$HOME/.local/share/farmerbob/logs/proofs/$T"
    suite=".fb/conformance/$T.rs"
    [ -d "$PROOFS" ] || { echo "$T: no proofs"; exit 0; }
    any=0
    for j in "$PROOFS"/*.json; do
      [ -f "$j" ] || continue
      subj=$(python3 -c "import json;print(json.load(open('$j'))['subject'])")
      conf=$(python3 -c "import json;print(json.load(open('$j'))['confirmed'])")
      [ "${conf:-0}" -gt 0 ] || continue
      prov=$(python3 -c "import json;print(json.load(open('$j')).get('provisional',False))" 2>/dev/null)
      # `provisional` means fb-prove could not veto, because prove always runs BEFORE the
      # merge and HEAD had no reference yet. That is the normal case, not an exception: a
      # blanket refusal here meant nothing could ever escalate. Note it and continue -- the
      # veto below runs against the reference as it stands NOW, which is the check that
      # actually matters.
      [ "$prov" = "True" ] && echo "  $subj: prove could not veto (pre-merge); vetoing now instead"
      tree="$PROOFS/$subj.tree/$tgt"
      [ -f "$tree" ] || { echo "  $subj: confirmed $conf but no proof tree"; continue; }
      # Which critic found it. A claim names its critic; take the one with claims on this subject.
      critic=$(python3 -c "
import json,collections
try: cs=json.load(open('$HOME/.local/share/farmerbob/logs/$T.claims.json'))['claims']
except Exception: raise SystemExit
c=collections.Counter(x['critic'] for x in cs if x.get('subject')=='$subj')
print(c.most_common(1)[0][0] if c else '')" 2>/dev/null)
      [ -n "$critic" ] || critic="unknown"
      marker="escalated_${T//-/_}_${subj//-/_}"
      if grep -q "$marker" "$suite" 2>/dev/null; then echo "  $subj: already escalated"; continue; fi
      python3 - "$tree" "$suite" "$marker" "$T" "$subj" "$critic" "$conf" <<'PYE'
import re, sys
tree, suite, marker, task, subj, critic, conf = sys.argv[1:8]
src = open(tree).read()
m = re.search(r'#\[cfg\(test\)\]\s*mod proved\s*\{', src)
if not m:
    print(f"  {subj}: no `mod proved` block in the proof tree"); raise SystemExit(0)
# Balance the braces. A greedy `.*` under DOTALL swallows the ENCLOSING module's closing
# brace too, which appends a stray `}` and breaks the suite -- caught on the first run.
i = src.index('{', m.start()); depth = 0; end = None
for j in range(i, len(src)):
    if src[j] == '{': depth += 1
    elif src[j] == '}':
        depth -= 1
        if depth == 0: end = j + 1; break
if end is None:
    print(f"  {subj}: unbalanced `mod proved` block"); raise SystemExit(0)
body = src[m.start():end].replace("mod proved", f"mod {marker}", 1)
hdr = (f"\n// ESCALATED: {conf} confirmed finding(s) by critic {critic}, found on {subj}.\n"
       f"// Promoted from an executed proof that passed the reference veto. Provenance is\n"
       f"// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)\n")
open(suite, "a").write(hdr + body + "\n")
print(f"  {subj}: escalated {conf} finding(s) from {critic}")
PYE
      # RUN THE VETO HERE, against the reference as it is NOW. fb-prove's veto ran when the
      # task may not yet have been merged -- reviewer.rs did not exist on master during the
      # backfill, so there was no reference and nothing could be vetoed. An escalation
      # admitted on a veto that could not run is exactly the false confirmation the
      # discipline exists to prevent.
      # Graft into the CRATE before vetoing. Running `cargo test -p <crate> <marker>` while
      # the block lives only in .fb/conformance matches zero tests, and cargo exits 0 on
      # zero tests -- so the veto passed by measuring nothing. That is the empty-suite trap
      # (farmerbob-slh) reappearing inside the check built to prevent it.
      python3 - "$suite" "$tgt" "$marker" <<'PYG'
import re, sys
suite, target, marker = sys.argv[1:4]
blk = re.search(rf'\n// ESCALATED:(?:(?!\n// ESCALATED:).)*?mod {marker}\b.*', open(suite).read(), re.S)
if blk:
    t = open(target).read()
    if marker not in t:
        open(target, 'a').write('\n' + blk.group(0))
PYG
      if ! cargo test -p "$crate" "$marker" 2>&1 | grep -qE "^test .*$marker.*ok$"; then
        echo "  $subj: VETOED against the merged reference -- retracting"
        python3 -c "
import re,sys
t=sys.argv[1]; s=open(t).read()
m=re.search(r'\n// ESCALATED:(?:(?!\n// ESCALATED:).)*\$', s, re.S)
if m: open(t,'w').write(s[:m.start()]+'\n')" "$tgt"
        python3 - "$suite" <<'PYV'
import re, sys
p = sys.argv[1]; s = open(p).read()
m = re.search(r'\n// ESCALATED:(?:(?!\n// ESCALATED:).)*$', s, re.S)
if m: open(p, 'w').write(s[:m.start()] + '\n')
PYV
        continue
      fi
      ./fb-escalate.sh credit "$T" "$critic" "$subj" "$conf" >/dev/null
      any=1
    done
    [ "$any" -eq 1 ] && echo "  -> $suite  (run: ./fb-escalate.sh verify $T)"
    exit 0
    ;;
  verify)
    T="${2:?task}"
    suite=".fb/conformance/$T.rs"
    [ -f "$suite" ] || { echo "no suite for $T"; exit 2; }
    tgt=$(fb_target "$T")
    crate=$(awk -F/ '{print $2}' <<< "$tgt")
    echo "reference veto: $T against merged HEAD ($crate)"
    cargo test -p "$crate" "conformance_${T//-/_}" 2>&1 | grep -E '^test |test result:' | tail -20
    ;;
  credit)
    T="${2:?task}"; CRITIC="${3:?critic}"; SUBJECT="${4:?subject}"; N="${5:-1}"
    python3 - "$LEDGER" "$T" "$CRITIC" "$SUBJECT" "$N" <<'PY'
import json, os, sys
led, task, critic, subject, n = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])
d = json.load(open(led)) if os.path.exists(led) else {"contributions": []}
key = (task, critic, subject)
if any((c["task"], c["critic"], c["found_on"]) == key for c in d["contributions"]):
    print(f"already credited: {critic} on {task}/{subject}"); raise SystemExit(0)
d["contributions"].append({"task": task, "critic": critic, "found_on": subject, "tests": n})
json.dump(d, open(led, "w"), indent=1)
print(f"credited {critic}: {n} test(s) on {task}, found on {subject}")
PY
    ;;
  ledger)
    python3 - "$LEDGER" <<'PY'
import json, os, sys
from collections import defaultdict
led = sys.argv[1]
if not os.path.exists(led):
    print("no contributions recorded"); raise SystemExit(0)
d = json.load(open(led))
by = defaultdict(lambda: {"tests": 0, "tasks": set()})
for c in d["contributions"]:
    by[c["critic"]]["tests"] += c["tests"]
    by[c["critic"]]["tasks"].add(c["task"])
print(f"{'CRITIC':<22}{'TESTS':>6}  TASKS")
for critic, v in sorted(by.items(), key=lambda kv: -kv[1]["tests"]):
    print(f"{critic:<22}{v['tests']:>6}  {' '.join(sorted(v['tasks']))}")
print(f"\n{sum(v['tests'] for v in by.values())} tests escalated into the permanent suite "
      f"from {len(by)} critic(s)")
PY
    ;;
esac

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
