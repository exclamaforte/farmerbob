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
git -C "$REPO" branch -D "$BRANCH" >/dev/null 2>&1
git -C "$REPO" worktree add -q -b "$BRANCH" "$WT" "$BASE" || { echo "$SRC: worktree failed"; exit 1; }

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

START=$(date +%s)
CGSNAP="$LOG_ROOT/$RUN.cgroup"; : > "$CGSNAP"
( for _ in $(seq 1 400); do
    for pid in $(pgrep -f "$WT" 2>/dev/null); do
      sed -n 's|^0::||p' "/proc/$pid/cgroup" 2>/dev/null; done
    sleep 3
  done ) >> "$CGSNAP" 2>/dev/null &
CGPID=$!
UNIT="fb-${RUN//[^a-zA-Z0-9_-]/_}-$$"
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
      -p CPUQuota="${FB_CPU_QUOTA:-400%}" -p TasksMax=2048 -- "$@"
  }
  case "$SRC" in
    codex-luna)      run_confined codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.6-luna "$P" ;;
    gemini-38-flash) run_confined agy -p "$P" --model gemini-3.8-flash-high --add-dir "$WT" \
                         --dangerously-skip-permissions --output-format text ;;
    glm-53-flash)    run_confined zcode --prompt "$P" ;;
    ifm-*)           set -a; . "$HOME/.config/farmerbob/secrets.env"; set +a
                     run_confined opencode run -m "$MODEL" "$P" ;;
    or-*)            run_confined ori opencode run -m "$MODEL" "$P" ;;
    *) echo "unknown source $SRC"; exit 127 ;;
  esac
) >"$LOG" 2>&1
RC=$?
END=$(date +%s)

# Did confinement actually apply? A scope that silently failed to materialise
# looks exactly like one that worked, so record it as evidence rather than assume.
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
NTESTS=$(grep -oE '^test result: ok\. [0-9]+ passed' "$LOG" | awk '{s+=$4} END{print s+0}')
# verdict is the only field that means anything: real work, compiles, tests exist and pass
VERDICT="FAIL"
if [ "$BUILD" = "pass" ] && [ "$TEST" = "pass" ] && [ "$LOC" -gt 30 ] && [ "$NTESTS" -gt 0 ]; then
  VERDICT="PASS"
elif [ "$LOC" -eq 0 ]; then VERDICT="NO-OP"
elif [ "$BUILD" != "pass" ]; then VERDICT="NO-COMPILE"
elif [ "$NTESTS" -eq 0 ]; then VERDICT="NO-TESTS"
fi

python3 -c "
import json,sys
json.dump({'source':'$SRC','bead':'$BEAD','model':'$MODEL','rc':$RC,'duration_s':$((END-START)),
 'build':'$BUILD','test':'$TEST','verdict':'$VERDICT','confined':'$CONFINED','tests_run':$NTESTS,'lines_added':$LOC,'files_touched':$FILES,
 'worktree':'$WT','branch':'$BRANCH','log':'$LOG'}, open('$META','w'), indent=1)"
printf '%-24s %-11s rc=%-3s %4ss  build=%-5s tests=%-3s +%s lines\n' "$SRC" "$VERDICT" "$RC" "$((END-START))" "$BUILD" "$NTESTS" "$LOC"
