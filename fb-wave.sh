#!/usr/bin/env bash
# fb-wave — launch one dispatch wave detached, and refuse to launch the SAME one twice.
#
# Two concurrent fb-admit runs on the same matrix each call
#   git worktree remove --force && git worktree add
# for the SAME run, so the second silently deletes the first's worktree mid-flight. Eight
# of ten runs were destroyed that way, and the surviving evidence looked like arms producing
# nothing. The duplicate arose because `cmd & disown; cat "$LOG"` in a single shell line
# binds `&` to the WHOLE `&&` chain, so the variable holding the log path was set only
# inside the background job -- the launch succeeded and only the readback failed, which
# reads exactly like a failure worth retrying.
#
#   fb-wave.sh <matrix.tsv> [logfile]
set -uo pipefail
cd /home/gabe/Documents/farmerbob
M="${1:?matrix.tsv}"
LOG="${2:-$HOME/.local/share/farmerbob/logs/wave-$(basename "$M" .tsv).log}"
# PER-MATRIX, not global. The hazard this guards is documented above and is specific: two
# fb-admit runs on the SAME matrix each call `git worktree remove --force && git worktree add`
# for the SAME run, and the second deletes the first's worktree mid-flight. A worktree is named
# <task>--<arm>, so two waves on DIFFERENT matrices cannot name the same directory and cannot
# collide.
#
# The lock was global because that was the safe thing to write while the cause was still being
# found. It then became the binding constraint on throughput: a wave is three or four arms
# against a six-slot machine, so half the box sat idle by construction, and the autopilot's
# slot-based launching -- added specifically to run two waves at once -- could never fire. It
# reported "0/2 waves" on every launch it ever made.
#
# Same file, same flock, one name per matrix.
LOCKDIR="$HOME/.local/share/farmerbob"
LOCK="$LOCKDIR/wave.$(basename "$M" .tsv).lock"
mkdir -p "$LOCKDIR"

exec 9>"$LOCK"
if ! flock -n 9; then
  echo "REFUSING: another wave holds $LOCK" >&2
  exit 1
fi

# CLAIM THE MATRIX HERE, not in the caller.
#
# The lock above stops two waves running AT ONCE. It cannot stop a wave being launched again
# AFTER the first finishes, because the only record that a matrix was ever dispatched is
# which directory its .tsv sits in -- and this script never moved it. Only fb-autopilot did,
# as a caller-side step. So a hand-launched wave left its .tsv in the queue permanently, and
# the autopilot tripped it the moment the machine went idle.
#
# That is exactly what happened to wave24: launched by hand at 20:34, three arms finished and
# the fourth ran for two hours, and at 22:51 -- seconds after the last agent exited -- the
# autopilot found the .tsv still queued and relaunched the whole wave. fb-dispatch removes and
# recreates each worktree, so all four completed runs were destroyed unscored. Roughly three
# hours of agent time, no data.
#
# The `flock -n` above was added after the SAME root cause on wave20 and fixed only the
# concurrent half of it. Claiming here makes a hand launch and an autopilot launch identical,
# which is the actual invariant: whoever launches a wave claims it, because this is the only
# code that knows a wave was launched.
#   (bead farmerbob-bal)
QUEUE="/home/gabe/Documents/farmerbob/.fb/queue"
if [ "$(dirname "$(readlink -f "$M")")" = "$(readlink -f "$QUEUE")" ]; then
  mkdir -p "$QUEUE/dispatched"
  if mv "$M" "$QUEUE/dispatched/$(basename "$M")"; then
    M="$QUEUE/dispatched/$(basename "$M")"
    echo "claimed $(basename "$M") -> dispatched/"
  else
    echo "REFUSING: cannot claim $M" >&2
    exit 1
  fi
fi

# The lock is held by THIS shell; hand it to the background job by keeping fd 9 open there.
setsid bash -c '
  exec 9>"'"$LOCK"'"
  # -n: REFUSE rather than queue. A blocking flock made a duplicate launch invisible --
  # the second dispatcher simply waited, then ran, removing and recreating worktrees the
  # first had already handed to agents. Observed on wave20 when a manual launch raced the
  # autopilot over a matrix still sitting in the queue. Failing loudly is the point.
  flock -n 9 || { echo "REFUSED: another wave holds the lock" >&2; exit 1; }
  exec bash ./fb-admit.sh "'"$M"'"
' >"$LOG" 2>&1 &
PID=$!
disown
echo "wave $(basename "$M") -> pid $PID"
echo "log  $LOG"
