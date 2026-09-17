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
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target}"
# The default prover was or-mercury-25, which has been DISABLED in the registry since the
# user's instruction "I don't trust mercury; there are free models better". The disable was
# recorded where router::eligibility reads it, so every arm SELECTION honours it -- but this
# default is not a selection, it is a hardcoded name, and it kept pointing at the disabled
# arm. A rule enforced in one place and hardcoded past in another is this project's most
# frequent shape of bug.
#
# Ask the router instead, so the prover obeys the same eligibility filter as every other
# dispatch: parked arms, missing capabilities, paid duplicates of free routes, and disabled
# arms are all excluded, and the choice is recorded rather than assumed.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
if [ -n "${4:-}" ]; then
  PROVER="$4"
elif [ -x "$FB_BIN" ]; then
  PROVER=$("$FB_BIN" select --n 1 2>/dev/null | tail -1 | sed 's/tsv: //')
fi
PROVER="${PROVER:-or-ling-30-flash}"   # free, healthy, 100% over five runs
echo "prover: $PROVER"
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
