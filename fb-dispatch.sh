#!/usr/bin/env bash
# fb-dispatch — manual stand-in for farmerbob, used to bootstrap farmerbob.
#   fb-dispatch.sh <source-id> <bead-id> <prompt-file>
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

SRC="${1:?source id}"; BEAD="${2:?bead id}"; PROMPT_FILE="${3:?prompt file}"
CRATE="${4:-farmerbob-core}"
REPO="/home/gabe/Documents/farmerbob"
WT_ROOT="${FB_WT_ROOT:-$HOME/.local/share/farmerbob/worktrees}"
LOG_ROOT="${FB_LOG_ROOT:-$HOME/.local/share/farmerbob/logs}"
BASE="${FB_BASE:-HEAD}"
mkdir -p "$WT_ROOT" "$LOG_ROOT"

RUN="${BEAD}--${SRC}"; WT="$WT_ROOT/$RUN"; LOG="$LOG_ROOT/$RUN.log"
META="$LOG_ROOT/$RUN.json"; BRANCH="fb/$BEAD/$SRC"

git -C "$REPO" worktree remove --force "$WT" >/dev/null 2>&1
# `worktree remove` REFUSES a worktree whose admin directory is gone -- "is not a working

# SHARED lock on this bead, held for the whole run, taken BEFORE anything touches the worktree
# path. Two bugs in one fix.
#
# ORDER. The first version acquired this just before launching the agent -- after the block
# below had already cleared and recreated the worktree. So it stopped a spec critique and a
# dispatch from RUNNING at once but not from dispatch deleting the directory a critique was
# working in, which is the thing it was added for. Observed on wave73: "clearing an orphaned
# worktree directory" printed while fb-speccheck still held the lock and was still writing
# there.
#
# MODE. It was also EXCLUSIVE, and dispatch holds it for its entire run -- so every arm of a
# wave queued behind the first one. wave73 dispatched three arms and ran them sequentially,
# each waiting 15-40 minutes for the last. That is a throughput regression I introduced hours
# after establishing that queue depth, not machinery, was the parallelism bottleneck.
#
# Dispatchers never contend with each other: each arm owns its own <bead>--<arm> directory.
# They contend only with fb-speccheck, which owns every one of those paths for the bead. That
# is a reader/writer relationship, so dispatch takes it SHARED and fb-speccheck EXCLUSIVE:
# many dispatchers together, no dispatcher while a critique runs, no critique while one
# dispatches.  (farmerbob: lock-ordering and lock-serialisation beads)
LOCKDIR="$HOME/.local/share/farmerbob"; mkdir -p "$LOCKDIR"
exec 8>"$LOCKDIR/speccheck.$BEAD.lock"
flock -s 8

# tree" -- and leaves the directory sitting there, at which point `worktree add` refuses too
# because the path exists and is not empty. 103 worktrees were left in exactly that state
# (farmerbob-13p), so every one of their tasks was permanently un-redispatchable and the
# only symptom was the one-line "worktree failed" below.
#
# Clear the leftover by hand, but only a path this script itself just constructed: it must
# live under WT_ROOT and be named exactly "$BEAD--$SRC". A bare rm -rf on an interpolated
# path is how a harness deletes something it did not mean to.
if [ -e "$WT" ] && ! git -C "$REPO" worktree list --porcelain | grep -qxF "worktree $WT"; then
  case "$WT" in
    "$WT_ROOT/$BEAD--$SRC")
      echo "$SRC: clearing an orphaned worktree directory (no git admin entry)"
      rm -rf -- "$WT" ;;
    *) echo "$SRC: refusing to remove unexpected path $WT"; exit 1 ;;
  esac
fi
git -C "$REPO" worktree prune >/dev/null 2>&1
git -C "$REPO" branch -D "$BRANCH" >/dev/null 2>&1
git -C "$REPO" worktree add -q -b "$BRANCH" "$WT" "$BASE" || { echo "$SRC: worktree failed"; exit 1; }

# --- preconditions -------------------------------------------------------
# A task spec is authored against a base commit. Merging a winner can create the very file
# a queued task was told to create, at which point every arm correctly no-ops and the
# harness scores them all as failures.  (bead farmerbob-m71)
# Task prompts declare their effects as comment lines:
#     <!-- fb:creates crates/x/src/y.rs -->      must NOT exist on the base
#     <!-- fb:modifies crates/x/src/lib.rs -->   MUST exist on the base
fail_precondition() {
  printf '%-24s %-13s %s\n' "$SRC" "TASK-INVALID" "$1"
  REASON="$1" python3 -c "
import json, os
json.dump({'source': os.environ['SRC'], 'bead': os.environ['BEAD'],
           'verdict': 'TASK-INVALID', 'outcome_class': 'task_invalid',
           'reason': os.environ['REASON'], 'rc': -1, 'duration_s': 0,
           'lines_added': 0, 'worktree': os.environ['WT'],
           'branch': os.environ['BRANCH']}, open(os.environ['META'], 'w'), indent=1)"
  exit 3
}
export SRC BEAD WT BRANCH META

# ONE declaration, fence-aware, via the shared reader.
#
# This used to loop over EVERY fb:creates marker in the prompt with a bare grep. A spec that
# documents the marker format carries several as EXAMPLES, so target-decl -- which declared
# target_decl.rs on its first line -- was killed with "declares creates:.../window.rs",
# a path that appears only inside a fenced example block. Its arm was refused before it ran.
#
# That is the wave60 defect in a second reader: an example of a declaration parsed as a
# declaration. fb-speclint did not catch it because speclint permits a FENCED marker, which
# is correct, while this loop did not honour fences at all. Three readers, three rules.
. /home/gabe/Documents/farmerbob/fb-target.sh
DECL=$(fb_declaration_in "$PROMPT_FILE")
case "${DECL%% *}" in
  creates)
    dpath="${DECL#* }"
    [ -e "$WT/$dpath" ] && fail_precondition "declares creates:$dpath but it already exists on the base"
    ;;
  modifies)
    dpath="${DECL#* }"
    [ -e "$WT/$dpath" ] || fail_precondition "declares modifies:$dpath but it does not exist on the base"
    ;;
  *)
    : # no declaration: other stages report that; it is not this stage's refusal
    ;;
esac

MODEL=$(python3 -c "
import tomllib;print(tomllib.load(open('$REPO/sources.toml','rb'))['source']['$SRC'].get('model',''))")
cp "$PROMPT_FILE" "$WT/.fb-task.md"

# Per-run state isolation. A git worktree isolates the REPO only; agent CLIs keep
# their own databases under XDG dirs and N of them deadlock on one SQLite file
# (bead farmerbob-p3r). Give every run private state dirs.
# State lives OUTSIDE the worktree: agents that snapshot their cwd (opencode does, for
# undo) otherwise swallow farmerbob's own bookkeeping -- measured at 6.0GB of state around
# a 56KB deliverable. Sibling path, referenced by env only.  (bead farmerbob-p3r)
FBSTATE="$HOME/.local/share/farmerbob/state/$RUN"
rm -rf "$FBSTATE"
mkdir -p "$FBSTATE/data" "$FBSTATE/state" "$FBSTATE/cache"
# Do not hand the implementer the orchestrator's own issue-tracker instructions.
rm -f "$WT/CLAUDE.md" "$WT/AGENTS.md"
rm -rf "$WT/.beads" "$WT/.cursor" "$WT/.codex" "$WT/.agents"

# The hidden suites must actually be hidden. `.fb/` is tracked, so every worktree shipped the
# conformance tests, the injected-defect patches, and every other task's prompt. An agent that
# reads .fb/conformance/<task>.rs can write code that passes exactly those assertions -- which
# would explain a suite saturating at 15/15 without measuring anything. Observed live: an arm
# read .fb/conformance before writing a line.   (bead: hidden-suite-leak)
rm -rf "$WT/.fb/conformance" "$WT/.fb/defects" "$WT/.fb/prompts"
rm -f "$WT/.fb"/*.tsv
# also remove the harness itself: it describes how scoring works
rm -f "$WT"/fb-*.sh

# EXCEPT a script the task is explicitly about. A port task's deliverable IS one of these
# scripts, so stripping it makes the task impossible as intended: the arm cannot read the
# thing it is porting, the scripts it calls, or the formats it consumes. Eight of twelve port
# runs produced nothing, and the logs show why -- `Glob "**/fb-crossx*" 0 matches`, then an
# arm exploring a repo with no harness in it.
#
# The spec opts in by naming exactly what it needs:
#     <!-- fb:reads fb-crossx.sh fb-verdict.sh -->
# Only the named files come back. Everything else stays stripped, so the guard still holds
# for every ordinary task, and a port cannot quietly grant itself the whole harness.
for extra in $(grep -ohE '<!-- fb:reads [^>]+ -->' "$PROMPT_FILE" 2>/dev/null \
               | sed 's/<!-- fb:reads //; s/ -->//'); do
  # The guard is against ESCAPING the repo, not against subdirectories. It used to be
  #   case "$extra" in */*|.*) refuse
  # which rejected every path containing a slash, so `fb:reads crates/fb/src/objective.rs` --
  # a file plainly inside the repo -- was refused with the message "outside the repo root".
  # That took out all four arms of wave38 before any of them started, and the message named a
  # rule that was not the rule being applied: the check was "no slashes", and it said "outside
  # the repo root". A guard whose error message misdescribes it costs more than the guard saves.
  #
  # What actually has to be forbidden is leaving $REPO: an absolute path, or any `..`
  # component. A relative path made only of ordinary components cannot escape.
  case "$extra" in
    /*)        echo "$SRC: refusing fb:reads absolute path: $extra"; exit 1 ;;
    ..|../*|*/..|*/../*)
               echo "$SRC: refusing fb:reads path that escapes the repo: $extra"; exit 1 ;;
    .*)        echo "$SRC: refusing fb:reads dotfile path: $extra"; exit 1 ;;
  esac
  if [ -f "$REPO/$extra" ]; then
    mkdir -p "$(dirname "$WT/$extra")"
    cp "$REPO/$extra" "$WT/$extra"
    echo "$SRC: restored $extra for a task that is about it"
  else
    echo "$SRC: fb:reads names $extra, which does not exist"; exit 1
  fi
done

START=$(date +%s)
UNIT="fb-${RUN//[^a-zA-Z0-9_-]/_}-$$"
CGSNAP="$LOG_ROOT/$RUN.cgroup"; : > "$CGSNAP"
MEMSNAP="$LOG_ROOT/$RUN.mem"; : > "$MEMSNAP"
# pgrep -f "$WT" matched ANY process with the worktree path in its command line --
# including this script's own `git worktree add ... "$WT"`, which runs in the CALLER's
# cgroup. The max then reported the desktop session's memory as the agent's: 13815 MB
# under a 2G cap.  The cgroup is the authority on which processes belong to this run,
# so ask it directly and never sample a cgroup that is not ours.  (bead farmerbob-05p)
( for _ in $(seq 1 400); do
    for pid in $(pgrep -f "$WT" 2>/dev/null); do
      cg=$(sed -n 's|^0::||p' "/proc/$pid/cgroup" 2>/dev/null)
      [ -n "$cg" ] || continue
      echo "$cg"
      case "$cg" in *"$UNIT"*) ;; *) continue ;; esac   # foreign cgroup: record, never measure
      cat "/sys/fs/cgroup$cg/memory.peak" 2>/dev/null >> "$MEMSNAP"
    done
    sleep 3
  done ) >> "$CGSNAP" 2>/dev/null &
CGPID=$!
(
  cd "$WT" || exit 1
  P="$(cat .fb-task.md)"
  # Confine the agent. --scope would place it in the CALLER's cgroup subtree;
  # we need our own unit, and we must VERIFY it, because systemd-run failing
  # open is indistinguishable from it working (see bead: confinement-unverified).
  run_confined() {
    systemd-run --user --scope --quiet --unit="$UNIT" \
      -E XDG_DATA_HOME="$FBSTATE/data" -E XDG_STATE_HOME="$FBSTATE/state" \
      -E XDG_CACHE_HOME="$FBSTATE/cache" \
      -p MemoryMax="${FB_MEM_MAX:-3G}" -p MemoryHigh="${FB_MEM_HIGH:-2500M}" \
      -p CPUQuota="${FB_CPU_QUOTA:-400%}" -p TasksMax=2048 \
      -p RuntimeMaxSec="${FB_RUN_TIMEOUT:-2700}" -- "$@"
  }
  # RuntimeMaxSec above is the run timeout, and it is systemd's rather than a `timeout`
  # wrapper DELIBERATELY: it terminates the whole scope, so it reaches the agent inside bwrap
  # and every process it spawned. A `timeout` around fb-isolated would signal the wrapper and
  # leave the sandboxed children running.
  #
  # Until 2026-09-17 there was no run timeout of any kind. The one `timeout` in this file was
  # a --print-timeout flag passed to ONE launcher because that launcher happened to offer it.
  # port-status/glm-53-flash then ran 21 minutes on 26 seconds of CPU, writing nothing, and
  # because a live run holds the wave lock it held the entire queue behind it. It ended when a
  # human read `ps`. (farmerbob-... : no dispatch timeout)
  #
  # systemd sends SIGTERM, which surfaces as rc=143, which classify_outcome already maps to
  # orchestrator_cancelled -- so a timed-out run is NOT recorded as the arm producing nothing.
  # That existing mapping is what makes this cheap; without it the cure would have been another
  # way to blame a model for the harness.
  #
  # 45 minutes: the longest legitimate run observed is 1304s (22 min), so the cap is roughly
  # double it. Override per wave with FB_RUN_TIMEOUT.
  # CONTINUE the spec-critique session when this arm has one for this task.
  #
  # fb-speccheck runs at THIS path before dispatch, so the arm has already read the spec and
  # the code once and returned findings on it. All four launchers key continuation to the
  # working directory, and the session outlives the directory -- verified by deleting the
  # directory between two turns and confirming the second still recalled the first, which is
  # exactly what `git worktree add` does above.
  #
  # Best effort, never load-bearing: if no session exists the launcher starts cold and the
  # prompt is self-contained. A stage that only works when the agent remembers has a failure
  # state that looks like a slightly worse answer instead of an obvious one.
  CONT=""
  [ -f "$LOG_ROOT/speccheck/$BEAD/$SRC.findings.md" ] && CONT=yes
  a_c=(); z_c=(); o_c=(); c_c=()
  if [ -n "$CONT" ]; then
    a_c=(--continue); z_c=(--continue); o_c=(--continue); c_c=(--continue)
    echo "$SRC: continuing its spec-critique session"
    # SAY THAT THE STAGE CHANGED. A continued session cannot tell these two turns apart:
    # the critique turn wrapped the spec in _speccheck.md's instructions, and this turn IS
    # the spec, verbatim. The model's last action was "critique this document", the new turn
    # is that same document, and repeating the last action is the reasonable reading.
    #
    # wave86, the first wave in which CONT ever fired, cost a run to exactly that. codex-luna
    # resumed its critique session and replied "Done. Wrote the findings to .fb/speccheck.md",
    # ran the suite, and exited 0: NO-OP, +0 lines, no implementation at all. It is not a weak
    # arm -- it passed wave59 and wave63 cold. gemini-38-flash continued in the same wave and
    # passed with +387 lines, which is how this would have gone on being invisible.
    #
    # The continuation is worth keeping: the arm has already read {spec + code} and reasoned
    # about it once, which is the entire point of running the critique first. It just has to
    # be TOLD which stage it is in, and nothing in the prompt said so.  (bead farmerbob-81tg)
    P="Your spec critique of this task is COMPLETE and has been recorded. This turn is the
IMPLEMENTATION turn: write the code the task below specifies. Do not critique the spec again
and do not write .fb/speccheck.md -- that file belongs to the previous turn and anything you
write to it now is discarded.

Everything you found while critiquing still applies. Where the spec is defective, the task
below says what to do: implement the closest honest thing and say so in your handoff.

$P"
  fi
  case "$SRC" in
    codex-luna)
      if [ -n "$CONT" ]; then
        run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" codex exec resume --last \
            --dangerously-bypass-approvals-and-sandbox --strict-config \
            -c model_reasoning_effort="${FB_CODEX_EFFORT:-xhigh}" \
            -m gpt-5.6-luna "$P"
      else
        run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" codex exec \
            --dangerously-bypass-approvals-and-sandbox --strict-config \
            -c model_reasoning_effort="${FB_CODEX_EFFORT:-xhigh}" \
            -m gpt-5.6-luna "$P"
      fi ;;
    claude-sonnet)   run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" claude \
                         -p "$P" --permission-mode bypassPermissions --model sonnet \
                         --effort "${FB_CLAUDE_EFFORT:-xhigh}" "${c_c[@]}" --output-format json ;;
    gemini-38-flash) run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" agy -p "$P" --print-timeout 45m --model gemini-3.8-flash-high --add-dir "$WT" \
                         "${a_c[@]}" --dangerously-skip-permissions --output-format text ;;
    glm-53-flash)    run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" zcode "${z_c[@]}" --prompt "$P" ;;
    ifm-*)           set -a; . "$HOME/.config/farmerbob/secrets.env"; set +a
                     run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" opencode run "${o_c[@]}" -m "$MODEL" "$P" ;;
    or-*)            run_confined /home/gabe/Documents/farmerbob/fb-isolated "$WT" ori opencode run "${o_c[@]}" -m "$MODEL" "$P" ;;
    # An arm the registry calls verified but this table cannot launch. sources.toml declares
    # `cmd` and `args` for every arm and NOTHING HERE READS THEM -- this case statement is a
    # second, authoritative-looking copy of the same fact, and claude-sonnet was re-enabled,
    # dispatched, and came back rc=127 in 0 seconds because it was missing from this one.
    # Building the command line from the registry is the real fix (farmerbob-...); until then
    # the message at least says which of the two copies is wrong.
    *) echo "unknown source $SRC: no launcher branch in fb-dispatch.sh (sources.toml declares" \
            "cmd/args for it, but this table is what actually runs and does not read them)"
       exit 127 ;;
  esac
) >"$LOG" 2>&1
RC=$?
END=$(date +%s)

# Did confinement actually apply? A scope that silently failed to materialise
# looks exactly like one that worked, so record it as evidence rather than assume.
MEMPEAK=$(sort -n "$MEMSNAP" 2>/dev/null | tail -1)
MEMPEAK=$(( ${MEMPEAK:-0} / 1048576 ))
CONFINED="unknown"
if [ -f "$LOG_ROOT/$RUN.cgroup" ]; then
  grep -q "$UNIT" "$LOG_ROOT/$RUN.cgroup" && CONFINED="yes" || CONFINED="NO"
fi

kill $CGPID 2>/dev/null; rm -f "$WT/.fb-task.md"
cd "$WT" || exit 1
BUILD="skip"; TEST="skip"
# Verification is a SCARCE RESOURCE, exactly like the GPU. rustc is memory-hungry
# (rusqlite/bundled compiles SQLite from source) and N concurrent verifications will OOM the
# box even when every agent is properly confined. Serialise through a lease and confine it --
# the lease manager was always meant to be generic over named resources. (beads farmerbob-89j,
# farmerbob-qe3)
VERIFY_LOCK="$HOME/.local/share/farmerbob/verify.lock"
verify() {  # verify <cargo-subcommand>
  flock "$VERIFY_LOCK" systemd-run --user --scope --quiet --unit="fb-verify-$$-$1" \
    -p MemoryMax=4G -p CPUQuota=800% \
    -- cargo "$1" -p "$CRATE" >>"$LOG" 2>&1
}
if verify build; then BUILD="pass"; else BUILD="FAIL"; fi
if [ "$BUILD" = "pass" ] && verify test; then TEST="pass"; else
  [ "$BUILD" = "pass" ] && TEST="FAIL"; fi
# Count untracked NEW files too. Round-1 tasks said "replace lib.rs" (a tracked edit) so a
# diff-only count worked; round-3 tasks say "create proto.rs" (a new file) and the same
# metric silently scored real 242- and 623-line implementations as "+1 lines FAIL".
# The metric did not change -- the task shape did.  (bead farmerbob-2em, outcome_class)
LOC=$(git -C "$WT" diff --numstat HEAD -- crates/ | awk '{a+=$1} END{print a+0}')
for f in $(git -C "$WT" ls-files --others --exclude-standard crates/ 2>/dev/null); do
  LOC=$(( LOC + $(wc -l < "$WT/$f" 2>/dev/null || echo 0) ))
done
FILES=$(git -C "$WT" status --porcelain | wc -l)
# an empty crate builds and "tests" clean -- count tests that actually RAN
# THE LAST INVOCATION, not every invocation in the log.
#
# This was `grep -oE '^test result: ok\. [0-9]+ passed' | awk '{s+=$4}'` -- a sum over the
# WHOLE log. A `cargo test` run emits one result line per target (lib, doctests, compile-fail),
# so an arm that runs the suite twice is credited twice, and the number stops being "how many
# tests ran" and becomes "how many times the arm ran them, times the size of the workspace".
#
# 124 of the 421 run logs on this machine have more than one invocation, so roughly 29% of
# every `tests` figure this project has recorded is inflated by an integer multiple. It is the
# `tests=` column in `fb brief` and it feeds TestDepth in farmerbob_core::adjudicate, which
# means the metric was REWARDING having needed a second attempt.
#
# wave87/field-shape is the demonstration. codex-luna scored tests_run=4566 against
# gemini-38-flash's 1534 on the same module and looked like the best-tested candidate in the
# field. It had run the suite three times; its module contains THREE `#[test]` functions, the
# fewest of the four. or-nemotron-ultra, which actually wrote 42, scored 4779 for the same
# wrong reason.
#
# The delimiter is the widest block in the log: a full `cargo test` opens with
# `running <N> tests` for the lib target, and N is the largest such number in the run. Reset
# the sum at every occurrence of that block and the surviving total is the last complete
# invocation. Filtered re-runs (`cargo test <filter>`) open with a smaller N and do not reset
# it, which is right -- they are not a scoring run.
#
# `null` when no invocation is found, never 0: "the suite never ran" and "the suite ran and
# nothing passed" are different facts, and `tests` is already an Option downstream.
#   (bead farmerbob-xbu1)
NTESTS=$(awk '
  NR==FNR {
    if ($0 ~ /^running [0-9]+ tests?$/ && $2+0 > max) max = $2+0
    next
  }
  {
    if (max > 0 && $0 ~ /^running [0-9]+ tests?$/ && $2+0 == max) { s = 0; seen = 1 }
    if (seen && $0 ~ /^test result: ok\. [0-9]+ passed/) s += $4
  }
  END { if (seen) print s+0; else print "null" }
' "$LOG" "$LOG")
# verdict is the only field that means anything: real work, compiles, tests exist and pass
VERDICT="FAIL"
# No absolute line threshold: it grades on VOLUME and fails correct implementations of
# small tasks. Task size is a property of the task, not the arm.  (see fb-score.sh)
if [ "$BUILD" = "pass" ] && [ "$TEST" = "pass" ] && [ "$LOC" -gt 0 ] && [ "$NTESTS" -gt 0 ]; then
  VERDICT="PASS"
elif [ "$LOC" -eq 0 ]; then VERDICT="NO-OP"
elif [ "$BUILD" != "pass" ]; then VERDICT="NO-COMPILE"
elif [ "$NTESTS" -eq 0 ]; then VERDICT="NO-TESTS"
fi

python3 -c "
import json,sys
json.dump({'source':'$SRC','bead':'$BEAD','model':'$MODEL','rc':$RC,'duration_s':$((END-START)),
 'build':'$BUILD','test':'$TEST','verdict':'$VERDICT','confined':'$CONFINED','mem_peak_mb':$MEMPEAK,'tests_run':$NTESTS,'lines_added':$LOC,'files_touched':$FILES,
 'worktree':'$WT','branch':'$BRANCH','log':'$LOG'}, open('$META','w'), indent=1)"
printf '%-24s %-11s rc=%-3s %4ss  build=%-5s tests=%-3s +%s lines\n' "$SRC" "$VERDICT" "$RC" "$((END-START))" "$BUILD" "$NTESTS" "$LOC"
