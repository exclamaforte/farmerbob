#!/usr/bin/env bash
# fb-score — score every candidate for a bead by enumerating WORKTREES, not run
# records. Survives a dispatch wrapper dying, which run-record-driven scoring does not.
#   fb-score.sh <bead> [crate]
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
. /home/gabe/Documents/farmerbob/fb-verdict.sh
BEAD="${1:?bead}"; CRATE="${2:-farmerbob-core}"
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
LOG_ROOT="$HOME/.local/share/farmerbob/logs"
OUT="$LOG_ROOT/$BEAD.score.json"

echo "[" > "$OUT"; first=1
# Baseline the crate on the repo HEAD, once, so each arm is measured on what it ADDED
# rather than on what it inherited.  (bead farmerbob-0fx)
BASE_CLIPPY=$( (cd /home/gabe/Documents/farmerbob && cargo clippy -p "$CRATE" --all-targets 2>&1) | grep -cE '^warning|^error' )
echo "clippy baseline: $CRATE on HEAD = $BASE_CLIPPY"

for WT in "$WT_ROOT/$BEAD--"*; do
  [ -d "$WT" ] || continue
  SRC="${WT##*/$BEAD--}"
  # NEVER score a live worktree: cargo build/test/clippy take the same target/ lock the
  # agent is using, so harness and implementer block each other, the agent's latency is
  # inflated, and mtime-based timing reconstruction measures the observer (bead farmerbob-qe3).
  #
  # Liveness needs TWO signals. The task file alone is a stale marker: an orphaned run whose
  # supervisor died leaves it behind forever and the run looks live for good. Require the
  # marker AND an actual process whose cwd is this worktree.
  LIVE=no
  if [ -f "$WT/.fb-task.md" ]; then
    for pid in $(pgrep -x opencode; pgrep -x agy; pgrep -x zcode; pgrep -x codex) ; do
      [ "$(readlink -f /proc/$pid/cwd 2>/dev/null)" = "$(readlink -f "$WT")" ] && LIVE=yes && break
    done
    [ "$LIVE" = no ] && rm -f "$WT/.fb-task.md"   # orphan: clear the stale marker
  fi
  if [ "$LIVE" = yes ]; then
    printf '%-22s %-11s (SKIPPED: run still live)\n' "$SRC" "LIVE"; continue
  fi
  LOC=$(git -C "$WT" diff --numstat HEAD -- crates/ 2>/dev/null | awk '{a+=$1} END{print a+0}')
  UNT=$(git -C "$WT" ls-files --others --exclude-standard crates/ 2>/dev/null | wc -l)
  # count untracked new source files toward the work total
  for f in $(git -C "$WT" ls-files --others --exclude-standard crates/ 2>/dev/null); do
    LOC=$(( LOC + $(wc -l < "$WT/$f" 2>/dev/null || echo 0) )); done
  # scope discipline: did it touch crates it was told to leave alone?
  CRATES=$( { git -C "$WT" diff --name-only HEAD -- crates/ 2>/dev/null
              git -C "$WT" ls-files --others --exclude-standard crates/ 2>/dev/null; } \
            | cut -d/ -f2 | sort -u | tr '\n' ',' )
  NCRATES=$(echo "$CRATES" | tr ',' '\n' | grep -c .)

  B="FAIL"; T="FAIL"; N=0; CLIPPY="-"
  TL=$(mktemp)
  (cd "$WT" && cargo build -p "$CRATE" >"$TL" 2>&1) && B="pass"
  if [ "$B" = "pass" ]; then
    (cd "$WT" && cargo test -p "$CRATE" >"$TL" 2>&1) && T="pass"
    N=$(grep -oE '^test result: ok\. [0-9]+ passed' "$TL" | awk '{s+=$4} END{print s+0}')
    # DELTA, not absolute. Counting the crate's total warnings charges every candidate for
    # lint debt it inherited: on cli-tree, master's fb crate already had 6 and both arms had
    # exactly 6, yet both were scored clippy=5. It failed quietly -- all candidates got the
    # same wrong number, so criterion 3 read as a tie rather than as a broken metric.
    # A negative delta is kept: cleaning up inherited warnings deserves the credit.
    # (bead farmerbob-0fx)
    CAND=$( (cd "$WT" && cargo clippy -p "$CRATE" --all-targets 2>&1) | grep -cE '^warning|^error' )
    CLIPPY=$(( CAND - BASE_CLIPPY ))
  else
    ERR=$(grep -m1 -oE '^error(\[E[0-9]+\])?: .*' "$TL" | cut -c1-70)
  fi
  rm -f "$TL"

  # No absolute line threshold. A fixed `lines > 30` graded on VOLUME and failed nine
  # correct 16-27 line implementations of a deliberately small task, while passing the one
  # arm that happened to write 31. Task size is a property of the task, not the arm.
  # PASS = it compiled, tests executed and passed, and it actually changed something.
  V=$(fb_verdict "$B" "$T" "$N" "$LOC")

  DUR=$(python3 -c "
import json,os
p='$LOG_ROOT/$BEAD--$SRC.json'
print(json.load(open(p))['duration_s'] if os.path.exists(p) else -1)" 2>/dev/null || echo -1)

  [ $first -eq 0 ] && echo "," >> "$OUT"; first=0
  python3 -c "
import json,sys
json.dump({'source':'$SRC','verdict':'$V','build':'$B','test':'$T','tests_run':$N,
'lines':$LOC,'new_files':$UNT,'crates_touched':$NCRATES,'crates':'$CRATES',
'clippy':'$CLIPPY','duration_s':$DUR,'err':'''${ERR:-}'''},sys.stdout,indent=1)" >> "$OUT"
  unset ERR
  printf '%-22s %-11s tests=%-3s clippy=%-4s %5sL crates=%-2s %4ss\n' \
    "$SRC" "$V" "$N" "$CLIPPY" "$LOC" "$NCRATES" "$DUR"
done
echo "]" >> "$OUT"; echo "-> $OUT"
