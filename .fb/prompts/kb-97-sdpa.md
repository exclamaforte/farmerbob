# Task: beat the baseline on KernelBench 97_ScaledDotProductAttention

You are optimizing a CUDA kernel for research evaluation. The task directory is:

  /home/gabe/.local/share/farmerbob/tasks/kernelbench/level1/97_ScaledDotProductAttention

It contains:

  candidate.py   WHAT YOU EDIT -- starts as a copy of the reference (correct, 1.0x)
  ref/problem.py the reference you are scored against -- DO NOT TOUCH
  verify.sh      correctness check -- DO NOT TOUCH
  bench.sh       timing -- DO NOT TOUCH
  task.toml      manifest -- DO NOT TOUCH
  setup.sh       env prep -- DO NOT TOUCH

## What to do

Edit ONLY `candidate.py` in that directory. Do not create, modify, or delete any other file
anywhere, including in your working directory. Your working directory is a scratch checkout;
the task lives at the absolute path above.

Keep the class named `Model` or `ModelNew` (the harness rewrites `Model` to `ModelNew`
in memory and never edits the file; any other name fails to import).

Correctness is judged FIRST and a wrong kernel scores nothing however fast it is.
A correct kernel is timed; faster than baseline is a win.

## The target

Baseline (already measured, RTX 5090, torch 2.14.0+cu130, cuda 13.0, driver 610.57.04):

  median 32.7 ms, IQR 0.1 ms, 5 trials

Your job: beat 32.7 ms with a correct kernel. An arm that does not know the target
cannot aim at it.

Current candidate.py is:

```python
import torch
import torch.nn as nn

class Model(nn.Module):
    def __init__(self):
        super(Model, self).__init__()

    def forward(self, Q: torch.Tensor, K: torch.Tensor, V: torch.Tensor) -> torch.Tensor:
        out = torch.nn.functional.scaled_dot_product_attention(Q, K, V)
        return out
```

with batch_size=32, num_heads=32, sequence_length=512, embedding_dimension=1024.

Attention is the best-documented fusion win in the literature: a competent optimization
has a genuine path (flash-attention style fusion, memory-efficient attention, dtype/
layout changes that preserve numerics) rather than needing to invent one. Do NOT just
call the reference implementation from a wrapper -- that is correct at 1.0x and teaches
nothing.

## How to check your work

The kb-env python is `/home/gabe/.local/share/farmerbob/kb-env/bin/python`
(venv --system-site-packages, reuses system torch, adds tqdm/pydantic/dotenv).
KernelBench src is `/home/gabe/KernelBench/src` (KB_SRC).

  ./verify.sh   # from the task dir: prints {"correct": bool, "detail": ...}, exit 0 when instrument ran
  ./bench.sh    # prints {"ms": float, ...}, exit 0 only with a measurement

Or via fb:

  fb bench /home/gabe/.local/share/farmerbob/tasks/kernelbench/level1/97_ScaledDotProductAttention

Run verify before bench. If verify says incorrect, fix correctness before timing.

## Rules

- Edit only candidate.py in the task directory above. Touching ref/problem.py, verify.sh,
  bench.sh, task.toml, or setup.sh voids the run.
- Do not exfiltrate, do not install packages, do not reboot. GPU is cuda:0 RTX 5090.
- Timeout: task.toml timeout_s=600, min_trials=3. Keep iterations quick; verify is cheap,
  full bench is ~10 perf trials.
- Write a short handoff to stdout when done: what you changed, verify output, bench
  numbers, and whether you believe you beat 32.7 ms.

## Scoring (for your information, fb re-runs everything itself)

Gate: correctness true. Scored: speedup vs 32.7 ms median. A correct-but-slower novel
approach still counts as evidence; a null result ("free arms cannot beat torch SDPA")
is a real result -- record it honestly rather than faking a win.
