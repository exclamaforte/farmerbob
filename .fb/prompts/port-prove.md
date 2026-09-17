<!-- fb:creates crates/fb/src/prove.rs -->
<!-- fb:reads fb-prove.sh -->
# Task: port fb-prove.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/prove.rs` and declare it from `crates/fb/src/main.rs` with
`mod prove;`. Do not change any other file, and do not delete the shell script.

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
# fb-prove — turn TESTABLE claims into executed evidence.
#
# A test is written to assert the REVIEWER'S expectation, so it fails when the reviewer was
# right and passes when they were wrong. Both outcomes are recorded: a confirmed claim becomes
# permanent evidence and credits the critic; a refuted one debits it. That gives a usefulness
# signal for each critic without any verifier-evaluation machinery.   (bead farmerbob-ljq)
#
#   fb-prove.sh <bead> <crate> <target-rel> [prover-arm]
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target}"; PROVER="${4:-or-mercury-25}"
REPO=/home/gabe/Documents/farmerbob
. /home/gabe/Documents/farmerbob/fb-launch.sh
WT="$HOME/.local/share/farmerbob/worktrees"
LOGS="$HOME/.local/share/farmerbob/logs"
SLOTS=${FB_SLOTS:-5}
OUT="$LOGS/$BEAD.proved.json"
mkdir -p "$LOGS/proofs/$BEAD"

# group testable claims by the implementation they are about
mapfile -t SUBJECTS < <(python3 -c "
import json,sys
d=json.load(open('$LOGS/$BEAD.claims.json'))
subs={c['subject'] for c in d['claims'] if c['kind']=='TESTABLE'}
print('\n'.join(sorted(subs)))")
echo "${#SUBJECTS[@]} implementations have claims against them"

for subj in "${SUBJECTS[@]}"; do
  while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ]; do sleep 2; done
  (
    sw="$WT/$BEAD--$subj"
    [ -d "$sw" ] || exit 0
    claims=$(python3 -c "
import json
d=json.load(open('$LOGS/$BEAD.claims.json'))
cs=[c for c in d['claims'] if c['kind']=='TESTABLE' and c['subject']=='$subj']
for i,c in enumerate(cs,1):
    print(f\"### claim_{i}  (from {c['critic']})\")
    print(f\"CLAIM: {c['claim']}\")
    print(f\"WHERE: {c['where']}\")
    print(f\"TRIGGER: {c['trigger']}\")
    print(f\"EXPECT (assert this): {c['expect']}\")
    print(f\"reviewer says ACTUAL is: {c['actual']}\")
    print()")
    [ -z "$claims" ] && exit 0
    p="$LOGS/proofs/$BEAD/$subj.prompt.md"
    python3 - "$REPO/.fb/prompts/_prove.md" "$p" "$TARGET" "$CRATE" <<PY
import sys
tpl=open(sys.argv[1]).read()
claims="""$(printf '%s' "$claims" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 20000)"""
open(sys.argv[2],'w').write(tpl.replace("{TARGET}",sys.argv[3]).replace("{CRATE}",sys.argv[4]).replace("{CLAIMS}",claims))
PY
    # prove in a scratch copy so the candidate worktree is never mutated
    d="$LOGS/proofs/$BEAD/$subj.tree"; rm -rf "$d"; mkdir -p "$d"
    (cd "$sw" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$d" && tar xf -) 2>/dev/null
    P="$(cat "$p")"
    ( fb_launch "$PROVER" "$P" "$d" ) > "$LOGS/proofs/$BEAD/$subj.log" 2>&1
    (cd "$d" && timeout 400 cargo test -p "$CRATE" proved > "$LOGS/proofs/$BEAD/$subj.test" 2>&1)
    pass=$(grep -oE '[0-9]+ passed' "$LOGS/proofs/$BEAD/$subj.test"|head -1|awk '{print $1}')
    fail=$(grep -oE '[0-9]+ failed' "$LOGS/proofs/$BEAD/$subj.test"|head -1|awk '{print $1}')

    # REFERENCE VETO. A test that also fails against the MERGED implementation is testing the
    # claim's misreading of the spec, not a defect in this candidate. Without this, a wrong
    # claim plus a prover that faithfully encodes it produces a false confirmation -- observed
    # live: a claim asserted sensitivity() should be None for 5 missed / 0 rejected, when the
    # spec's P(reject|defective) makes it a measured 0.0.   (bead farmerbob-ljq)
    ref="$LOGS/proofs/$BEAD/$subj.ref"; rm -rf "$ref"; mkdir -p "$ref"
    (cd "$REPO" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$ref" && tar xf -) 2>/dev/null
    awk '/#\[cfg\(test\)\]\n?mod proved/{f=1} /^#\[cfg\(test\)\]$/{buf=$0; next} {if(buf){print buf; buf=""} print}' \
      "$d/$TARGET" 2>/dev/null >/dev/null
    python3 - "$d/$TARGET" "$ref/$TARGET" <<'PYX'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'#\[cfg\(test\)\]\s*mod proved\s*\{.*', src, re.S)
if m:
    ref = open(sys.argv[2]).read()
    open(sys.argv[2], 'w').write(ref + "\n" + m.group(0))
PYX
    # The veto is only meaningful if HEAD actually contains the module under test. When the
    # task has not been merged yet there is no reference, the grafted proof cannot resolve
    # its symbols, and the run reports 0 failures -- indistinguishable from "checked and
    # clean". Say UNAVAILABLE instead, and mark the confirmations provisional.
    veto_state=ok
    if [ ! -s "$REPO/$TARGET" ]; then
      veto_state=unavailable
    else
      (cd "$ref" && timeout 400 cargo test -p "$CRATE" proved > "$LOGS/proofs/$BEAD/$subj.reftest" 2>&1)
      if grep -qE '^error\[E[0-9]+\]:|could not compile' "$LOGS/proofs/$BEAD/$subj.reftest"; then
        veto_state=unavailable
      fi
    fi
    if [ "$veto_state" = unavailable ]; then
      reffail=0
      echo "  $subj: VETO UNAVAILABLE -- HEAD has no reference for $TARGET; confirmations are PROVISIONAL"
    else
      reffail=$(grep -oE '[0-9]+ failed' "$LOGS/proofs/$BEAD/$subj.reftest"|head -1|awk '{print $1}')
    fi
    reffail=${reffail:-0}
    # failures that also occur on the reference are invalid tests, not confirmations
    invalid=$(( reffail < ${fail:-0} ? reffail : ${fail:-0} ))
    fail=$(( ${fail:-0} - invalid ))
    rm -rf "$ref"
    printf '  %-22s proved %s: %s CONFIRMED, %s refuted, %s VETOED (also fail on the reference)\n' \
      "$subj" "$(( ${pass:-0} + ${fail:-0} + invalid ))" "${fail:-0}" "${pass:-0}" "$invalid"
    python3 -c "
import json
json.dump({'subject':'$subj','confirmed':${fail:-0},'refuted':${pass:-0},'vetoed':$invalid,
           'veto':'$veto_state','provisional':'$veto_state'=='unavailable'},
          open('$LOGS/proofs/$BEAD/$subj.json','w'))"
  ) &
done
wait
python3 - "$LOGS/proofs/$BEAD" "$OUT" "$LOGS/$BEAD.claims.json" <<'PY'
import glob, json, sys, os
d, out, claimsf = sys.argv[1], sys.argv[2], sys.argv[3]
rows = [json.load(open(f)) for f in glob.glob(f"{d}/*.json")]
claims = json.load(open(claimsf))["claims"]
# credit critics whose claims were confirmed
by_subject = {r["subject"]: r for r in rows}
credit = {}
for c in claims:
    if c["kind"] != "TESTABLE": continue
    r = by_subject.get(c["subject"])
    if not r: continue
    k = credit.setdefault(c["critic"], {"claims": 0, "on_confirmed_subjects": 0})
    k["claims"] += 1
    if r["confirmed"]: k["on_confirmed_subjects"] += 1
json.dump({"subjects": rows, "critics": credit}, open(out, "w"), indent=1)
tc = sum(r["confirmed"] for r in rows); tr = sum(r["refuted"] for r in rows)
print(f"\n  {tc} claims CONFIRMED by execution, {tr} refuted")
print(f"-> {out}")
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
