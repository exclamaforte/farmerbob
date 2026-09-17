#!/usr/bin/env bash
# fb-promote — turn critics' CLAIMs into executed evidence.
#
# A claim is a hypothesis, not a finding. Each is classified before any test is written:
#
#   CONTRADICTED  two independent critics assert OPPOSITE expectations of the same behaviour
#                 -> the SPEC is underdetermined, not the code. Routes to task authoring.
#                    Neither arm is scored. This is SpecAmbiguous, and by the rule the router
#                    itself implements, it outranks a demonstrated defect.
#   CONFIRMATORY  EXPECT == ACTUAL: the critic documented correct behaviour rather than
#                 alleging a defect. Costs a cycle to confirm something that already passes.
#   TESTABLE      a genuine single-sided allegation -> generate a test, run it, record the
#                 outcome and credit or debit the critic.
#
#   fb-promote.sh <bead>
set -uo pipefail
BEAD="${1:?bead}"

# As of this commit the classification lives in Rust -- `fb promote` -- and this script
# DELEGATES to it. Verified field-for-field against this shell on port-crossx's real claims:
# 7 claims from 3 critics, identical critic/subject/claim/where/trigger/expect/actual/kind on
# every one, identical contradiction set.
#
# Ported because every repair that came back in this project came back in bash: three fixes
# each caused a second occurrence of the bug they fixed, and all three were shell. Bash cannot
# express the distinction the harness depends on most -- measured zero versus not measured --
# having only an empty string and a 0.
#
# Falls back to the inline Python below when the binary is not built, so a clean checkout
# still works.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
if [ -x "$FB_BIN" ]; then
  exec "$FB_BIN" promote "$BEAD"
fi

C="$HOME/.local/share/farmerbob/logs/critiques/$BEAD"
OUT="$HOME/.local/share/farmerbob/logs/$BEAD.claims.json"
python3 - "$C" "$OUT" <<'PY'
import glob, json, os, re, sys
cdir, out = sys.argv[1], sys.argv[2]

claims = []
_reviews = sorted(glob.glob(f"{cdir}/*.on.*.md"))
for f in _reviews:
    base = os.path.basename(f)[:-3]
    critic, subject = base.split(".on.")
    txt = open(f).read()
    # split on CLAIM: markers, keep each block's fields
    for blk in re.split(r'(?m)^CLAIM:', txt)[1:]:
        one = blk.strip().splitlines()
        title = one[0].strip() if one else ""
        def field(name):
            m = re.search(rf'{name}:\s*(.*?)(?=\n[A-Z]+:|\n\n|\Z)', blk, re.S)
            return " ".join(m.group(1).split()) if m else ""
        where, trig = field("WHERE"), field("TRIGGER")
        exp, act = field("EXPECT"), field("ACTUAL")
        # critics inline ACTUAL on the EXPECT line as often as not; split it out first or
        # the comparison reads an expect-string that already contains the actual
        if "ACTUAL" in exp:
            head, tail = re.split(r'ACTUAL:?', exp, 1)
            exp = head.strip()
            if not act:
                act = tail.strip()
        exp = re.sub(r'^\(.*?\)\s*', '', exp).strip()   # drop parentheticals like "(plain reading of ...)"
        claims.append({"critic": critic, "subject": subject, "claim": title,
                       "where": where, "trigger": trig,
                       "expect": exp.strip(), "actual": act.strip()})

def norm(s):
    return re.sub(r'[^a-z0-9]+', ' ', s.lower()).strip()

# confirmatory: the critic documented correct behaviour
for c in claims:
    a = norm(c["actual"])
    c["kind"] = "CONFIRMATORY" if (a in ("same", "same as expected", "as expected")
                                   or (a and a == norm(c["expect"]))) else "TESTABLE"

# contradiction: two critics, same subject area, opposite expectations
def topic(c):
    t = norm(c["claim"] + " " + c["where"])
    for k in ("excluded", "family", "budget", "shadow", "sensitivity", "ambiguous", "ladder"):
        if k in t:
            return k
    return None

by_topic = {}
for c in claims:
    if c["kind"] != "TESTABLE":
        continue
    t = topic(c)
    if t:
        by_topic.setdefault(t, []).append(c)

contradictions = []
for t, group in by_topic.items():
    if len(group) < 2:
        continue
    for i in range(len(group)):
        for j in range(i + 1, len(group)):
            a, b = group[i], group[j]
            if a["critic"] == b["critic"]:
                continue
            # opposite readings: one expects X where the other reports X as the defect
            ea, eb = norm(a["expect"]), norm(b["expect"])
            aa, ab = norm(a["actual"]), norm(b["actual"])
            # both must be real allegations, and one's EXPECTED behaviour must be the other's
            # COMPLAINT. Merely differing wording is not a contradiction.
            if not (ea and eb and aa and ab):
                continue
            if ea == aa or eb == ab:        # confirmatory, not an allegation
                continue
            if (ea and ea == ab) or (eb and eb == aa):
                for c in (a, b):
                    c["kind"] = "CONTRADICTED"
                contradictions.append({"topic": t, "a": a["critic"], "b": b["critic"],
                                       "a_expects": a["expect"][:90],
                                       "b_expects": b["expect"][:90]})
                break

# "No critic found a defect" and "no critic produced a review" are DIFFERENT facts, and
# writing an empty claims file for both makes the second invisible: fb-status then reports
# the task as critiqued and ready to adjudicate. prior, bandit-route and crossx were all
# recorded as 0-claims when in truth zero critiques were ever written -- the critics had
# been told to write a relative .fb/critique.md, which resolved to /.fb/critique.md and was
# refused.  Refuse to write the artefact rather than assert a measurement that was not made.
#   (beads farmerbob-k9f, farmerbob-1bd)
if not _reviews:
    print("NO critiques were written -- refusing to emit an empty claims file")
    raise SystemExit(3)
json.dump({"claims": claims, "contradictions": contradictions}, open(out, "w"), indent=1)

from collections import Counter
k = Counter(c["kind"] for c in claims)
print(f"  {len(claims)} claims from {len({c['critic'] for c in claims})} critics")
for kind in ("CONTRADICTED", "TESTABLE", "CONFIRMATORY"):
    print(f"    {kind:<14}{k.get(kind, 0)}")
if contradictions:
    print("\n  SPEC AMBIGUITY — independent critics read the spec differently:")
    seen = set()
    for c in contradictions:
        key = (c["topic"], c["a"], c["b"])
        if key in seen: continue
        seen.add(key)
        print(f"    topic '{c['topic']}': {c['a']} vs {c['b']}")
        print(f"      {c['a']} expects: {c['a_expects']}")
        print(f"      {c['b']} expects: {c['b_expects']}")
print(f"\n-> {out}")
PY
