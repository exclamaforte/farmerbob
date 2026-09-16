#!/usr/bin/env bash
# fb-admit — run a dispatch matrix with admission control.
# MemoryMax is a per-run LIMIT, not a reservation; systemd will happily accept scopes whose
# limits exceed the machine. The sum is what must be bounded.  (bead farmerbob-89j)
#   fb-admit.sh <matrix.tsv>      lines: task<TAB>crate<TAB>arm1,arm2,...
set -uo pipefail
M="${1:?matrix.tsv}"
PER_GB=${FB_MEM_GB:-3}
HEADROOM_GB=${FB_HEADROOM_GB:-6}
avail() { free -g | awk '/^Mem:/{print $7}'; }
SLOTS=$(( ( $(avail) - HEADROOM_GB ) / PER_GB ))
[ "$SLOTS" -lt 1 ] && SLOTS=1
echo "admission: ${PER_GB}G/run, ${HEADROOM_GB}G headroom, $(avail)G available -> $SLOTS slots"

QUEUE=()
while IFS=$'\t' read -r task crate arms; do
  [ -z "${task:-}" ] && continue
  IFS=',' read -ra A <<< "$arms"
  for a in "${A[@]}"; do QUEUE+=("$a|$task|$crate"); done
done < "$M"
echo "queued ${#QUEUE[@]} runs"

running=0
for item in "${QUEUE[@]}"; do
  IFS='|' read -r arm task crate <<< "$item"
  # block until a slot frees AND memory actually allows it
  while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ] || [ "$(avail)" -lt $(( PER_GB + HEADROOM_GB )) ]; do
    sleep 10
  done
  echo "dispatch $task/$arm  (avail $(avail)G, running $(jobs -rp|wc -l)/$SLOTS)"
  FB_MEM_MAX="${PER_GB}G" ./fb-dispatch.sh "$arm" "$task" ".fb/prompts/$task.md" "$crate" &
  sleep 5
done
wait
echo "all runs complete"
