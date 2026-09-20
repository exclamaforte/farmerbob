#!/usr/bin/env python3
"""Score KernelBench task directories: correctness gate, speedup, fast_p.

Bead farmerbob-nxn: score = correctness first (a wrong kernel scores zero
regardless of speed), then speedup over the local baseline, aggregated as
fast_p (fraction of tasks both correct and faster than p x baseline) per
KernelBench semantics.

    shims/kb_report.py --tasks <level-dir> [--p 1.0] [--perf-trials 10]

This script does GPU I/O only (verify + bench samples via the eval bridge).
All scoring math lives in ONE implementation, `fb kernel-score`
(crates/fb/src/kernel_score_cmd.rs over farmerbob-core::kernel_score):
correctness gate, stale-baseline refusal, median/IQR, reliability, speedup.
fast_p below applies kernel_score::fast_p verbatim: the fraction of ALL
tasks scored with speedup > p; every other status scores zero.

Output is one JSON object per line (tasks then a summary).
"""

import argparse
import json
import os
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
KB_SHIM = REPO / "shims" / "kb_shim.py"
KB_ENV_PYTHON = os.path.expanduser("~/.local/share/farmerbob/kb-env/bin/python")


def _python():
    return KB_ENV_PYTHON if os.path.exists(KB_ENV_PYTHON) else sys.executable


def _fb():
    env = os.environ.get("FB_BIN")
    if env:
        return env
    repo_bin = REPO / "target" / "debug" / "fb"
    if repo_bin.is_file():
        return str(repo_bin)
    return "fb"


def _last_json(stdout: str):
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                continue
    return None


def _current_fingerprint():
    out = subprocess.run(
        [_python(), str(REPO / "shims" / "kb_fingerprint.py")],
        capture_output=True,
        text=True,
        timeout=60,
    )
    if out.returncode != 0:
        raise RuntimeError(f"fingerprint failed: {out.stderr.strip()}")
    return out.stdout.strip()


def _verify(task_dir: pathlib.Path):
    out = subprocess.run(
        [
            _python(), str(KB_SHIM), "verify",
            "--ref", str(task_dir / "ref" / "problem.py"),
            "--candidate", str(task_dir / "candidate.py"),
            "--trials", "1",
        ],
        capture_output=True, text=True, timeout=900,
    )
    if out.returncode != 0:
        return None
    obj = _last_json(out.stdout)
    return obj.get("correct", False) if obj else None


def _bench_sample(task_dir: pathlib.Path, perf_trials: int):
    out = subprocess.run(
        [
            _python(), str(KB_SHIM), "bench",
            "--ref", str(task_dir / "ref" / "problem.py"),
            "--candidate", str(task_dir / "candidate.py"),
            "--perf-trials", str(perf_trials),
        ],
        capture_output=True, text=True, timeout=900,
    )
    if out.returncode != 0:
        return None
    obj = _last_json(out.stdout)
    try:
        ms = float(obj["ms"])
    except (TypeError, KeyError, ValueError):
        return None
    return ms if ms > 0 else None


def score_task(task_dir: pathlib.Path, fingerprint: str, perf_trials: int, p: float):
    """Collect I/O, delegate scoring to `fb kernel-score`. Missing or
    unreadable measurements score as unreliable (never as zero evidence)."""
    name = task_dir.name
    baseline_path = task_dir / "ref" / "baseline.json"
    if not baseline_path.is_file():
        return {"task": name, "status": "missing-baseline", "correct": None, "fast": False}
    correct = _verify(task_dir)
    if correct is None:
        return {"task": name, "status": "instrument", "correct": None, "fast": False}
    samples = []
    if correct:
        for _ in range(3):
            ms = _bench_sample(task_dir, perf_trials)
            if ms is None:
                break
            samples.append(ms)
    if correct and len(samples) != 3:
        # Verified but unmeasurable: unreliable, with the samples we have
        # (possibly none) passed through for transparency.
        pass
    out = subprocess.run(
        [
            _fb(), "kernel-score",
            str(baseline_path),
            "--fingerprint", fingerprint,
            "--samples", ",".join(str(s) for s in samples),
            "--p", str(p),
            "--min-samples", "3",
        ]
        + (["--correct"] if correct else []),
        capture_output=True, text=True, timeout=60,
    )
    row = _last_json(out.stdout)
    if row is None:
        return {"task": name, "status": "instrument", "correct": correct, "fast": False}
    row["task"] = name
    row["correct"] = correct
    return row


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Score kernel tasks: gate, speedup, fast_p")
    parser.add_argument("--tasks", required=True)
    parser.add_argument("--p", type=float, default=1.0)
    parser.add_argument("--perf-trials", type=int, default=10)
    args = parser.parse_args(argv)
    try:
        fingerprint = _current_fingerprint()
    except RuntimeError as e:
        print(json.dumps({"summary": True, "error": str(e)}))
        return 2
    level_dir = pathlib.Path(args.tasks)
    task_dirs = sorted(p for p in level_dir.iterdir() if (p / "task.toml").is_file())
    rows = [score_task(t, fingerprint, args.perf_trials, args.p) for t in task_dirs]
    for row in rows:
        print(json.dumps(row))
    # kernel_score::fast_p verbatim: fraction of ALL tasks with speedup > p.
    fast = sum(1 for r in rows if r.get("status") == "scored" and (r.get("speedup") or 0) > args.p)
    print(json.dumps({
        "summary": True,
        "tasks": len(rows),
        "scored": sum(1 for r in rows if r.get("status") == "scored"),
        "fast": fast,
        "fast_p": (fast / len(rows)) if rows else 0.0,
        "p": args.p,
    }))
    return 0


if __name__ == "__main__":
    sys.exit(main())
