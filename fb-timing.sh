#!/usr/bin/env bash
# fb-timing — reconstruct per-run timing from the WORKTREE, not the run record.
# Survives a dead supervisor, and gives richer signal than total wallclock:
#   t_start       worktree created (dispatch)
#   t_first       earliest mtime of a file the agent wrote  -> responsiveness
#   t_last        latest mtime                              -> when it stopped working
#   ttfw          t_first - t_start   (time to first artefact)
#   work_span     t_last  - t_first   (how long it was actually producing)
#   total         t_last  - t_start
#   lines/min     throughput over work_span
set -uo pipefail
BEAD="${1:?bead}"
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
printf '%-22s %7s %7s %7s %7s %9s\n' SOURCE TTFW SPAN TOTAL LINES LINES/MIN
for WT in "$WT_ROOT/$BEAD--"*; do
  [ -d "$WT" ] || continue
  SRC="${WT##*/$BEAD--}"
  # dispatch time = when the worktree's .git link was created
  T0=$(stat -c %Y "$WT/.git" 2>/dev/null || echo 0)
  FILES=$( { git -C "$WT" diff --name-only HEAD -- crates/ 2>/dev/null
             git -C "$WT" ls-files --others --exclude-standard crates/ 2>/dev/null; } | sort -u )
  [ -z "$FILES" ] && { printf '%-22s %7s %7s %7s %7s %9s\n' "$SRC" - - - 0 -; continue; }
  TF=99999999999; TL=0
  for f in $FILES; do
    m=$(stat -c %Y "$WT/$f" 2>/dev/null) || continue
    [ "$m" -lt "$TF" ] && TF=$m
    [ "$m" -gt "$TL" ] && TL=$m
  done
  LOC=$(git -C "$WT" diff --numstat HEAD -- crates/ 2>/dev/null|awk '{a+=$1}END{print a+0}')
  for f in $(git -C "$WT" ls-files --others --exclude-standard crates/ 2>/dev/null); do
    LOC=$(( LOC + $(wc -l < "$WT/$f" 2>/dev/null||echo 0) )); done
  TTFW=$(( TF - T0 )); SPAN=$(( TL - TF )); TOTAL=$(( TL - T0 ))
  RATE=$(awk -v l="$LOC" -v s="$SPAN" 'BEGIN{if(s>0)printf "%.0f",l*60/s; else printf "-"}')
  printf '%-22s %6ss %6ss %6ss %7s %9s\n' "$SRC" "$TTFW" "$SPAN" "$TOTAL" "$LOC" "$RATE"
done
