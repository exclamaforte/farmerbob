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
        otherwise. Exits 3 with the recorded reason when the task is
        recorded unmeasurable on this fingerprint (bead farmerbob-x81s.13).
        Scoring must refuse to score against a stale baseline.
    shims/kb_baseline.py bulk --tasks-dir <dir> [--trials N] [--force]
        Captures every task missing a current baseline, sequentially, one
        GPU holder at a time. Prints `bulk [i/N] <task> OK|FAIL|SKIP:<why>`
        per task and ends with `bulk done: <ok> ok, <failed> failed,
        <skipped> skipped` -- with the failure count and a non-zero exit
        whenever any task failed (bead farmerbob-x81s.13: the old loop
        ended with a bare `bulk done` over 26 failures, reading clean).
        Structurally unmeasurable tasks are recorded, not retried blindly.

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
        raise RuntimeError(f"bench sample failed (rc={out.returncode}): {_oom_line(out.stderr)}")
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


def _oom_line(stderr: str) -> str:
    """The most diagnostic line of a failed sample, not just its tail.

    The old code kept the last 500 chars, which cut the OOM sizes off and
    left only the generic fragmentation hint -- one reason the 26 missing
    baselines were misread as fragmentation (bead farmerbob-x81s.13).
    Prefer the line stating what was tried and what was free.
    """
    for line in stderr.splitlines():
        if "CUDA out of memory" in line or "Tried to allocate" in line:
            return line.strip()[-500:]
    return stderr.strip()[-500:]


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
        # A task recorded unmeasurable on THIS fingerprint is answered,
        # not missing (bead farmerbob-x81s.13). A stale record (other
        # fingerprint) is plain missing: a new GPU may well measure it.
        unmeas = _read_unmeasurable(task_dir)
        if unmeas is not None:
            try:
                current = _current_fingerprint()
            except RuntimeError as e:
                print(f"check: {e}", file=sys.stderr)
                return 2
            if unmeas.get("fingerprint") == current:
                print(f"check: unmeasurable for {task_dir}: {unmeas.get('reason')}: {unmeas.get('detail')}")
                return 3
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


# Peak-memory preflight (bead farmerbob-x81s.13). One bench process holds
# roughly input + reference output + candidate output + the allclose diff
# -- four copies of the inputs -- so PEAK_FACTOR estimates the high-water
# mark from the input bytes. Only tasks above nearly the whole card are
# structural: anything else is attempted, and an OOM there is a retryable
# FAIL (a neighbour may be holding gigabytes), never a verdict about the
# task.
PEAK_FACTOR = 4
PREFLIGHT_FRACTION = 0.95

# torch factories get_inputs is allowed to call. Anything else (notably
# torch.tensor over literal data) aborts the estimate: fail-open, attempt
# the capture, let the GPU decide.
_META_FACTORIES = (
    "rand", "randn", "randint", "zeros", "ones", "empty", "full",
    "arange", "linspace", "eye",
)


def _estimate_inputs_bytes(task_dir: pathlib.Path):
    """Input bytes without materialising: meta tensors, zero RAM.

    Factories return real meta tensors (ops on them work, memory is zero),
    so get_inputs runs normally and the returned shapes are summed. Any
    surprise -- an unknown constructor, literal data, a read error --
    yields None: the caller attempts the capture rather than guessing.
    """
    import torch

    try:
        ref_src = (task_dir / "ref" / "problem.py").read_text()
    except OSError:
        return None

    def numel_of_shape(shape):
        n = 1
        for v in shape:
            if isinstance(v, bool) or not isinstance(v, int):
                raise ValueError(f"non-shape dim {v!r}")
            n *= max(v, 0)
        return n

    def as_shape(name, args, kwargs):
        ints = [a for a in args if isinstance(a, int) and not isinstance(a, bool)]
        tuples = [a for a in args if isinstance(a, (tuple, list))]
        if name in ("rand", "randn", "zeros", "ones", "empty"):
            if tuples:
                if len(tuples) != 1 or ints:
                    raise ValueError(f"{name}: ambiguous shape")
                return tuple(tuples[0])
            return tuple(ints)
        if name == "full":
            if not args:
                raise ValueError("full: no size")
            first = args[0]
            return tuple(first) if isinstance(first, (tuple, list)) else tuple(ints[:1] or [1])
        if name == "randint":
            for a in reversed(args):
                if isinstance(a, (tuple, list)):
                    return tuple(a)
            return ()
        if name == "arange":
            nums = [a for a in args if isinstance(a, (int, float)) and not isinstance(a, bool)]
            if "end" in kwargs or len(nums) >= 2:
                start = nums[0] if len(nums) >= 2 else 0
                end = kwargs.get("end", nums[-1])
                step = kwargs.get("step", nums[2] if len(nums) >= 3 else 1)
                import math

                return (max(0, math.ceil((end - start) / step)),)
            if len(nums) == 1:
                return (max(0, int(nums[0])),)
            raise ValueError("arange: no end")
        if name == "linspace":
            steps = kwargs.get("steps", args[2] if len(args) >= 3 else None)
            if not isinstance(steps, int) or isinstance(steps, bool):
                raise ValueError("linspace: no steps")
            return (max(steps, 0),)
        if name == "eye":
            n = ints[0] if ints else None
            m = ints[1] if len(ints) >= 2 else n
            if n is None:
                raise ValueError("eye: no size")
            return (n, m)
        raise ValueError(f"unknown factory {name}")

    saved = {}
    # NOTE: torch.empty is itself patched below, so the real one must be
    # captured first: the meta factories allocate through it, and going
    # through the patch would recurse until RecursionError (which reads
    # as "cannot estimate" -- the wrong answer for every task).
    real_empty = torch.empty

    def make(name):
        def meta(*args, **kwargs):
            dtype = kwargs.get("dtype", torch.float32)
            if not isinstance(dtype, torch.dtype):
                raise ValueError(f"{name}: bad dtype")
            return real_empty(as_shape(name, args, kwargs), dtype=dtype, device="meta")

        return meta

    try:
        for name in _META_FACTORIES:
            if not hasattr(torch, name):
                continue
            saved[name] = getattr(torch, name)
            setattr(torch, name, make(name))
        local = {}
        exec(compile(ref_src, "<ref>", "exec"), local)
        get_inputs = local.get("get_inputs")
        if get_inputs is None:
            return None
        produced = get_inputs()
        if not isinstance(produced, (list, tuple)):
            return None
        total = 0
        for t in produced:
            if not isinstance(t, torch.Tensor):
                return None
            total += t.numel() * t.element_size()
        return total
    except Exception:
        return None
    finally:
        for name, fn in saved.items():
            setattr(torch, name, fn)


def _unmeasurable_path(task_dir: pathlib.Path) -> pathlib.Path:
    return task_dir / "ref" / "unmeasurable.json"


def _read_unmeasurable(task_dir: pathlib.Path):
    try:
        return json.loads(_unmeasurable_path(task_dir).read_text())
    except (json.JSONDecodeError, OSError):
        return None


def _gpu_total_bytes():
    """Device memory in bytes, or None when nvidia-smi is unavailable."""
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.total", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    try:
        return int(out.stdout.strip().splitlines()[0]) * 1024 * 1024
    except (ValueError, IndexError):
        return None


def cmd_bulk(args) -> int:
    """Capture every task missing a current baseline, honestly summarised."""
    tasks_dir = pathlib.Path(args.tasks_dir)
    try:
        fingerprint = _current_fingerprint()
    except RuntimeError as e:
        print(f"bulk: {e}", file=sys.stderr)
        return 2
    gpu_total = _gpu_total_bytes()
    tasks = sorted(p for p in tasks_dir.iterdir() if (p / "ref" / "problem.py").is_file())
    print(f"bulk start {len(tasks)} tasks fp={fingerprint}")
    ok = failed = skipped = 0
    for i, task in enumerate(tasks, 1):
        name = task.name
        baseline_path = task / "ref" / "baseline.json"
        if baseline_path.is_file() and not args.force:
            try:
                old = json.loads(baseline_path.read_text())
            except (json.JSONDecodeError, OSError):
                old = None
            if old and old.get("fingerprint") == fingerprint:
                print(f"bulk [{i}/{len(tasks)}] {name} SKIP:current")
                skipped += 1
                continue
        estimate = _estimate_inputs_bytes(task)
        if (
            estimate is not None
            and gpu_total
            and estimate * PEAK_FACTOR > gpu_total * PREFLIGHT_FRACTION
        ):
            detail = (
                f"inputs ~{estimate / 1e9:.1f} GB x{PEAK_FACTOR} peak ~"
                f"{estimate * PEAK_FACTOR / 1e9:.1f} GB over {gpu_total / 1e9:.1f} GB device"
            )
            _unmeasurable_path(task).write_text(
                json.dumps(
                    {
                        "fingerprint": fingerprint,
                        "reason": "structural-oom",
                        "detail": detail,
                        "io_bytes_estimate": estimate,
                        "gpu_total_bytes": gpu_total,
                        "task": name,
                    },
                    indent=2,
                )
                + "\n"
            )
            print(f"bulk [{i}/{len(tasks)}] {name} SKIP:unmeasurable {detail}")
            skipped += 1
            continue
        cap = argparse.Namespace(
            task=str(task), trials=args.trials, perf_trials=args.perf_trials, force=True
        )
        rc = cmd_capture(cap)
        if rc == 0:
            try:
                _unmeasurable_path(task).unlink()
            except OSError:
                pass
            print(f"bulk [{i}/{len(tasks)}] {name} OK")
            ok += 1
        else:
            print(f"bulk [{i}/{len(tasks)}] {name} FAIL:rc={rc}")
            failed += 1
    print(f"bulk done: {ok} ok, {failed} failed, {skipped} skipped")
    return 1 if failed else 0


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
    bulk = sub.add_parser("bulk")
    bulk.add_argument("--tasks-dir", required=True)
    bulk.add_argument("--trials", type=int, default=5)
    bulk.add_argument("--perf-trials", type=int, default=10)
    bulk.add_argument("--force", action="store_true")
    args = parser.parse_args(argv)
    if args.cmd == "capture":
        return cmd_capture(args)
    if args.cmd == "bulk":
        return cmd_bulk(args)
    return cmd_check(args)


if __name__ == "__main__":
    sys.exit(main())
