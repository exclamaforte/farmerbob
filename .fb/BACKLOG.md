# Open bug backlog, grouped

Generated 2026-09-19 from `bd list --status=open --type=bug`. **90 bugs, 18 tasks, every bead
assigned exactly once.** Regenerate rather than edit by hand when the backlog moves.

## Why this document exists

The backlog does not converge one bead at a time. Most of these are not ninety independent
defects -- they are roughly eighteen design questions, each asked several times by a different
part of the system. Fixing the question closes the group.

A grouped task declares every file it touches. That became possible on 2026-09-19, when
`scope::Declared` went from one path to a set; before that a task spanning two files in one
crate could not pass its own gate.

## Ordering

T1, T3, T4 and T6 first. T1 is the headline deliverable's axis. T3 and T4 are the duplication
that keeps generating new bugs -- every group below has at least one bead that exists because
one rule had two implementations. T6 is the one that destroys evidence, and evidence is the
product.

T9 before T8: there is no point repairing the grafter until the matrix it produces is being read
correctly.

---

## T1 — Cost and the Pareto axis

**Files:** `crates/farmerbob-core/src/cost.rs`  
**Beads:** 6 (P0, P1, P3)  
**Status:** SPECCED - .fb/prompts/cost-honesty.md, wave157 in flight

Six ways of saying one thing: `RunCost.usd` is `Option<f64>`, so "free on a plan", "metered but
unpriced", "never measured" and "measured and disputed" are one `None` -- and a `None` that
reaches a display becomes `free`. `pricing::Billing` already has the five states, including
`MeteredUnpriced`, whose own doc comment reads "This must never read as free". cost.rs does not
use it.

- `farmerbob-xsa` **P0** — Cost axis is inferred from opencode's price table and is ~10% off the provider bill, with mixed sign
- `farmerbob-b52` **P1** — A metered arm with no recorded price is displayed as free, fabricating a zero on the Pareto board
- `farmerbob-tb92` **P1** — Cost accounting under-reports actual OpenRouter spend by ~19%
- `farmerbob-lzae` **P1** — launcher-agy is unreachable in production: pareto.rs keeps a private duplicate of the launcher mapping, and the spec forbade touching it
- `farmerbob-t7er` **P3** — cost::Launcher does not know agy: no token count to predict when a plan arm is near its cap
- `farmerbob-5gh4` **P1** — An observed provider refusal does not park the arm, so every wave re-spends slots on it

## T2 — What the scope gate looks at

**Files:** `crates/fb/src/scope_cmd.rs + crates/farmerbob-core/src/scope.rs`  
**Beads:** 4 (P1, P2)  
**Status:** open

The gate disagrees with itself about what universe of paths it observes. One bead says it
reports 777 departures for a clean arm because a pathspec was omitted; another says it sees
only `crates/`, so an arm editing a shell script or sources.toml is reported CLEAN. Those are
two sides of one unanswered question, and until it is answered the gate is both too loud and
too quiet.

- `farmerbob-3owl` **P1** — scope-cmd spec omitted the crates/ pathspec; fb scope reports 777 departures for a clean arm
- `farmerbob-f5br` **P1** — scope flagged main.rs as a departure, so every binary-crate task was marked out of scope for following its own instructions
- `farmerbob-0s8x` **P2** — The scope gate measures only crates/: an arm editing its own spec, a shell script or sources.toml is reported CLEAN
- `farmerbob-8c7q` **P1** — Spec template can order a type name the rubric forbids and the scope gate makes unfixable

## T3 — One spec-target reader

**Files:** `crates/farmerbob-core/src/target_decl.rs + precondition.rs + fb-target.sh`  
**Beads:** 7 (P0, P1, P2)  
**Status:** open

Seven scripts each reimplemented the reader for `fb:creates` / `fb:modifies`, and six of them
knew only `creates`. Every `modifies` spec was therefore unpipelineable and sat in NEEDS
CRITIQUE forever, and the autopilot once spun on one and launched nothing. `target_decl` is
the one reader; the work is deleting the other six and teaching the survivor what a marker is
(not merely where it may appear).

- `farmerbob-0d2` **P1** — fb-pipeline read only fb:creates, so every fb:modifies spec was unpipelineable and sat in NEEDS CRITIQUE forever
- `farmerbob-9mh` **P1** — Seven scripts each reimplemented the spec-target reader and six knew only fb:creates
- `farmerbob-u7sa` **P1** — fb-dispatch.sh parses fb:creates/fb:modifies markers anywhere in a prompt, including in prose examples
- `farmerbob-t8a5` **P1** — precondition spec pins WHERE a marker may appear but never WHAT a marker is
- `farmerbob-6mc` **P2** — A crate-scoped deliverable has no declaration verb, so three foundation tasks were dispatched into a pipeline that could never process them
- `farmerbob-z1p` **P1** — fb-autopilot span on an unpipelineable task and never launched any wave
- `farmerbob-m71` **P0** — Merging a winner invalidates queued task specs: validate preconditions against the live base

## T4 — One decision, one implementation

**Files:** `crates/farmerbob-core/src/{build_verdict,quota}.rs + crates/fb/src/sources.rs`  
**Beads:** 4 (P1)  
**Status:** open

The same decision implemented more than once, diverging silently. Four tools reimplemented the
verdict and each reintroduced the same bug. `quota::Registry` duplicates `fb::sources::Registry`
because core cannot depend on fb. Three `pub enum Fate` exist, two from my own specs on one day.
sources.toml declares how to launch an arm and fb-dispatch ignores it for a hardcoded table.
This is the project's most expensive recurring shape: today alone it produced a park the Rust
registry could not see.

- `farmerbob-9m7` **P1** — Single verdict function: four tools reimplemented it and each reintroduced the same bug
- `farmerbob-hf9i` **P1** — quota::Registry duplicates fb::sources::Registry because core cannot depend on fb
- `farmerbob-vtp1` **P1** — farmerbob-core now has three pub enum Fate; two of them came from my specs on the same day, and nothing checks a spec's Exact API against existing names
- `farmerbob-ozv0` **P1** — sources.toml declares how to launch an arm; fb-dispatch ignores it and uses its own hardcoded table

## T5 — Counting tests

**Files:** `crates/fb/src/score.rs + crates/farmerbob-core/src/testout.rs`  
**Beads:** 6 (P1, P2)  
**Status:** open

`tests_run` is the crate's total presented as the candidate's contribution -- an arm that wrote
6 tests has a record saying 547. `test_delta` and `score-delta` fixed the arithmetic; these are
the remaining readers, plus counting bugs of their own (re-running the suite inflates the count;
one flaky test reads as NO-TESTS).

- `farmerbob-apxp` **P1** — tests_run is the crate's total and is displayed as the candidate's: sem-port's arm wrote 6 tests and its record says 547
- `farmerbob-jd2.8` **P1** — score.json tests_run is the whole crate's test count, not what the candidate wrote
- `farmerbob-xbu1` **P1** — tests_run sums every cargo test invocation, so re-running the suite inflates the count -- 124 of 421 run logs affected
- `farmerbob-s5qf` **P1** — One flaky test turns a correct run into NO-TESTS: the dispatch counter reads an absent 'test result: ok' line as zero
- `farmerbob-evuz` **P2** — tests_delta serialises Missing as null, discarding the Absent reason
- `farmerbob-jd2.12` **P1** — fb score --crate defaulted to farmerbob-core, so a bare invocation measured a crate the candidates never touched

## T6 — Worktree lifecycle

**Files:** `fb-dispatch.sh + fb-score.sh + crates/farmerbob-core/src/wtreap.rs`  
**Beads:** 9 (P0, P1, P2, P3)  
**Status:** open

A candidate's work lives only in its worktree, uncommitted, and several things delete it. The
hidden conformance suites shipped inside every worktree; an arm can register worktrees in the
parent repo from inside its sandbox; the dispatch lock is taken AFTER the worktree is cleared;
scoring runs inside a live worktree. fb-archive.sh is today's stopgap and is not in the harness.

- `farmerbob-w00` **P0** — The hidden conformance suites were shipped inside every agent worktree
- `farmerbob-2xa` **P1** — Agents can register git worktrees in the parent repo from inside their sandbox
- `farmerbob-4a02` **P1** — speccheck/dispatch lock is acquired after the worktree is cleared, not before
- `farmerbob-5ar3` **P1** — Reaping a worktree destroys the candidate's work: nothing ever commits or archives it
- `farmerbob-qe3` **P1** — Scoring must never run inside a live worktree
- `farmerbob-l2yw` **P1** — The port tasks were dispatched into worktrees with the harness deleted, so 8 of 12 arms could not find the script they were told to port
- `farmerbob-6psh` **P2** — wt-reap spec leaves contradictory duplicate Worktree entries unpinned; reapable can emit a registered dir
- `farmerbob-plgz` **P3** — wtreap::safe_to_reap filters with Vec::contains where the doc claims set subtraction
- `farmerbob-m4i` **P1** — Hand diagnostics leaked 4.6G into tmpfs, starving the machine the admission controller was budgeting

## T7 — How a critic is run

**Files:** `crates/fb/src/critique.rs + fb-critique.sh`  
**Beads:** 6 (P1, P2)  
**Status:** open

Critics get none of the protections implementers get: no systemd scope, no bwrap, no timeout.
One wandering critic stalls the pipeline and is invisible. A rate-limited critic reports "no
critique written" -- the same line as a critic that ran and declined to write. And `fb brief`
renders a critique as blank when it filed no CLAIMS, hiding its JUDGEMENTS entirely.

- `farmerbob-vi7` **P1** — Critics run unconfined: fb-critique bypasses the systemd scope and bwrap that implementers get
- `farmerbob-epb5` **P1** — fb-critique launches agents with no timeout and no systemd scope: one wandering critic stalls the pipeline and is invisible to admission control
- `farmerbob-kk8w` **P2** — fb critique ignores a rate-limited critic: no park, no retry, and the report says 'no critique written'
- `farmerbob-b7c5` **P1** — fb brief renders a critique as blank when it filed no CLAIMS, hiding its JUDGEMENTS and FOLLOWUPS
- `farmerbob-fp7q` **P1** — Showing the critic only the declared target hides a split-module deliverable, so the reviewable artefact is a header
- `farmerbob-vqg3` **P1** — verify_quotes ignores backticks, so it misses the projection critics actually write

## T8 — Cross-examination grafting

**Files:** `fb-crossx.sh + crates/farmerbob-core/src/graft.rs`  
**Beads:** 6 (P1)  
**Status:** open

The grafter mangles the suites it moves. It dropped the source file's imports, then the fix for
that injected duplicates, which are a hard error. It renames `mod tests` but not every module.
It still VOIDs on `crates/fb` tasks, and the cause is the graft rather than the strip.

- `farmerbob-74l` **P1** — Cross-examination grafter dropped the source file's imports, making a suite fail against its own implementation
- `farmerbob-4xv` **P1** — The grafter's fix for dropped imports now injects duplicates, and a duplicate explicit import is a hard error
- `farmerbob-kyki` **P1** — fb-crossx: rename EVERY module in a grafted suite, not just 'mod tests'
- `farmerbob-e59v` **P1** — crossx still VOIDs on crates/fb tasks, and it is the GRAFT not the strip
- `farmerbob-jd2.13` **P1** — The merged crossx port waited the full timeout on every cell, and builds-plus-unit-tests passed it
- `farmerbob-uw3j` **P1** — crossx concurrency is a constant while every other admission decision is computed; 4 slots on crate fb makes the pipeline killable

## T9 — What a cross-examination matrix MEANS

**Files:** `crates/farmerbob-core/src/{crossx,divergence,field_shape}.rs`  
**Beads:** 4 (P1, P2)  
**Status:** open

Separate from T8: even a correctly grafted matrix is being READ wrong. Total API divergence is a
fifth cell shape and means the spec under-specified the interface, but crossx reports it as "no
signal". A 2x2 partition is evidence of spec ambiguity, not four discriminating suites. At N=3
the detector cannot tell a correct strict suite from an over-fitted one.

- `farmerbob-4j6` **P1** — Total API divergence is a fifth cross-examination shape, and it means the spec under-specified the interface -- crossx reports it as 'no signal'
- `farmerbob-gm4` **P1** — Cross-examination should name the 2x2 partition shape as spec ambiguity, not as four discriminating suites
- `farmerbob-ltm1` **P2** — At N=3 the crossx partition detector cannot tell a correct strict suite from an over-fitted one
- `farmerbob-g0u5` **P2** — Arms keep adding unpinned public API and testing through it, voiding crossx cells

## T10 — Quota, parking and re-probing

**Files:** `crates/farmerbob-core/src/quota.rs + crates/fb/src/sources.rs + fb-eligible.sh`  
**Beads:** 8 (P1, P2, P3)  
**Status:** open

`parked_until` exists and until today the Rust registry could not read it. Now it can, and the
remaining half is that nothing SETS it from an observation and nothing re-probes an expired one
-- five free arms sat eligible and undispatched for 29 hours. Plus the tracker's own bugs:
`due()` deletes the bucket entry, making a backoff reset unobservable.

- `farmerbob-tdl7` **P1** — Nothing re-probes an expired park; five free arms sat eligible and undispatched for 29 hours
- `farmerbob-j77x` **P1** — OpenRouter free tier exhausted account-wide: 12 of 16 arms dead until reset, and speccheck saw it two seconds in while park could not
- `farmerbob-dk7y` **P2** — fb park cannot see a speccheck log, so a quota-exhausted arm burns a second launch
- `farmerbob-syyo` **P2** — QuotaTracker::due() deletes the bucket entry, making succeeded()'s backoff reset unobservable
- `farmerbob-fptc` **P3** — quota::due() no longer mutates but still takes &mut self
- `farmerbob-al2y` **P2** — fb-eligible's redundancy message asserts 'the same model' for arms that are not
- `farmerbob-0919` **P2** — The registry claims a reasoning effort that nothing verifies the run actually used
- `farmerbob-7vnp` **P3** — cerebras free routes answer 'Payment required' -- both arms disabled

## T11 — Specs that contradict themselves

**Files:** `.fb/prompts/_rubric.md + crates/farmerbob-core/src/speclint_shape.rs`  
**Beads:** 5 (P0, P1)  
**Status:** open

Specs that cannot be satisfied. A boundary that reads as a demand but was meant as delegation
voided a whole matrix. Every spec contradicts itself on the handoff file. One pinned a closed
enum AND ordered reuse of another type, with no legal move. These are caught one at a time by
spec critics today; speclint should catch the SHAPE before dispatch.

- `farmerbob-vg0` **P0** — The rubric must be disclosed to the arms, or you are measuring style priors, not capability
- `farmerbob-7chc` **P1** — 'state which one wins and pin it' reads as a spec demand, not as delegation; it voided a whole matrix
- `farmerbob-qfxv` **P1** — Every spec contradicts itself on the handoff file, and speclint-shape's DelegatedFormat rule is unpinned in a way that made clause 10 untestable
- `farmerbob-udp5` **P1** — The stage-key spec pinned a closed enum AND told the arm to reuse StageOutcome; the two cannot both be obeyed, and master now carries both types
- `farmerbob-li6o` **P1** — SPEC DEFECT: KnownDefect.evidence is a Vec<String> that cannot say which of the four labels are present

## T12 — Artefacts that outlive the run that made them

**Files:** `fb-speccheck.sh + .fb/queue + crates/fb/src/critique.rs`  
**Beads:** 4 (P1, P2, P3)  
**Status:** open

An artefact outliving the run that produced it, and nothing noticing. A spec critique is reused
against a spec that has since changed. A spec edited mid-wave shows the critic and the
implementer different texts. A spent wave file is resurrected by any git tree restore and the
autopilot relaunches merged work. All four want the same fix: content-hash the input, record it
beside the artefact, and re-run when they differ.

- `farmerbob-5esi` **P3** — Editing a spec while its wave is in flight can show the critic and the implementer different texts
- `farmerbob-03vv` **P2** — fb-speccheck reuses a critique written against an older version of the spec
- `farmerbob-dkbu` **P2** — A spec critique that failed to be written can never be recovered: fb-speccheck refuses once the candidate worktree exists
- `farmerbob-29r8` **P1** — A spent wave file is resurrected by any git tree restore, and the autopilot relaunches merged tasks

## T13 — Never mutate a running script

**Files:** `fb-dispatch.sh + fb-pipeline.sh + fb-autopilot.sh`  
**Beads:** 3 (P1, P2)  
**Status:** open

bash reads scripts incrementally, so editing a running one corrupts it mid-execution. This has
happened twice. It is a discipline today (a rule in the tick prompt) and should be a lock.

- `farmerbob-qoe` **P1** — Never mutate a running dispatch script; run records must not be the only evidence
- `farmerbob-n1vl` **P1** — I edited fb-dispatch.sh while wave72 was live; a running dispatcher read the file mid-write
- `farmerbob-a27c` **P2** — Editing fb-pipeline.sh while a pipeline runs kills it: bash reads scripts incrementally

## T14 — Instruments that measure the wrong thing

**Files:** `crates/farmerbob-core/src/{resource,liveness}.rs + fb-score.sh`  
**Beads:** 5 (P1)  
**Status:** open

Instruments pointed at the wrong target. `mem_peak_mb` samples foreign cgroups because `pgrep -f`
matches the harness's own git processes. Process-name matching is not process identity. An
orchestrator-initiated kill is recorded as an arm failure. clippy is scored as an absolute count,
so candidates are charged for pre-existing lint debt.

- `farmerbob-05p` **P1** — mem_peak_mb samples foreign cgroups: pgrep -f matches the harness's own git/worktree processes
- `farmerbob-7e2` **P1** — Process-name matching is not process identity
- `farmerbob-hky` **P1** — Orchestrator-initiated kills are infrastructure outcomes, not arm failures
- `farmerbob-0fx` **P1** — clippy is scored as an absolute count, so candidates are charged for pre-existing lint debt
- `farmerbob-3l6` **P1** — Adapter defaults tuned for interactive use silently corrupt orchestrated runs

## T15 — Escalation and known-defect parsing

**Files:** `crates/farmerbob-core/src/known_defect.rs + fb-prove.sh + fb-escalate.sh`  
**Beads:** 3 (P1, P2)  
**Status:** open

The escalation path's parsers. `known_defect` splits the heading on the FIRST ' -- ', so a stamp
containing one misparses the critic; it returns an empty claim when the prefix is missing; and
prove's reference veto silently passes when the task is not yet merged.

- `farmerbob-95z` **P1** — fb-prove's reference veto silently passes when the task is not yet merged
- `farmerbob-ydr3` **P2** — known_defect heading splits on the FIRST ' -- ', so a stamp carrying one misparses the critic
- `farmerbob-yjqi` **P2** — known_defect::parse returns an empty claim when the claim line lacks the critic-on-subject prefix

## T16 — Did the arm actually do the work

**Files:** `crates/fb/src/{score,doctor}.rs + crates/farmerbob-core/src/lib_diff.rs`  
**Beads:** 3 (P1)  
**Status:** open

Nothing checks that an arm did what it claimed beyond the gate. An arm stubbed a shipped command
to `-> i32 { 1 }` and scored PASS, because nothing verifies that public items the task never
mentioned still work. Eight modules are declared, tested and unreachable. `fb verify` reports
'the suite ran no tests' for every passing candidate.

- `farmerbob-wu1m` **P1** — An arm can stub out a shipped command and still score PASS: nothing checks that untouched public items survive
- `farmerbob-649f` **P1** — Eight fb modules are declared, tested, and unreachable; my own specs told every arm to add the allow(dead_code) that hides it
- `farmerbob-35x1` **P1** — fb verify reports 'the suite ran no tests' for every passing candidate: verify_gather feeds the error field to a test-log parser

## T17 — Dispatch orchestration

**Files:** `fb-dispatch.sh + fb-admit.sh + fb-speccheck.sh`  
**Beads:** 5 (P1, P2)  
**Status:** open

Sequencing around dispatch. codex's session continuation makes it re-answer the spec critique
instead of implementing. fb-admit dispatches immediately after speccheck, so nobody ever rules
on the findings. speccheck gives no per-arm progress, so a buffered launcher's silence reads
identically to a hang.

- `farmerbob-81tg` **P1** — fb-dispatch's session continuation makes codex re-answer the spec critique instead of implementing
- `farmerbob-mu7s` **P1** — fb-admit dispatches immediately after speccheck, so nobody ever rules on the spec findings
- `farmerbob-ei23` **P2** — fb-speccheck gives no per-arm progress signal, so a buffered launcher's silence reads identically to a hang
- `farmerbob-7vh` **P1** — capacity() and admit() disagree when memory_mb is zero -- in BOTH candidates
- `farmerbob-2h3k` **P2** — port-defects/codex-luna: no timeout on cargo, PID-named tempdir reused

## T18 — Formatting, already fixed today

**Files:** `hooks/pre-commit-fmt + crates/farmerbob-core/src/scope.rs`  
**Beads:** 2 (P1, P2)  
**Status:** CLOSEABLE - verify then close

Both fixed on 2026-09-19 -- the pre-commit formatter hook and scope's Semantic/FormattingOnly
distinction. Listed so they are verified and closed rather than quietly assumed.

- `farmerbob-jy1z` **P1** — Nothing keeps the repo rustfmt-clean, so scope departures regenerate after every merge
- `farmerbob-x56m` **P2** — The scope gate cannot tell a cargo-fmt departure from a semantic one, and discards good work over it

---

## Coverage check

```
beads in backlog   90
beads assigned     90
tasks              18
unassigned          0
```

Re-run the grouping after any wave that closes beads; a group whose beads are all closed should
be deleted from this file rather than left as a heading with nothing under it.
