# KernelBench candidate skeleton (shared fragment)

Include this section verbatim in every KernelBench task spec. It pins the
file skeleton that the first real wave proved necessary (bead
farmerbob-x81s.14): the prompt said "keep the class" and the arm kept it
while deleting everything else, assuming only the model matters. Verify
still ran -- the eval uses the reference's inputs -- so it scored mismatch
for the wrong reason first, and a follow-up turn had to restore the file.

## The file you edit

`candidate.py` in the task directory, and nothing else. It has required
parts, and they stay:

- `class Model` (or `ModelNew`): keep the name. The harness rewrites
  `Model` to `ModelNew` in memory and never edits the file; any other
  name fails to import.
- Module constants (`batch_size`, `num_heads`, `sequence_length`,
  `embedding_dimension`, and friends): keep names AND values. They
  define what is measured -- shrinking them to fake speed is
  input-specialisation with extra steps, and it voids the run.
- `get_inputs()` / `get_init_inputs()`: keep them with the same
  behavior (shapes, dtypes, distribution). The scorer reads the
  reference's copies, but the file's contract needs them: a candidate
  without them is incomplete, and "correct" on an incomplete file is
  never evidence of work.
- Imports: keep what the file needs; add what your approach needs.

## The optimization surface

The body of `Model.forward` is the surface. Additions are allowed --
helper functions, custom autograd `Function`s, Triton kernels as separate
functions, backend-selection context: real optimization lives there, and
forbidding additions would cripple every approach past trivial fusion.
Additions inherit every scrutiny the harness applies: the clock guards
reach them, the reference-call detectors read them, the held-out set
measures them.

Deletions of required parts, and value changes to constants or input
generators, are departures, not optimizations. If a required part as
specified makes the task uncomputable, say so in your handoff and
implement the closest honest thing -- do not silently return a
placeholder.
