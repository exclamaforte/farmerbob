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
avail_mb() { free -m | awk '/^Mem:/{print $7}'; }

# The slot count comes from `fb slots`, which charges each admission the HARD cap against
# farmerbob_core::slots::SlotTable, so committed+requested <= available-headroom bounds the
# SUM. (farmerbob-89j)
#
# This script used to compute it here, in two lines of awk, against SLOT_GB -- the MEASURED
# p95 per-run size -- while handing every run HARD_GB as its MemoryMax. On this machine that
# was 7 slots of 2G permits against an 11G ceiling: 3.3GB over-committed, bounded by nothing.
#
# The clamp that used to follow is deliberately gone:
#     [ "$SLOTS" -lt 1 ] && SLOTS=1
# It admitted one run onto a machine measured as unable to hold it. Zero is an answer, and the
# dispatcher waits for the next tick rather than being told a comfortable number.
#
# PROVIDER_CAP stays here. It is a different constraint -- the provider, not the machine -- and
# `fb slots` deliberately does not model it.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
HARD_MB=$(awk -v g="$HARD_GB" 'BEGIN{printf "%d", g*1024}')
HEADROOM_MB=$(awk -v g="$HEADROOM_GB" 'BEGIN{printf "%d", g*1024}')
if [ -x "$FB_BIN" ] && SLOTS=$("$FB_BIN" slots --available-mb "$(avail_mb)" \
        --headroom-mb "$HEADROOM_MB" --memory-mb "$HARD_MB" --plan 2>/dev/null \
        | grep -oE '[0-9]+ slots?' | grep -oE '^[0-9]+'); then
  echo "admission: ${HARD_GB}G/slot (hard cap), ${HEADROOM_GB}G headroom, $(avail)G avail -> $SLOTS slots, max $PROVIDER_CAP per upstream vendor"
else
  # The binary is not built, or could not answer. Refuse rather than fall back to the old
  # arithmetic: a second copy of this decision is how it stayed wrong for so long.
  echo "admission: cannot size the machine -- $FB_BIN unavailable or gave no count; refusing to guess" >&2
  exit 1
fi
if [ "$SLOTS" -lt 1 ]; then
  echo "admission: 0 slots at ${HARD_GB}G/run with ${HEADROOM_GB}G headroom on $(avail)G -- nothing dispatched" >&2
  exit 0
fi

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

# The eligibility guard existed and was called by nothing, so every protection it
# encodes -- disabled arms, paid duplicates of free routes, unregistered names -- was
# advisory. An unregistered arm reached provider_of and crashed it with a KeyError
# after burning a dispatch.  (bead farmerbob-vgn)
. /home/gabe/Documents/farmerbob/fb-eligible.sh

QUEUE=()
while IFS=$'\t' read -r task crate arms; do
  [ -z "${task:-}" ] && continue
  IFS=',' read -ra A <<< "$arms"
  for a in "${A[@]}"; do
    if ! fb_eligible "$a"; then
      # A name absent from the registry is a typo, and a typo means this matrix does not
      # run what its author believed. That is worth stopping the wave for; a deliberately
      # disabled arm is not.
      grep -q "^\[source\.$a\]" /home/gabe/Documents/farmerbob/sources.toml \
        || { echo "abort: $a is not a registered arm"; exit 2; }
      continue
    fi
    QUEUE+=("$a|$task|$crate")
  done
done < "$M"
[ "${#QUEUE[@]}" -eq 0 ] && { echo "no eligible runs"; exit 1; }
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
