#!/usr/bin/env python3
"""Capture and check local baselines for KernelBench task directories.

Bead farmerbob-85t: upstream reference timings are not portable to this
machine. For every imported task, measure the torch eager reference on this
5090 under full measurement hygiene, repeated until the confidence interval
is tight, and store it with the env fingerprint.

    shims/kb_baseline.py capture --task <task-dir> [--trials N]
        Measures candidate.py (which starts as the reference copy) N times
        via the kb eval bridge and writes ref/baseline.json:
        {fingerprint, median_ms, q1_ms, q3_ms, iqr_ms, trials, samples}.
        When a baseline already exists with a DIFFERENT fingerprint, it is
        re-baselined automatically (the old file is overwritten and the
        change is reported). With the SAME fingerprint the capture is
        refused unless --force is given.
    shims/kb_baseline.py check --task <task-dir>
        Exits 0 when ref/baseline.json exists and its fingerprint matches
        the current environment; exits 1 (stale) or 2 (missing/unreadable)
        otherwise. Scoring must refuse to score against a stale baseline.

Measurement hygiene: one task holds the whole GPU (6GB+ tensors are normal),
so captures run sequentially and each bench sample is a fresh process.
"""

import argparse
import json
import os
import pathlib
import statistics
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
KB_SHIM = REPO / "shims" / "kb_shim.py"
KB_ENV_PYTHON = os.path.expanduser("~/.local/share/farmerbob/kb-env/bin/python")


def _python():
    return KB_ENV_PYTHON if os.path.exists(KB_ENV_PYTHON) else sys.executable


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


def _shim_bench(task_dir: pathlib.Path, perf_trials: int) -> float:
    out = subprocess.run(
        [
            _python(),
            str(KB_SHIM),
            "bench",
            "--ref",
            str(task_dir / "ref" / "problem.py"),
            "--candidate",
            str(task_dir / "candidate.py"),
            "--perf-trials",
            str(perf_trials),
        ],
        capture_output=True,
        text=True,
        timeout=900,
    )
    if out.returncode != 0:
        raise RuntimeError(f"bench sample failed (rc={out.returncode}): {out.stderr.strip()[-500:]}")
    # Last JSON object line wins, mirroring bench_read: log chatter on
    # stdout is skipped, never parsed.
    ms = None
    for line in out.stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            ms = float(json.loads(line)["ms"])
        except (json.JSONDecodeError, KeyError, ValueError):
            continue
    if ms is None:
        raise RuntimeError(f"no JSON measurement on bench stdout: {out.stdout.strip()[-200:]}")
    return ms


def _summarize(samples):
    ordered = sorted(samples)
    median = statistics.median(ordered)
    n = len(ordered)
    if n == 1:
        q1 = q3 = median
    elif n % 2 == 0:
        q1 = statistics.median(ordered[: n // 2])
        q3 = statistics.median(ordered[n // 2 :])
    else:
        q1 = statistics.median(ordered[: n // 2])
        q3 = statistics.median(ordered[n // 2 + 1 :])
    return median, q1, q3, max(0.0, q3 - q1)


def cmd_capture(args) -> int:
    task_dir = pathlib.Path(args.task)
    baseline_path = task_dir / "ref" / "baseline.json"
    try:
        fingerprint = _current_fingerprint()
    except RuntimeError as e:
        print(f"capture: {e}", file=sys.stderr)
        return 2
    old = None
    if baseline_path.is_file():
        try:
            old = json.loads(baseline_path.read_text())
        except (json.JSONDecodeError, OSError) as e:
            print(f"capture: existing baseline unreadable, overwriting: {e}")
            old = None
        if old and old.get("fingerprint") == fingerprint and not args.force:
            print(f"capture: baseline current ({fingerprint}), use --force to re-capture")
            return 0
        if old and old.get("fingerprint") != fingerprint:
            print(
                f"capture: fingerprint changed:\n  old {old.get('fingerprint')}\n"
                f"  new {fingerprint}\n  re-baselining automatically"
            )
    samples = []
    for i in range(args.trials):
        try:
            ms = _shim_bench(task_dir, args.perf_trials)
        except RuntimeError as e:
            print(f"capture: sample {i + 1}/{args.trials} failed: {e}", file=sys.stderr)
            return 1
        samples.append(ms)
        print(f"capture: sample {i + 1}/{args.trials}: {ms} ms")
    median, q1, q3, iqr = _summarize(samples)
    baseline_path.write_text(
        json.dumps(
            {
                "fingerprint": fingerprint,
                "median_ms": median,
                "q1_ms": q1,
                "q3_ms": q3,
                "iqr_ms": iqr,
                "trials": len(samples),
                "samples": samples,
                "unit": "ms",
            },
            indent=2,
        )
        + "\n"
    )
    print(f"capture: wrote {baseline_path}: median {median} ms, IQR {iqr} ms")
    return 0


def cmd_check(args) -> int:
    task_dir = pathlib.Path(args.task)
    baseline_path = task_dir / "ref" / "baseline.json"
    if not baseline_path.is_file():
        print(f"check: no baseline at {baseline_path}")
        return 2
    try:
        baseline = json.loads(baseline_path.read_text())
    except (json.JSONDecodeError, OSError) as e:
        print(f"check: baseline unreadable: {e}")
        return 2
    try:
        current = _current_fingerprint()
    except RuntimeError as e:
        print(f"check: {e}", file=sys.stderr)
        return 2
    if baseline.get("fingerprint") != current:
        print(
            f"check: STALE baseline for {task_dir}:\n"
            f"  baseline {baseline.get('fingerprint')}\n  current  {current}"
        )
        return 1
    print(f"check: baseline current for {task_dir}")
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Capture and check kernel baselines")
    sub = parser.add_subparsers(dest="cmd", required=True)
    cap = sub.add_parser("capture")
    cap.add_argument("--task", required=True)
    cap.add_argument("--trials", type=int, default=5)
    cap.add_argument("--perf-trials", type=int, default=10)
    cap.add_argument("--force", action="store_true")
    chk = sub.add_parser("check")
    chk.add_argument("--task", required=True)
    args = parser.parse_args(argv)
    if args.cmd == "capture":
        return cmd_capture(args)
    return cmd_check(args)


if __name__ == "__main__":
    sys.exit(main())
