#!/usr/bin/env python3
"""KernelBench eval bridge for farmerbob task scripts.

Decision record (farmerbob-j0nj): importing KernelBench's eval code drags in
its whole LLM inference stack (`dotenv`, `openai`, `litellm`, ...), which
farmerbob never calls -- it supplies its own arms. Bypassing the package
`__init__` is not safe either: `eval.py` uses relative imports
(`from . import timing, dataset`), so a direct file load fails, and the
`__init__` has a side effect (registering `torch.rand_mix`) whose absence
would change what "correct" means.

This shim takes the second of the three options: import the package normally
so the side effect is preserved, but stub the inference modules first by
inserting dummy entries in `sys.modules` for `openai` and `litellm` before
importing. Verified: the only `__init__` side effect is the additive
`torch.rand_mix` / `rand_mix_like` registration (nothing patches
`torch.randn`), which no eval path reads; `level1` constructs 100 problems
through this path and a reference-against-reference eval reports
correctness True.

Usage:
    kb_shim.py verify --ref ref.py --candidate candidate.py [--trials N]
        Prints {"correct": bool, "detail": str} and exits 0. Exit non-zero
        only when the instrument itself failed (missing files, import error):
        a wrong kernel is a successful read of a failed verification.
    kb_shim.py bench --ref ref.py --candidate candidate.py [--perf-trials N]
        Prints {"ms": float, "metrics": {...}} and exits 0 when the kernel is
        correct and timed. Exits 1 when the kernel is incorrect or timing
        failed (counts as a bad trial, never as a measurement).

The candidate file may define `class Model` (natural shape, as agents write
it) or `class ModelNew` (KernelBench's entry point). When only `Model` is
present the source is rewritten in memory to `ModelNew`, including the
`super(Model, ...)` fixup; the file on disk is never modified. When both
are present `ModelNew` wins.

Environment:
    KB_SRC    KernelBench `src/` directory (default /home/gabe/KernelBench/src).
    KB_DEVICE CUDA device (default cuda:0).
"""

import argparse
import contextlib
import io
import json
import os
import pathlib
import sys
import types

# Large KernelBench tasks hold 6GB+ tensors several times over (input +
# reference output + candidate output + perf inputs ≈ 25GB on a 32GB card),
# and the default caching allocator fragments on them. Expandable segments
# let the allocator reuse freed blocks across trials. Must be set before
# torch initialises, i.e. at this module's top.
os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")


def _stub_inference_stack():
    """Insert dummy inference modules so `import kernelbench` never pulls
    the LLM stack. Only `openai` and `litellm` need stubs: every other eval
    dependency (torch, numpy, tqdm, pydantic, dotenv, requests, packaging)
    is real in the kb-env virtualenv."""
    if "openai" not in sys.modules:
        try:
            __import__("openai")
        except ImportError:
            module = types.ModuleType("openai")
            module.OpenAI = object
            sys.modules["openai"] = module
    if "litellm" not in sys.modules:
        try:
            __import__("litellm")
        except ImportError:
            module = types.ModuleType("litellm")

            def _unavailable(*args, **kwargs):
                raise RuntimeError("stubbed litellm: farmerbob never calls the LLM stack")

            module.completion = _unavailable
            sys.modules["litellm"] = module


def _kb_src():
    src = os.environ.get("KB_SRC", "/home/gabe/KernelBench/src")
    if not os.path.isdir(src):
        raise FileNotFoundError(f"KernelBench src not found: {src} (set KB_SRC)")
    if src not in sys.path:
        sys.path.insert(0, src)
    return src


def _load_kb():
    _stub_inference_stack()
    _kb_src()
    from kernelbench import eval as kb_eval  # noqa: E402

    return kb_eval


def _call_eval_quietly(func, *args, **kwargs):
    """Run an eval call with its stdout captured.

    KernelBench prints unconditionally (`[Profiling] ...`, compile notes)
    to stdout, which would corrupt this shim's contract that stdout carries
    exactly one JSON object. Captured chatter is re-emitted on stderr so it
    is still visible in logs but never parsed as output.
    """
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        result = func(*args, **kwargs)
    chatter = buffer.getvalue()
    if chatter.strip():
        print(chatter.strip(), file=sys.stderr)
    return result


def _normalise_candidate(src: str) -> str:
    """Rewrite a `class Model` candidate to KernelBench's `ModelNew` entry
    point when `ModelNew` is absent. Semantics-preserving: renames the class
    and its `super(Model, ...)` call."""
    if "class ModelNew(" in src:
        return src
    if "class Model(" not in src:
        return src
    return src.replace("class Model(", "class ModelNew(").replace(
        "super(Model,", "super(ModelNew,"
    )


def cmd_verify(args) -> int:
    try:
        ref = pathlib.Path(args.ref).read_text()
    except OSError as e:
        print(f"verify: cannot read ref {args.ref}: {e}", file=sys.stderr)
        return 2
    try:
        candidate_raw = pathlib.Path(args.candidate).read_text()
    except OSError as e:
        print(f"verify: cannot read candidate {args.candidate}: {e}", file=sys.stderr)
        return 2
    try:
        kb_eval = _load_kb()
    except Exception as e:
        print(f"verify: cannot load kernelbench.eval: {e}", file=sys.stderr)
        return 2
    candidate = _normalise_candidate(candidate_raw)
    try:
        result = _call_eval_quietly(
            kb_eval.eval_kernel_against_ref,
            ref,
            candidate,
            num_correct_trials=args.trials,
            measure_performance=False,
            device=args.device,
        )
    except AssertionError as e:
        # No CUDA etc: the instrument, not the candidate, failed.
        print(f"verify: instrument assertion: {e}", file=sys.stderr)
        return 2
    except Exception as e:
        print(f"verify: eval crashed: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    if result is None:
        print("verify: eval returned None (lock error, retry)", file=sys.stderr)
        return 2
    detail = str(result.metadata.get("correctness_trials", ""))
    if not result.correctness and result.metadata.get("correctness_issue"):
        detail = f"{result.metadata.get('correctness_issue_name', 'mismatch')}: {result.metadata.get('correctness_issue')}"
    if not detail:
        detail = "correct" if result.correctness else "incorrect"
    print(json.dumps({"correct": bool(result.correctness), "detail": detail}))
    return 0


def cmd_bench(args) -> int:
    try:
        ref = pathlib.Path(args.ref).read_text()
    except OSError as e:
        print(f"bench: cannot read ref {args.ref}: {e}", file=sys.stderr)
        return 2
    try:
        candidate_raw = pathlib.Path(args.candidate).read_text()
    except OSError as e:
        print(f"bench: cannot read candidate {args.candidate}: {e}", file=sys.stderr)
        return 2
    try:
        kb_eval = _load_kb()
    except Exception as e:
        print(f"bench: cannot load kernelbench.eval: {e}", file=sys.stderr)
        return 2
    candidate = _normalise_candidate(candidate_raw)
    try:
        result = _call_eval_quietly(
            kb_eval.eval_kernel_against_ref,
            ref,
            candidate,
            num_correct_trials=1,
            measure_performance=True,
            num_perf_trials=args.perf_trials,
            device=args.device,
        )
    except AssertionError as e:
        print(f"bench: instrument assertion: {e}", file=sys.stderr)
        return 2
    except Exception as e:
        print(f"bench: eval crashed: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    if result is None or not result.correctness:
        # Wrong kernel: no measurement. Exit 1 so the harness counts a bad
        # trial, never a sample.
        return 1
    ms = float(result.runtime)
    if not (ms == ms and ms != float("inf") and ms != float("-inf")) or ms <= 0:
        return 1
    metrics = {
        "ref_ms": float(result.ref_runtime),
        "unit": "ms",
        "correctness_trials": result.metadata.get("correctness_trials", ""),
    }
    print(json.dumps({"ms": ms, "metrics": metrics}))
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="KernelBench eval bridge")
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("verify", "bench"):
        p = sub.add_parser(name)
        p.add_argument("--ref", required=True)
        p.add_argument("--candidate", required=True)
        p.add_argument("--device", default=os.environ.get("KB_DEVICE", "cuda:0"))
        if name == "verify":
            p.add_argument("--trials", type=int, default=1)
        else:
            p.add_argument("--perf-trials", type=int, default=10)
    args = parser.parse_args(argv)
    if args.cmd == "verify":
        return cmd_verify(args)
    return cmd_bench(args)


if __name__ == "__main__":
    sys.exit(main())
