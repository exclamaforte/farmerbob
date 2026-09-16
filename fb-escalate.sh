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
