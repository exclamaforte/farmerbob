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
        Prints {"correct": bool, "detail": str, "clock": "ok"|"tampered:..."}
        and exits 0. Exit non-zero only when the instrument itself failed
        (missing files, import error): a wrong kernel is a successful read
        of a failed verification. The verdict also carries the widened
        correctness record (tolerance/precision/trials/failed_axis), the
        held-out verdict (specialised/heldout_seed), and the
        reference-call detectors (static_hits as questions,
        runtime_hit/runtime_modules/reference_call as the answer).
    kb_shim.py bench --ref ref.py --candidate candidate.py [--perf-trials N]
        Prints {"ms": float, "metrics": {...}, "clock": "ok"} and exits 0
        when the kernel is correct and timed. Exits 1 when the kernel is
        incorrect or timing failed (counts as a bad trial, never as a
        measurement) -- and prints {"clock": "tampered:..."} or
        {"clock": "divergent:..."} instead of any ms when the candidate
        reached the instrument's clock (bead farmerbob-x81s.2): a corrupted
        clock reads as no measurement, never as a fast one.

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
import time
import types

# Large KernelBench tasks hold 6GB+ tensors several times over (input +
# reference output + candidate output + perf inputs ≈ 25GB on a 32GB card),
# and the default caching allocator fragments on them. Expandable segments
# let the allocator reuse freed blocks across trials. Must be set before
# torch initialises, i.e. at this module's top.
os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")

# Clock-guard identities (bead farmerbob-x81s.2). The candidate is
# exec_module'd into THIS process, so its module-level code runs inside the
# instrument and can rebind the timing primitives the measurement uses --
# no fraud required, a debug monkey-patch left in place does the same.
# The wall-clock references are saved at import, before any candidate or
# eval code loads; the torch references are snapshotted per command after
# the (harness-trusted) eval import but before the candidate executes, so
# a later rebinding is the candidate's doing and compares unequal. Kept to
# four: the two wall clocks anything could fall back on, and the two CUDA
# primitives the cuda_event timing path uses.
_REAL_PERF_COUNTER = time.perf_counter
_REAL_MONOTONIC = time.monotonic


def _snapshot_clock():
    """Record the timing primitives' identities. torch is imported here,
    not at module top, so `--help` and import cost stay free of it."""
    import torch

    return {
        "time.perf_counter": time.perf_counter,
        "time.monotonic": time.monotonic,
        "torch.cuda.synchronize": torch.cuda.synchronize,
        "torch.cuda.Event": torch.cuda.Event,
    }


def _check_clock(before):
    """Names of timing primitives rebound since the snapshot, sorted.

    Compared with `is`: a rebinding replaces the module attribute with a
    different object, while innocent use never does. An empty list means
    the instrument's clock is the harness's own."""
    after = _snapshot_clock()
    return sorted(name for name, obj in before.items() if after.get(name) is not obj)


# Correctness audit (bead farmerbob-x81s.3). What KernelBench's
# eval_kernel_against_ref actually enforces, read from the source rather
# than inherited: inputs cast to the precision dtype and moved to device;
# BOTH models .to(device, precision) (device agreement is forced, never
# checked); output SHAPES must match exactly (the only path that names
# itself: correctness_issue_name is set only here); VALUES must satisfy
# torch.allclose with atol == rtol == tolerance-for-precision (fp32 1e-4,
# fp16/bf16 1e-2, torchbench-inspired); every one of num_correct_trials
# trials must pass, seeded deterministically from 42 with inputs from the
# REFERENCE's get_inputs (the candidate's own are ignored). Dtype IS
# required to match, harder than the bead first guessed: allclose raises
# ("Float did not match Double") on mixed dtypes instead of comparing, so
# an fp16/f64 candidate dies as a runtime error -- no correctness_issue,
# no axis -- rather than passing within tolerance. The silent
# undeclared-precision-change path does not exist on this stack; the
# harness records both dtypes anyway, so a future torch that promotes
# instead of raising is still caught by name.
PRECISION_TOLERANCES = {"fp32": 1e-4, "fp16": 1e-2, "bf16": 1e-2}
# Audited mirror: KernelBench get_tolerance_for_precision AND
# farmerbob-core tolerance_for_precision state this same table. Three
# copies is two too many, but Python and Rust cannot share one; the unit
# tests on both sides pin it, so a drift fails loudly instead of silently.

# Seed for the harness's own confirmation inputs. Fixed and stated: a
# second visible set the agent could in principle also memorise (it is in
# this source). Memorising it AND the eval seed buys nothing, because the
# held-out set below uses a fresh per-run seed the agent never had.
HARNESS_SEED = 1234


def _bitwise_equal(a, b):
    """Exact equality with NaN-aware comparison: NaNs in the same
    positions count as equal (a deterministic NaN is still
    deterministic), anything else bitwise."""
    import torch

    if tuple(a.shape) != tuple(b.shape) or a.dtype != b.dtype:
        return False
    try:
        same = (a == b) | (torch.isnan(a) & torch.isnan(b))
        return bool(same.all().item())
    except Exception:
        return False


def _task_dir_from_ref(ref_path):
    # Generated layout: <task>/ref/problem.py, so the task dir is two up.
    return pathlib.Path(ref_path).resolve().parent.parent


def _read_correctness_manifest(task_dir):
    """Correctness knobs from the task manifest: (precision, trials,
    determinism).

    task.toml states them (written by kb_ingest, backfilled on existing
    tasks); absent keys fall back to the audited upstream defaults. An
    unreadable manifest or an unknown value is the harness misconfigured,
    never the candidate's fault -- the caller exits 2 (instrument)."""
    precision, trials, determinism = "fp32", 1, "required"
    manifest = pathlib.Path(task_dir) / "task.toml"
    if manifest.is_file():
        try:
            import tomllib
        except ImportError:  # pragma: no cover - kb-env is 3.14
            tomllib = None
        if tomllib is not None:
            try:
                with open(manifest, "rb") as f:
                    doc = tomllib.load(f)
            except Exception as e:
                raise ValueError(f"cannot parse {manifest}: {e}")
            precision = str(doc.get("correctness_precision", precision))
            trials = doc.get("correctness_trials", trials)
            determinism = str(doc.get("determinism", determinism))
    if precision not in PRECISION_TOLERANCES:
        raise ValueError(f"unknown correctness_precision {precision!r}")
    try:
        trials = int(trials)
    except (TypeError, ValueError):
        raise ValueError(f"bad correctness_trials {trials!r}")
    if trials < 1:
        raise ValueError("correctness_trials must be at least 1")
    if determinism not in ("required", "allowed"):
        raise ValueError(f"unknown determinism {determinism!r}")
    return precision, trials, determinism


def _dtype_from_string(precision):
    import torch

    return {"fp32": torch.float32, "fp16": torch.float16, "bf16": torch.bfloat16}[precision]


def _axis_of_eval_failure(result):
    """Map an eval-reported failure to its axis, or None when it is not a
    comparison failure at all (a launch exception has no axis). Only the
    shape path names itself upstream; the value path is identified by its
    text."""
    issue = result.metadata.get("correctness_issue") or ""
    if "shape mismatch" in issue.lower():
        return "shape"
    if "output mismatch" in issue.lower():
        return "values"
    return None


def _harness_check(ref_src, candidate_src, precision, device, seed):
    """Compare candidate output to the reference on device, shape, dtype
    and the task-stated tolerance, on seeded inputs.

    Independent of the eval's own verdict: same provenance rules (inputs
    from the REFERENCE's get_inputs, both models cast to the precision),
    one extra trial's worth of compute. `seed` selects the input set:
    HARNESS_SEED for the stated confirmation set, a fresh per-run secret
    for the held-out set (bead farmerbob-x81s.4). Returns a dict with
    failed_axis None when every axis passes, else the first failing axis
    in device/shape/dtype/values order, plus the recorded dtypes, device
    and seed. The dtype check is a tripwire: allclose raises on mixed
    dtypes before any comparison, so a mismatch reaching here means
    upstream stopped requiring it -- and is still caught by name. Raises
    on harness-side errors (model won't load, forward crashes): the caller
    keeps the eval's verdict then, because a harness that cannot obtain
    its second opinion must not overrule the first."""
    import torch

    dtype = _dtype_from_string(precision)
    tol = PRECISION_TOLERANCES[precision]
    dev = torch.device(device)
    ref_ns: dict = {}
    exec(compile(ref_src, "<ref>", "exec"), ref_ns)
    cand_ns: dict = {}
    exec(compile(_normalise_candidate(candidate_src), "<candidate>", "exec"), cand_ns)
    get_inputs = ref_ns.get("get_inputs")
    get_init = ref_ns.get("get_init_inputs", lambda: [])
    cand_cls = cand_ns.get("ModelNew") or cand_ns.get("Model")
    ref_cls = ref_ns.get("Model")
    if get_inputs is None or cand_cls is None or ref_cls is None:
        raise RuntimeError("ref lacks get_inputs/Model or candidate lacks a model class")
    torch.manual_seed(seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed(seed)
    inputs = [x.to(dtype=dtype).to(dev) for x in get_inputs()]
    init = get_init()
    with torch.no_grad():
        out_ref = ref_cls(*init).to(dev, dtype)  # noqa: E1102 - classes from exec
        out_ref = out_ref(*inputs)
        cand_model = cand_cls(*init).to(dev, dtype)  # noqa: E1102
        out_cand = cand_model(*inputs)
        torch.cuda.synchronize(dev)
        # Determinism (bead farmerbob-x81s.7): the same instance twice
        # under one seed, compared bitwise. A race, an uninitialised
        # buffer, or an atomics-order dependence reads differently the
        # second time; legitimate fp nondeterminism reads differently
        # too, which is why the task declares the property rather than
        # the harness refusing it blanket.
        out_cand2 = cand_model(*inputs)
        torch.cuda.synchronize(dev)
        deterministic = _bitwise_equal(out_cand, out_cand2)
    report = {
        "failed_axis": None,
        "ref_dtype": str(out_ref.dtype),
        "candidate_dtype": str(out_cand.dtype),
        "device": str(out_ref.device),
        "seed": seed,
        "deterministic": bool(deterministic),
        # Minimum memory traffic in bytes: inputs read plus output
        # written, at the comparison precision. A lower bound by
        # construction (weights, temporaries and halo reads all move
        # more), which is the safe direction for a floor: a result above
        # it is merely not-impossible, while a result below it did not
        # happen (bead farmerbob-x81s.6).
        "io_bytes": int(
            sum(x.numel() * x.element_size() for x in inputs)
            + out_ref.numel() * out_ref.element_size()
        ),
    }
    if str(out_ref.device) != str(out_cand.device):
        report["failed_axis"] = "device"
    elif tuple(out_ref.shape) != tuple(out_cand.shape):
        report["failed_axis"] = "shape"
    elif out_ref.dtype != out_cand.dtype:
        report["failed_axis"] = "dtype"
    elif not torch.allclose(out_ref, out_cand, atol=tol, rtol=tol):
        report["failed_axis"] = "values"
        report["max_diff"] = float(torch.max(torch.abs(out_ref - out_cand)).item())
    return report


# Reference-call detectors (bead farmerbob-x81s.5). The cheapest correct
# kernel calls the reference and returns its output: it passes every
# correctness check by construction and times near the baseline, reading
# as "no speedup, honest attempt" while the bandit keeps paying the arm.
# Two detectors, deliberately different strengths. Static is nearly free
# and catches the honest-mistake case (a debug fallback left in); it is a
# QUESTION, never a verdict -- getattr and importlib defeat it. Runtime
# instruments nothing and trusts nothing the candidate touched: after the
# eval it scans sys.modules for anything the candidate executed from the
# task's own reference tree. A file must be executed to run, whatever name
# it was loaded under, so this is an ANSWER.
#
# Deliberately NOT flagged: the torch operator the task names as the thing
# to beat. The starting candidate IS that operator, so naming it would
# question every run including honest winners (the 1.70x SDPA result wraps
# the very call it beat). A same-operator call with different config is
# what optimisation looks like here; only a pass-through is a cheat, and
# a pass-through that never touches reference code is behaviourally the
# reference -- out of scope for these detectors by construction.
STATIC_REFERENCE_PATTERNS = (
    "ref/problem.py",
    "ref/",
    "baseline.json",
    "upstream.json",
    "verify.sh",
    "bench.sh",
    "importlib",
    "__import__",
    "SourceFileLoader",
    "spec_from_file_location",
    "exec_module",
    "subprocess",
    "os.system",
    "os.popen",
    "sys.path",
)


def static_reference_hits(candidate_src):
    """Pattern hits suggesting the candidate reaches for the reference.

    Returns [{pattern, line, excerpt}]. A QUESTION: reported, never
    scored on. Syntax-unreadable sources yield no hits -- the eval fails
    them anyway, and a guess about unparseable code is worse than silence.
    """
    import ast

    hits = []
    for lineno, line in enumerate(candidate_src.splitlines(), 1):
        lowered = line.lower()
        for pattern in STATIC_REFERENCE_PATTERNS:
            if pattern.lower() in lowered:
                hits.append(
                    {
                        "pattern": pattern,
                        "line": lineno,
                        "excerpt": line.strip()[:160],
                    }
                )
                break
    try:
        tree = ast.parse(candidate_src)
    except SyntaxError:
        return hits
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            mods = [n.name for n in node.names]
        elif isinstance(node, ast.ImportFrom):
            mods = [node.module or ""]
        else:
            continue
        for mod in mods:
            lowered_mod = mod.lower()
            if "ref" in lowered_mod or "problem" in lowered_mod or "baseline" in lowered_mod:
                hits.append(
                    {
                        "pattern": f"import:{mod}",
                        "line": getattr(node, "lineno", 0),
                        "excerpt": f"import of {mod}",
                    }
                )
    return hits


def runtime_reference_modules(task_dir):
    """Modules executed from the task's reference tree -- from the watch
    log, not from sys.modules.

    Two dead ends shaped this. A sys.modules scan misses the cheat
    entirely: importlib's exec_module does not register the module, and
    the eval drops its references on return, so CPython frees the module
    object before any post-eval scan runs -- verified live, the scan came
    back empty against a cheating candidate. A gc-object walk finds it
    only while something still references it: same lifetime problem.
    What cannot be evaded is the execution itself: every file the import
    system runs funnels through _LoaderBasics.exec_module (SourceFileLoader
    does not override it -- verified against the MRO), whatever name it
    was loaded under. The watch patches exactly that point around the
    eval, logging origins under <task>/ref. Plain `import problem` with a
    doctored sys.path funnels through the same point. exec() of a string
    does not -- no file is executed -- and stays a static question.
    Returns sorted [(name, file)], deduplicated."""
    key = str(pathlib.Path(task_dir).resolve())
    return sorted(_REF_EXEC_LOG.get(key, {}).items())


# Per-task-dir log of reference-tree file executions, filled by the watch.
_REF_EXEC_LOG = {}


def _under_ref(origin, task_key):
    try:
        resolved = str(pathlib.Path(origin).resolve())
    except OSError:
        return False
    root = task_key + os.sep + "ref"
    return resolved == root or resolved.startswith(root + os.sep)


class _RefExecWatch:
    """Patch _LoaderBasics.exec_module around candidate execution, logging
    every file run from the task's reference tree.

    Installed only around the eval and harness checks (torch and friends
    are already imported by then, so the per-import cost lands on a
    handful of loads, not thousands). Always removed in a finally: a
    leaked patch would log the harness's own later imports. Reentrant
    installs reuse the active patch; the log is per task dir.
    """

    def __init__(self, task_dir):
        import _frozen_importlib_external as frozen

        self._frozen = frozen
        self._key = str(pathlib.Path(task_dir).resolve())
        self._real = None

    def __enter__(self):
        base = self._frozen._LoaderBasics
        if getattr(base.exec_module, "_kb_ref_watch", False):
            return self
        real = base.exec_module
        key = self._key
        log = _REF_EXEC_LOG.setdefault(key, {})

        def patched(loader, module):
            spec = getattr(module, "__spec__", None)
            origin = getattr(spec, "origin", None)
            if origin and _under_ref(origin, key):
                log[getattr(module, "__name__", "?")] = str(origin)
            return real(loader, module)

        patched._kb_ref_watch = True
        base.exec_module = patched
        self._real = real
        return self

    def __exit__(self, *exc):
        if self._real is not None:
            self._frozen._LoaderBasics.exec_module = self._real
            self._real = None
        return False


# Stated tolerance for the wall cross-check below: the reported kernel
# time may not exceed the harness-measured wall time by more than this
# fraction. Timer resolution is microseconds against seconds of wall, so
# 5% is all slack and no signal. Anything beyond it is impossible -- the
# kernel cannot burn more time than elapsed -- not merely slow.
DIVERGENCE_TOL = 0.05


def _divergent(reported_ms, perf_trials, wall_s):
    """Whether a reported measurement is impossible against the wall clock.

    `reported_ms` is the mean per-trial milliseconds the eval claims;
    `wall_s` is harness-owned seconds around the whole eval, taken with
    the pre-saved clock the candidate cannot have rebound. Only the
    impossible direction counts: a tiny claim against a large wall is a
    matter for the plausibility floor (bead farmerbob-x81s.6), not for
    this check."""
    return (reported_ms * perf_trials / 1000.0) > wall_s * (1.0 + DIVERGENCE_TOL)


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
        task_dir = args.manifest or _task_dir_from_ref(args.ref)
        precision, manifest_trials, determinism = _read_correctness_manifest(task_dir)
    except ValueError as e:
        print(f"verify: misconfigured manifest: {e}", file=sys.stderr)
        return 2
    trials = args.trials if args.trials is not None else manifest_trials
    if args.precision is not None:
        if args.precision not in PRECISION_TOLERANCES:
            print(f"verify: unknown --precision {args.precision!r}", file=sys.stderr)
            return 2
        precision = args.precision
    tol = PRECISION_TOLERANCES[precision]
    clock_before = _snapshot_clock()
    try:
        kb_eval = _load_kb()
    except Exception as e:
        print(f"verify: cannot load kernelbench.eval: {e}", file=sys.stderr)
        return 2
    candidate = _normalise_candidate(candidate_raw)
    # The watch logs every file the import system executes from the
    # task's ref tree -- the runtime answer to reference-calling. Around
    # the eval only here; the harness checks get their own below.
    with _RefExecWatch(task_dir):
        try:
            result = _call_eval_quietly(
                kb_eval.eval_kernel_against_ref,
                ref,
                candidate,
                num_correct_trials=trials,
                measure_performance=False,
                device=args.device,
                precision=_dtype_from_string(precision),
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
    moved = _check_clock(clock_before)
    clock = "ok" if not moved else f"tampered:{','.join(moved)}"
    # The harness's own confirmation: compare on device, shape, dtype and
    # the task-stated tolerance, and report which axis failed. Runs only
    # when the eval passed -- a failed eval already names its axis above.
    # A harness that cannot obtain its second opinion keeps the eval's
    # verdict rather than overruling it.
    failed_axis = _axis_of_eval_failure(result) if not result.correctness else None
    harness = ""
    specialised = False
    heldout_seed = None
    if result.correctness:
        try:
            with _RefExecWatch(task_dir):
                report = _harness_check(ref, candidate_raw, precision, args.device, HARNESS_SEED)
        except Exception as e:
            print(f"verify: harness check errored, keeping eval verdict: {e}", file=sys.stderr)
            report = {"failed_axis": None, "ref_dtype": "?", "candidate_dtype": "?", "device": args.device}
        failed_axis = report["failed_axis"]
        harness = (
            f" | harness {precision} tol={tol}:"
            f"ref={report['ref_dtype']} cand={report['candidate_dtype']} "
            f"axis={failed_axis or 'pass'}"
        )
        if failed_axis is not None:
            result_correct = False
        else:
            # Held-out set (bead farmerbob-x81s.4): inputs from a seed the
            # agent never had, generated AFTER the agent finished -- this
            # process runs post-dispatch, so a per-run secret is fresh by
            # construction. Same comparison, same provenance. Passing the
            # visible sets and failing this one is not "incorrect", it is
            # SPECIALISED, and lumping them together would destroy the most
            # interesting measurement this harness can make. The seed is
            # recorded (and re-runnable via --heldout-seed) so the verdict
            # can be re-checked later.
            import secrets

            heldout_seed = args.heldout_seed
            if heldout_seed is None:
                heldout_seed = secrets.randbits(31)
            try:
                with _RefExecWatch(task_dir):
                    held = _harness_check(ref, candidate_raw, precision, args.device, heldout_seed)
            except Exception as e:
                print(f"verify: held-out check errored, keeping eval verdict: {e}", file=sys.stderr)
                held = {"failed_axis": None}
            if held["failed_axis"] is not None:
                specialised = True
                result_correct = False
                failed_axis = "specialised"
                harness += f" | heldout seed={heldout_seed}: SPECIALISED"
            else:
                result_correct = True
                harness += f" | heldout seed={heldout_seed}: pass"
    else:
        result_correct = False
        harness = f" | harness {precision} tol={tol} trials={trials} axis={failed_axis or 'none'}"
    # Reference-call detectors (bead farmerbob-x81s.5). Static hits are
    # computed from the source alone -- a question, reported with line
    # numbers, never scored on. The runtime scan runs after every stage
    # that executes candidate code: any module executed from the task's
    # own reference tree is an answer, and the run takes its own verdict.
    static_hits = static_reference_hits(candidate_raw)
    runtime_modules = [list(m) for m in runtime_reference_modules(task_dir)]
    reference_call = bool(runtime_modules)
    questions = f" | static: {len(static_hits)} questions" if static_hits else ""
    if reference_call:
        result_correct = False
    # Determinism against the task's declaration (bead farmerbob-x81s.7):
    # the confirmation report ran the candidate twice under one seed. A
    # mismatch under `required` fails with the nondeterminism axis; under
    # `allowed` the run stands and the observation is recorded. When the
    # confirmation never ran, there is no observation -- null, not False.
    confirm = report if result.correctness else None
    deterministic = (confirm or {}).get("deterministic")
    if result_correct and deterministic is False:
        if determinism == "required":
            result_correct = False
            failed_axis = "nondeterminism"
            harness += " | determinism: RACY vs required"
        else:
            harness += " | determinism: racy-but-allowed"
    print(
        json.dumps(
            {
                "correct": bool(result_correct),
                "detail": detail + harness + questions,
                "clock": clock,
                "tolerance": tol,
                "precision": precision,
                "trials": trials,
                "failed_axis": failed_axis,
                "specialised": specialised,
                "heldout_seed": heldout_seed,
                "static_hits": static_hits,
                "runtime_hit": reference_call,
                "runtime_modules": runtime_modules,
                "reference_call": reference_call,
                "deterministic": deterministic,
            }
        )
    )
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
        task_dir = args.manifest or _task_dir_from_ref(args.ref)
        precision, manifest_trials, determinism = _read_correctness_manifest(task_dir)
    except ValueError as e:
        print(f"bench: misconfigured manifest: {e}", file=sys.stderr)
        return 2
    if args.precision is not None:
        if args.precision not in PRECISION_TOLERANCES:
            print(f"bench: unknown --precision {args.precision!r}", file=sys.stderr)
            return 2
        precision = args.precision
    tol = PRECISION_TOLERANCES[precision]
    try:
        kb_eval = _load_kb()
    except Exception as e:
        print(f"bench: cannot load kernelbench.eval: {e}", file=sys.stderr)
        return 2
    candidate = _normalise_candidate(candidate_raw)
    clock_before = _snapshot_clock()
    wall_start = _REAL_PERF_COUNTER()
    try:
        result = _call_eval_quietly(
            kb_eval.eval_kernel_against_ref,
            ref,
            candidate,
            num_correct_trials=manifest_trials,
            measure_performance=True,
            num_perf_trials=args.perf_trials,
            device=args.device,
            precision=_dtype_from_string(precision),
        )
    except AssertionError as e:
        print(f"bench: instrument assertion: {e}", file=sys.stderr)
        return 2
    except Exception as e:
        print(f"bench: eval crashed: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    wall_s = _REAL_PERF_COUNTER() - wall_start
    moved = _check_clock(clock_before)
    if moved:
        # The candidate rebound a timing primitive: nothing this process
        # measured can be trusted, so no ms is printed at all. Exits 1, the
        # same bad-trial accounting as a wrong kernel -- but the stdout
        # names the verdict, so the harness reads clock-tamper, not noise.
        print(json.dumps({"clock": f"tampered:{','.join(moved)}", "detail": "timing primitive rebound during eval"}))
        return 1
    if result is None or not result.correctness:
        # Wrong kernel: no measurement. Exit 1 so the harness counts a bad
        # trial, never a sample.
        return 1
    try:
        report = _harness_check(ref, candidate_raw, precision, args.device, HARNESS_SEED)
    except Exception as e:
        print(f"bench: harness check errored, keeping eval verdict: {e}", file=sys.stderr)
        report = {"failed_axis": None, "deterministic": None}
    if report["failed_axis"] is not None:
        # Numerically wrong on the harness's own inputs (a lucky eval pass
        # over a racy kernel reads the same way): no measurement, and the
        # axis rides along for the log. No ms is printed, ever, for a run
        # the harness would not score.
        print(
            json.dumps(
                {
                    "correct": False,
                    "detail": f"harness {precision} tol={tol}: axis={report['failed_axis']}",
                    "clock": "ok",
                    "tolerance": tol,
                    "precision": precision,
                    "trials": manifest_trials,
                    "failed_axis": report["failed_axis"],
                    "deterministic": report.get("deterministic"),
                }
            )
        )
        return 1
    if report.get("deterministic") is False and determinism == "required":
        # Racy under a task that demands determinism: no measurement.
        print(
            json.dumps(
                {
                    "correct": False,
                    "detail": f"harness {precision}: nondeterministic vs required",
                    "clock": "ok",
                    "tolerance": tol,
                    "precision": precision,
                    "trials": manifest_trials,
                    "failed_axis": "nondeterminism",
                    "deterministic": False,
                }
            )
        )
        return 1
    ms = float(result.runtime)
    if not (ms == ms and ms != float("inf") and ms != float("-inf")) or ms <= 0:
        return 1
    if _divergent(ms, args.perf_trials, wall_s):
        # Impossible, not slow: the eval claims more kernel time than
        # elapsed on the harness-owned wall clock. No ms is printed.
        print(
            json.dumps(
                {
                    "clock": f"divergent:reported={ms}ms x {args.perf_trials} trials > wall={wall_s:.2f}s",
                    "detail": "reported kernel time exceeds harness wall time",
                }
            )
        )
        return 1
    metrics = {
        "ref_ms": float(result.ref_runtime),
        "unit": "ms",
        "correctness_trials": result.metadata.get("correctness_trials", ""),
        "io_bytes": report.get("io_bytes", 0),
    }
    print(
        json.dumps(
            {
                "ms": ms,
                "metrics": metrics,
                "clock": "ok",
                "tolerance": tol,
                "precision": precision,
                "trials": manifest_trials,
                "failed_axis": None,
                "deterministic": report.get("deterministic"),
            }
        )
    )
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="KernelBench eval bridge")
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("verify", "bench"):
        p = sub.add_parser(name)
        p.add_argument("--ref", required=True)
        p.add_argument("--candidate", required=True)
        p.add_argument("--device", default=os.environ.get("KB_DEVICE", "cuda:0"))
        # Task-stated correctness knobs live in <task>/task.toml (derived
        # from --ref when --manifest is absent); explicit flags win.
        p.add_argument("--manifest", default=None)
        p.add_argument("--precision", default=None)
        if name == "verify":
            p.add_argument("--trials", type=int, default=None)
            # Re-run a recorded held-out set: the verdict names its seed so
            # it can be re-checked later from the artifact store.
            p.add_argument("--heldout-seed", type=int, default=None)
        else:
            p.add_argument("--perf-trials", type=int, default=10)
    args = parser.parse_args(argv)
    if args.cmd == "verify":
        return cmd_verify(args)
    return cmd_bench(args)


if __name__ == "__main__":
    sys.exit(main())
