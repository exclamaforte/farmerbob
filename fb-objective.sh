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
                        "infrastructure" if permission_killed(rec) else
                        "unknown" if rec.get("verdict") is None else "arm_result"),
        })

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
