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

print(f"{'ARM':<24}{'COMPLETE':>10}{'N':>4}{'TOTAL $':>10}{'$/SUCCESS':>11}{'TOKENS':>10}{'EXCL':>6}  FRONTIER")
print("-" * 92)
for rate, c, arm, a, per in sorted(table, key=lambda t: (-t[0], t[1])):
    mark = "  <= pareto" if arm in front else ""
    p = f"{per:.4f}" if per is not None else "  n/a"
    print(f"{arm:<24}{rate:>9.0%}{a['n']:>4}{c:>10.4f}{p:>11}{a['tok']:>10}{a['excl']:>6}{mark}")

tot = sum(a["cost"] for a in agg.values())
ok = sum(a["ok"] for a in agg.values())
n = sum(a["n"] for a in agg.values())
print(f"\n  {n} scored runs, {ok} completed, ${tot:.2f} measured spend"
      f"  ({'$%.4f' % (tot / ok) if ok else 'n/a'} per completed task)")
json.dump({k: v for k, v in agg.items()}, open(f"{base}/logs/pareto.json", "w"), indent=1)
print(f"-> {base}/logs/pareto.json")
PY
