#!/usr/bin/env bash
# fb-pareto — cost versus ability to complete real implementation tasks.
#
# This is the number farmerbob exists to produce: given a task you actually need done, which
# arms can do it, and what do they cost?
#
# Cost is MEASURED, not estimated. Each run has its own isolated opencode database (built to
# fix a concurrency deadlock; it turns out to give exact per-run attribution), which records
# dollars and token counts per session. Plan-based arms are genuinely $0.
#
# Ability is the objective completion rate, counting only arm_result outcomes -- a run killed
# for cost, OOMed, rate-limited or given an invalid task says nothing about the arm.
set -uo pipefail
python3 - "$HOME/.local/share/farmerbob" "/home/gabe/Documents/farmerbob" <<'PY'
import glob, json, os, sqlite3, sys, tomllib
from collections import defaultdict

base, repo = sys.argv[1], sys.argv[2]
reg = tomllib.load(open(f"{repo}/sources.toml", "rb"))["source"]

# measured cost per run, from each run's own isolated store
cost, toks = {}, {}
for db in glob.glob(f"{base}/state/*/data/opencode/opencode.db"):
    run = db.split("/state/")[1].split("/")[0]
    try:
        c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        rows = list(c.execute("select cost, tokens_input, tokens_output from session"))
        c.close()
    except Exception:
        continue
    cost[run] = sum(r[0] or 0 for r in rows)
    toks[run] = sum((r[1] or 0) + (r[2] or 0) for r in rows)

# Critics and provers run through fb_launch, which does NOT set the per-run XDG_DATA_HOME
# that fb-dispatch gives implementers. Their sessions therefore land in the SHARED opencode
# store and were invisible to this board: $3.03 across 137 sessions, against a reported
# total of $6.41. The user's provider dashboard read $11.28 and this is most of the gap.
# Attribute that store by model, since the session row carries it.
shared = {}
sp = os.path.expanduser("~/.local/share/opencode/opencode.db")
if os.path.exists(sp):
    model_to_arm = {}
    for arm, v in reg.items():
        m = v.get("model")
        if m:
            model_to_arm[m.split("/", 1)[-1] if "/" in m else m] = arm
    try:
        sc = sqlite3.connect(f"file:{sp}?mode=ro", uri=True)
        # NB: do not name this `cost` -- that is the per-run dict built above, and
        # rebinding it to a float here silently broke every per-run attribution.
        for model_json, scost, tin, tout in sc.execute(
                "select model, cost, tokens_input, tokens_output from session"):
            mid = ""
            try:
                mid = json.loads(model_json or "{}").get("id", "")
            except Exception:
                pass
            arm = model_to_arm.get(mid)
            if not arm:
                continue
            c0, t0 = shared.get(arm, (0.0, 0))
            shared[arm] = (c0 + (scost or 0), t0 + (tin or 0) + (tout or 0))
        sc.close()
    except Exception:
        pass

rows = json.load(open(f"{base}/logs/objective.json"))
agg = defaultdict(lambda: {"n": 0, "ok": 0, "cost": 0.0, "tok": 0, "secs": 0, "excl": 0})
for r in rows:
    a = agg[r["arm"]]
    run = f"{r['task']}--{r['arm']}"
    a["cost"] += cost.get(run, 0.0)
    a["tok"] += toks.get(run, 0)
    if r["outcome"] != "arm_result":
        a["excl"] += 1
        continue
    a["n"] += 1
    a["secs"] += r.get("secs") or 0
    if r["verdict"] == "PASS":
        a["ok"] += 1

# fold the shared-store spend in, under a distinct label so it is never mistaken for
# per-run attribution
shared_total = sum(c for c, _ in shared.values())
for arm, (c, t) in shared.items():
    if arm in agg:
        agg[arm]["cost"] += c
        agg[arm]["tok"] += t

table = []
for arm, a in agg.items():
    if a["n"] == 0:
        continue
    rate = a["ok"] / a["n"]
    per_ok = (a["cost"] / a["ok"]) if a["ok"] else None
    table.append((rate, a["cost"], arm, a, per_ok))

# Pareto frontier: no other arm is both cheaper-per-success AND at least as capable
def dominated(x, others):
    rate, c, arm, a, per = x
    for r2, c2, arm2, a2, per2 in others:
        if arm2 == arm or a2["ok"] == 0:
            continue
        if per is None:
            return True
        if per2 is not None and per2 <= per and r2 >= rate and (per2 < per or r2 > rate):
            return True
    return False

front = {x[2] for x in table if not dominated(x, table)}

# Finding defects in a rival is a DIFFERENT capability from implementing, and until now
# nothing measured it. An arm that implements adequately but reliably sharpens the suite is
# worth knowing about, and the leaderboard could not express that.  (bead farmerbob-mqr)
credits = {}
try:
    led = json.load(open(f"{repo}/.fb/credits.json"))
    for c in led["contributions"]:
        credits[c["critic"]] = credits.get(c["critic"], 0) + c["tests"]
except Exception:
    pass

print(f"{'ARM':<24}{'COMPLETE':>10}{'N':>4}{'TOTAL $':>10}{'$/SUCCESS':>11}{'TOKENS':>10}{'EXCL':>6}{'ESC':>5}  FRONTIER")
print("-" * 97)
for rate, c, arm, a, per in sorted(table, key=lambda t: (-t[0], t[1])):
    mark = "  <= pareto" if arm in front else ""
    p = f"{per:.4f}" if per is not None else "  n/a"
    esc = credits.get(arm, 0)
    print(f"{arm:<24}{rate:>9.0%}{a['n']:>4}{c:>10.4f}{p:>11}{a['tok']:>10}{a['excl']:>6}"
          f"{(str(esc) if esc else '-'):>5}{mark}")

tot = sum(a["cost"] for a in agg.values())
ok = sum(a["ok"] for a in agg.values())
n = sum(a["n"] for a in agg.values())
print(f"\n  of which ${shared_total:.2f} is critic/prover spend from the shared store,"
      f" attributed by model")
print(f"  {n} scored runs, {ok} completed, ${tot:.2f} measured spend"
      f"  ({'$%.4f' % (tot / ok) if ok else 'n/a'} per completed task)")
json.dump({k: v for k, v in agg.items()}, open(f"{base}/logs/pareto.json", "w"), indent=1)
print(f"-> {base}/logs/pareto.json")
PY
