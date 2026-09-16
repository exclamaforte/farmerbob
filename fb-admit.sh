#!/usr/bin/env bash
# fb-admit — run a dispatch matrix with admission control.
# MemoryMax is a per-run LIMIT, not a reservation; systemd will happily accept scopes whose
# limits exceed the machine. The sum is what must be bounded.  (bead farmerbob-89j)
#   fb-admit.sh <matrix.tsv>      lines: task<TAB>crate<TAB>arm1,arm2,...
set -uo pipefail
M="${1:?matrix.tsv}"
PROVIDER_CAP=${FB_PROVIDER_CAP:-3}
SLOT_GB=${FB_SLOT_GB:-1.5}        # admission arithmetic: measured p95 was 1.26G
HARD_GB=${FB_MEM_GB:-2}           # per-run MemoryMax: contains an overrun inside its own scope
HEADROOM_GB=${FB_HEADROOM_GB:-3}   # orchestrator + one confined verification
avail() { free -g | awk '/^Mem:/{print $7}'; }
SLOTS=$(awk -v a="$(avail)" -v h="$HEADROOM_GB" -v g="$SLOT_GB" 'BEGIN{printf "%d", (a-h)/g}')
[ "$SLOTS" -lt 1 ] && SLOTS=1
echo "admission: ${SLOT_GB}G/slot (measured), ${HARD_GB}G hard cap, ${HEADROOM_GB}G headroom, $(avail)G avail -> $SLOTS slots, max $PROVIDER_CAP per upstream vendor"

# Agents are network-bound (measured: ~5% cpu, do_epoll_wait), so the machine is not the
# binding constraint -- the PROVIDER is. Poolside rate-limited a run when we burst free-tier
# calls. Cap concurrency per provider bucket as well as per machine.  (bead farmerbob-4ix)
provider_of() {
  python3 -c "
import tomllib,sys
d=tomllib.load(open('sources.toml','rb'))['source'].get(sys.argv[1],{})
print(d.get('quota_bucket') or d.get('provider') or 'unknown')" "$1"
}
in_flight_for() { ls "$LOCKDIR"/"$1".* 2>/dev/null | wc -l; }
LOCKDIR=$(mktemp -d); trap 'rm -rf "$LOCKDIR"' EXIT

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
  PROV=$(provider_of "$arm")
  while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ] \
     || [ "$(avail)" -lt "$HEADROOM_GB" ] \
     || [ "$(in_flight_for "$PROV")" -ge "$PROVIDER_CAP" ]; do
    sleep 5
  done
  touch "$LOCKDIR/$PROV.$$.$RANDOM"
  echo "dispatch $task/$arm  (avail $(avail)G, running $(jobs -rp|wc -l)/$SLOTS)"
  LK=$(ls "$LOCKDIR/$PROV".* 2>/dev/null | tail -1)
  ( FB_MEM_MAX="${HARD_GB}G" ./fb-dispatch.sh "$arm" "$task" ".fb/prompts/$task.md" "$crate"; rm -f "$LK" ) &
  sleep 2
done
wait
echo "all runs complete"
