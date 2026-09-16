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
cd /home/gabe/Documents/farmerbob
export PATH="$HOME/.cargo/bin:$PATH"
LEDGER=.fb/credits.json
CMD="${1:?verify|credit|ledger}"

case "$CMD" in
  auto)
    # Harvest every CONFIRMED proof into the task's permanent suite. fb-prove already wrote
    # the test, ran it against the subject, and ran it against the merged reference -- the
    # artefact exists and was simply being discarded. Escalation keeps it.
    T="${2:?task}"
    tgt=$(grep -ohE '<!-- fb:creates [^ ]+ -->' ".fb/prompts/$T.md" 2>/dev/null | awk '{print $3}' | head -1)
    [ -n "$tgt" ] || { echo "$T: no fb:creates target"; exit 2; }
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
      if [ "$prov" = "True" ]; then
        echo "  $subj: confirmed $conf but the veto was UNAVAILABLE -- provisional, not escalating"
        continue
      fi
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
      if ! cargo test -p "$crate" "$marker" >/dev/null 2>&1; then
        echo "  $subj: VETOED against the merged reference -- retracting"
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
    tgt=$(grep -ohE '<!-- fb:creates [^ ]+ -->' ".fb/prompts/$T.md" | awk '{print $3}' | head -1)
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
