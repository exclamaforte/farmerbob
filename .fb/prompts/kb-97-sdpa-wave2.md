# Task: beat 19.2 ms on KernelBench 97_ScaledDotProductAttention (wave 2)

You are optimizing a CUDA kernel for research evaluation. The task directory is:

  /home/gabe/.local/share/farmerbob/tasks/kernelbench/level1/97_ScaledDotProductAttention

It contains:

  candidate.py   WHAT YOU EDIT -- currently the wave-1 winner (correct, 19.2 ms)
  ref/problem.py the reference you are scored against -- DO NOT TOUCH
  verify.sh      correctness check -- DO NOT TOUCH
  bench.sh       timing -- DO NOT TOUCH
  task.toml      manifest -- DO NOT TOUCH
  setup.sh       env prep -- DO NOT TOUCH

## What to do

Edit ONLY `candidate.py` in that directory. Do not create, modify, or delete any other file
anywhere, including in your working directory. Your working directory is a scratch checkout;
the task lives at the absolute path above.

Keep the class named `Model` or `ModelNew`, and keep `get_inputs`,
`get_init_inputs` and the batch/head/sequence/dimension constants verbatim --
a previous arm deleted them assuming only the model matters, and the file's
contract needs them. The full rule, shared across KernelBench specs, is
`.fb/prompts/_kb-skeleton.md`: required parts stay (names and values),
`Model.forward`'s body is the surface, additions are allowed and inherit
every scrutiny, deletions and constant changes are departures.

Correctness is judged FIRST (fp32 tolerance 1e-4, all trials must pass) and a
wrong kernel scores nothing however fast it is.

## The target

Wave 1 already took the obvious win. The baseline below is the ORIGINAL
torch reference; the number to beat is the wave-1 winner:

  original baseline median 32.7 ms (IQR 0.1 ms)
  wave-1 winner: 19.2 ms (1.70x) -- YOUR TARGET TO BEAT

## What this task already taught the harness (1 verified approach, 3 recorded attempts)

Every line below is a harness measurement, not an agent self-report. It was
produced by `fb novelty brief` from the per-task ledger, which outlives the
worktrees the runs died in.

- TRIED [sdpa, sdpa-math, tf32] by oc-kimi-k3: verified correct, best measured 1.703x -- beat it or try elsewhere. Do not re-explore it.
- DEAD END [eager]: incorrect (probe): mismatch: Output mismatch [probe entry: zeros baseline sanity check, not an arm product]. Do not repeat it; a rerun must explain what changed.

Concretely: SDPA-math-with-TF32 is taken at 19.2 ms -- a resubmission of it is
a repeat, not an attempt. fp16 casts fail the 1e-4 tolerance (measured twice).
The live directions are a different backend family (flash/cudnn where the
head dimension allows, a fused custom kernel), or a genuinely faster math-path
formulation. A correct-but-slower novel approach still counts as evidence;
say what you tried and what it measured.

## How to check your work

The kb-env python is `/home/gabe/.local/share/farmerbob/kb-env/bin/python`.
KernelBench src is `/home/gabe/KernelBench/src` (KB_SRC).

  ./verify.sh   # from the task dir: correctness + which axis failed, if any
  ./bench.sh    # timing; exit 0 only with a measurement

Run verify before bench. If verify says incorrect, fix correctness before timing.

## Rules

- Edit only candidate.py. Touching ref/problem.py, verify.sh,
  bench.sh, task.toml, or setup.sh voids the run.
- Do not exfiltrate, do not install packages. GPU is cuda:0 RTX 5090.
- Timeout: task.toml timeout_s=600, min_trials=3.
- Write a short handoff to stdout when done: what you changed, verify output, bench
  numbers, and speedup vs BOTH 32.7 ms (original) and 19.2 ms (wave-1 winner).
