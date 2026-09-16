# farmerbob implementation notes

Running log kept by the planning agent while farming work out to implementers.
Design decisions live in beads (`bd list --type=decision`); this file is for what
the act of building actually taught us.

## Bootstrap paradox

farmerbob exists to dispatch agents into confined worktrees and score them. It
does not exist yet, so `fb-dispatch.sh` does that by hand: worktree per run,
`systemd-run --user --scope` for confinement, build+test gate, JSON result blob.

That script is the specification for the daemon, discovered by using it:

| fb-dispatch.sh does by hand          | becomes               |
|--------------------------------------|-----------------------|
| `git worktree add -b fb/<bead>/<src>` | `wt-provision`        |
| `systemd-run --scope -p MemoryMax=…`  | `slot-cgroup`         |
| case statement over launchers         | `adp-trait`           |
| `cargo build && cargo test` gate      | `exp-contract` verify |
| the `.json` result blob               | `fnd-store` results   |
| `for S in …; do … & done; wait`       | `slot-table` admission|

Things doing it by hand already exposed:

- **The build/test gate has to be farmerbob's, not the agent's.** An agent
  reporting success is not evidence. Every run is re-verified out-of-band by the
  harness, which is why `build`/`test` are separate fields from `rc`.
- **Prompt delivery needs a file, not argv.** A 65-line prompt through
  `bash -c` quoting was the first thing that broke. The daemon should write the
  task to a file in the worktree and point the agent at it.
- **`launcher` is a per-source property, not a per-kind one.** `ori opencode`
  and plain `opencode` are the same `kind` but different launchers, because ori
  replaces the environment rather than extending it.

## Round 1: competitive implementation of `fnd-core`

All 11 non-Sonnet implementers were given an identical, fully-specified task
(the domain model: pure types, no I/O, unit tests required). Sonnet is excluded
because the planning agent is Sonnet-family and would compete with itself for
tokens.

Rationale for competing everyone on one task rather than fanning out immediately:
the domain model is the foundation every other crate depends on, it is pure
types so correctness is unambiguous, and an identical task across 11 models is
the only way to get a *comparable* first ranking. Fan-out happens in round 2 on
top of the winner.

This also dogfoods the `wt-compare` / `wt-select` / `wt-sweep` flow before any
of it is written.

### Grading

Objective, from the harness: `build` pass/fail, `test` pass/fail, duration,
lines added. Subjective, from the planning agent reading the diffs: whether the
state machine is actually encoded or faked, whether newtype ids are real
newtypes, whether the tests test anything.

## Provider health checks must exercise a completion

The IFM key was rotated mid-session. A liveness check against
`GET /v1/models` returned **HTTP 200 on the dead key** — that endpoint does not
validate auth. Only `/v1/chat/completions` rejects it, which is how the failure
finally surfaced: an implementer run died with
`Incorrect API key provided: IFM-v1_C********qFrs`.

So `fb doctor`'s per-provider check cannot be a catalogue fetch. It has to be a
minimal real completion (1 token, `max_tokens=1`), because that is the only call
that proves the credential works for the thing farmerbob actually does.

Generalised: **a health check must exercise the same code path as the work.**
This is the same lesson as the scoring gate above and as `gpu-verify` — in all
three cases the cheap proxy for "is this working" is the thing that lies.

## The recurring failure mode: cheap proxies that lie

Three times in one session, a check that was supposed to tell us something was
working returned success while the thing was broken. Each was found by accident,
none by an error message:

| Proxy that returned success      | What was actually true            | Bead          |
|----------------------------------|-----------------------------------|---------------|
| `cargo test` → "ok, 0 tests"     | agent wrote nothing, exited 1     | farmerbob-slh |
| `GET /v1/models` → HTTP 200      | API key rotated and dead          | farmerbob-iiw |
| no error from `systemd-run`      | wrapper absent; zero confinement  | farmerbob-8y8 |

The pattern: **absence of failure was read as evidence of success.** In each case
the correct check was to assert something positive — tests actually executed, a
completion actually returned, the process actually landed in the expected cgroup.

This is the same shape as `gpu-verify`, which was specified early on for a
different reason: before scoring a GPU measurement, enumerate compute contexts and
prove the lease holder was the sole tenant rather than assuming the lease worked.
That instinct was right, and it generalises. Every place farmerbob is about to
record a result, it must be able to answer "what is my evidence?" — not "did
anything complain?"

Concretely this becomes:
- `slot-cgroup` reads back the effective `memory.max`/`cpu.max` and refuses a run
  whose scope did not materialise.
- `exp-contract` requires a task to declare its minimum evidence of work.
- `ux-doctor` health-checks providers with a real 1-token completion.
- `dmn-recover` already works this way by design: it asks systemd what is actually
  alive rather than trusting the database.

## CLI design, informed by being the planning agent

Spent a session hand-driving the bootstrap harness, i.e. doing by hand exactly what
the planning agent will ask `fb` to do. Four things that hurt, and what they imply:

1. **The spec must be a file, not argv.** The first dispatch died on bash quoting a
   65-line prompt. `fb run` takes `--spec <PATH|->`; the bare string form is sugar
   that still materialises a spec file.
2. **Dispatch must not block.** `& … wait` cost all visibility into 11 parallel runs.
   `fb run` returns trial + run ids immediately; `--wait` is opt-in.
3. **"Hung or thinking?" was the most frequent question and had no answer.** Fell
   back to `stat -c %Y` on logfiles and `pgrep`. So `fb status` carries liveness, not
   just state: idle-time, bytes of output, files touched. Those three columns are
   what decide whether to wait or intervene.
4. **Never surface agent self-reports.** Re-ran every build and test out of band,
   which is how the no-op agent was caught. `fb compare` shows only what farmerbob
   itself proved.

### Task identity is content-hashed

`fb run "…"` creates a real task whose id is a digest of the *normalised* spec text,
rather than an anonymous one-off. Rationale: the leaderboard is the entire point, and
cross-time ranking needs stable task identity — "codex beat mercury on task X" means
nothing unless X is the same object next week. Anonymous one-shots would give a nice
ergonomic and a leaderboard with sample size 1 everywhere.

Normalisation (BOM, line endings, trailing whitespace, blank-line runs) exists so
that trivially-reformatted specs still collide onto one task. Deliberately does NOT
lowercase or collapse internal spaces — those change meaning in a spec.

`fb task add` survives for the curated corpus (KernelBench), where ids come from
upstream problem ids instead.

## Liveness cannot be judged from log activity

`or-laguna-s-21` showed zero lines written and a logfile whose mtime had not moved in
six minutes. By every signal the harness had, it was hung, and it was about to be
dropped as a bad implementer. It was buffering stdout. It went on to produce +479
lines.

**Log-file mtime measures flushing behaviour, not progress.** Agents differ wildly
here: codex streams continuously (29KB and climbing), zcode and agy emit nothing until
they finish, opencode is somewhere between.

So `ux-status`'s "hung or thinking" answer must lean on **files touched in the
worktree**, which reflects work rather than I/O buffering, and report log activity only
as a secondary signal. Never auto-kill on log silence alone — only on a real timeout,
and say which signal fired.

Generalisation of the same rule as everywhere else in this file: the cheap proxy
(mtime) was not measuring the thing we cared about (progress).

## Routing, not ranking

The grading epic was reframed (user direction) from "leaderboard and trends" into a
**contextual bandit with Thompson sampling over a persistent attempt log**. The question
is not "which model is best" but "how likely is this arm to succeed on *this kind* of
task", and the router makes the choice rather than a human reading a table.

Round 2 produced the exact case that justifies it:

| arm            | fnd-core (tight spec, ~250L median) | fnd-cli (design a CLI surface) |
|----------------|-------------------------------------|--------------------------------|
| or-mercury-25  | PASS, 238L, **28s** — best of 11    | **NO-COMPILE**, 1031L, 8 errors|

Same arm, same harness, same prompt style. Only the bucket changed: scope local →
architectural, spec "enumerate these types" → "design this surface". A global leaderboard
built on round 1 would confidently route a large design task to the arm most likely to
fail it.

Consequences worth holding on to:

- **An arm is the whole execution configuration.** `agy` exposes
  `gemini-3.8-flash-{low,medium,high}` — three arms. The same base model via `ori opencode`
  and via its native CLI — two arms. Collapsing these into a model name mixes incompatible
  configurations into one posterior.
- **Quota parks must never touch the posterior.** A parked arm is *ineligible*, not
  unsuccessful, and a resumed run is the same attempt continuing — not attempt 2. The
  parked interval is also excluded from latency.
- **`NO-COMPILE` is not `NO-OP`.** Both are `accepted=false`, but 1031 broken lines is
  rescuable and zero lines is not. That gap is what makes `rescue_effort` worth recording
  separately rather than folding into the success bit.
- **Statistics are a derived view.** The definition of success will change; posteriors must
  be recomputable from the append-only log rather than relabelled in place.

## The observer keeps corrupting the observation

Running `cargo build/test/clippy` to score candidates *while they were still working* took
the same `target/` lock the agents were using. Three harms, all of them measurement
corrupting measurement:

1. harness and agent block each other;
2. time the agent spends blocked on the harness is charged to the agent, corrupting exactly
   the latency the router will consume;
3. `fb-timing.sh` reads worktree mtimes, and a harness build rewrites them — so the timing
   reconstruction measures the observer.

This is `gpu-verify` one level up. That bead exists because a concurrent CUDA context
invalidates a timing; a concurrent *build* invalidates a run's latency the same way. Which
means the lease manager generalises further than specced: **the build lock is a scarce
per-worktree resource too**, alongside the GPU and provider accounts.

Also: `NO-COMPILE` is a terminal state for one run and a transient one for another —
`ling-30-flash` passed through it, correctly diagnosed "clap derive can't resolve types
defined after the enum", and kept going. Only terminal states may be scored, which is an
independent reason not to touch a live worktree.
