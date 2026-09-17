#!/usr/bin/env bash
# fb-wave — launch one dispatch wave detached, and refuse to launch a second on top of it.
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
LOCK="$HOME/.local/share/farmerbob/wave.lock"

exec 9>"$LOCK"
if ! flock -n 9; then
  echo "REFUSING: another wave holds $LOCK" >&2
  exit 1
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
