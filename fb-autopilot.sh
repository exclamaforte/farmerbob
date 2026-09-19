#!/usr/bin/env bash
# fb-autopilot — keep agents working when the orchestrator is not around.
#
# The cron tick that adjudicates and dispatches lives inside a Claude session: it only fires
# while that REPL is idle, and it dies with the session. Twice now the machine has sat idle
# for hours with finished results nobody picked up. This is the part that does not depend on
# anyone waking up.
#
# It dispatches ONLY waves explicitly queued in .fb/queue/*.tsv, oldest first. It never
# adjudicates, never merges, never commits -- those need judgement. It just refuses to let the
# box be idle while there is queued work.
#
# CONCURRENCY. It used to launch only when NOTHING was running, which wasted most of the
# machine: a wave is four arms against a 6-7 slot admission, so five of every ten minutes the
# box ran one task at half capacity while the queue backed up. It now launches whenever there
# are free slots for the whole next wave, up to FB_MAX_WAVES concurrent dispatchers.
#
# Two guards, because parallelism here has already broken things once each:
#
#   SLOTS. It asks `fb slots` the same question fb-admit.sh asks, and refuses unless the
#   ENTIRE next wave fits alongside what is already live. Launching a partial wave would let
#   fb-admit.sh queue arms behind a full machine and the wave would straggle for hours.
#
#   TARGET COLLISION. It refuses a wave whose declared deliverable is the file a live wave is
#   already writing. park-decision and park-scope both targeted quota.rs; the second branched
#   from a base without the first and the merge had to be done by hand as a graft. Serial
#   dispatch hid that. Parallel dispatch makes it routine, so it is checked here.
#
# The pipeline branch below still runs ONLY when the box is completely idle: it blocks this
# loop for as long as a critique takes, and blocking the dispatcher is the thing this daemon
# exists not to do.
set -uo pipefail
. /home/gabe/Documents/farmerbob/fb-target.sh
cd /home/gabe/Documents/farmerbob
Q=.fb/queue; DONE=$Q/dispatched
LOG="$HOME/.local/share/farmerbob/logs/autopilot.log"
mkdir -p "$Q" "$DONE"

say() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >> "$LOG"; }

FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
MAX_WAVES=${FB_MAX_WAVES:-4}
HEADROOM_MB=${FB_HEADROOM_MB:-3072}
HARD_MB=${FB_HARD_MB:-1536}

# THE ENVIRONMENT THE LAUNCH ACTUALLY NEEDS.
#
# This loop launched 65 waves and then stopped, at wave85, and the reason was not the loop: it
# was that every launch after that was made BY HAND with two variables this script did not set.
#
#   FB_MEM_GB=1.5          admission was charging 2G a slot against a measured p95 of 1.26G,
#                          so a 14G machine granted 5 slots where it holds 8.
#   FB_SKIP_SPECCHECK      was forced to 1 here, because fb-admit ran the spec critique for
#                          every matrix line SERIALLY: six to thirteen minutes each, over an
#                          hour of ramp for an eight-task wave. That bought ramp time and
#                          silently cost BOTH the spec critique and session reuse, since
#                          fb-dispatch sets CONT=yes only when a findings file exists.
#                          fb-admit now runs them concurrently and waits, so the stage is
#                          affordable and the default is back to 0. Override to 1 for a wave
#                          where the specs have already been critiqued.
#
# Hand-launching with the right flags while the daemon sat idle with the wrong ones is not a
# daemon that does not work. It is a daemon that was never told what the job had become.
#
# Both are exported so `fb-wave.sh` -> `fb-admit.sh` inherit them, and both stay overridable.
export FB_MEM_GB="${FB_MEM_GB:-1.5}"
export FB_SKIP_SPECCHECK="${FB_SKIP_SPECCHECK:-0}"

# How many arms the machine can hold right now. Same question fb-admit.sh asks, same binary,
# so the two cannot disagree about the size of the box. Echoes nothing when it cannot be
# sized, and the caller then refuses to launch rather than guessing -- an over-committed
# machine is how a 2G-per-slot permit ended up 3.3GB past its ceiling.
free_slots() {
  local avail total
  avail=$(free -m | awk '/^Mem:/{print $7}')
  total=$("$FB_BIN" slots --available-mb "$avail" --headroom-mb "$HEADROOM_MB" \
            --memory-mb "$HARD_MB" --plan 2>/dev/null \
          | grep -oE '[0-9]+ slots?' | grep -oE '^[0-9]+') || return 1
  [ -n "$total" ] || return 1
  echo "$total"
}

# Tasks with a live agent, from the scope unit names (fb-<task>--<arm>.scope).
live_tasks() {
  systemctl --user list-units --type=scope --state=running --no-legend 2>/dev/null \
    | grep -oE 'fb-[a-z0-9-]+--[a-z0-9.-]+\.scope' \
    | sed -e 's/^fb-//' -e 's/--[a-z0-9.-]*\.scope$//' | sort -u
}

# True when the wave in $1 declares a deliverable a live task is already writing.
# Checks EVERY row, not just the first. Same defect as wave_arms had: a seven-row manifest
# under one-arm-per-task had six of its targets unchecked, so the guard that exists because
# park-decision and park-scope both wrote quota.rs was covering one seventh of the wave.
collides() {
  local tsv="$1" task want t other live
  live=$(live_tasks)
  [ -n "$live" ] || return 1
  while IFS=$'\t' read -r task _ _; do
    [ -n "${task:-}" ] || continue
    want=$(fb_target "$task" 2>/dev/null) || continue
    [ -n "$want" ] || continue
    while read -r t; do
      [ -n "$t" ] || continue
      other=$(fb_target "$t" 2>/dev/null) || continue
      if [ "$other" = "$want" ]; then
        say "HOLDING $(basename "$tsv"): $task targets $want, which live task $t is writing"
        return 0
      fi
    done <<< "$live"
  done < "$tsv"
  return 1
}

# Arms in a queued wave: the third column, comma separated, summed over EVERY row.
#
# This read NR==1 only. That was right when a wave was one task with several arms; under
# one-arm-per-task it reported 1 for a seven-row manifest, and the admission gate below --
# the only consumer -- would then launch all seven whenever ONE slot was free. It fit by
# luck on wave104 because the machine was idle. The gate cannot fail loudly; it just
# permits.  (bead farmerbob-jfm1)
wave_arms() { awk -F'\t' 'NF && $1 != "" { n += split($3, a, ",") } END { print n+0 }' "$1"; }
BEAT="$HOME/.local/share/farmerbob/logs/autopilot.heartbeat"
beat() { printf '%s pid=%s %s\n' "$(date -Is)" "$$" "$*" > "$BEAT"; }
say "autopilot up (pid $$)"; beat "starting"

while :; do
  # --state=running: a failed scope is not a live agent, and subtracting one from the slot
  # count forever is how a machine loses capacity one timeout at a time. See fb-status.sh.
  live=$(systemctl --user list-units --type=scope --state=running --no-legend 2>/dev/null \
         | grep -c 'fb-[a-z0-9-]*--.*\.scope' || true)
  waves=$(pgrep -f 'fb-admit\.sh' 2>/dev/null | wc -l)

  slots=$(free_slots || echo "")
  beat "live=$live waves=$waves slots=${slots:-?} queued=$(ls -1 "$Q"/*.tsv 2>/dev/null | wc -l)"

  # Queued waves first, and they no longer wait for an empty machine. A pipeline call blocks
  # this loop for as long as a critique takes -- ten minutes or more -- so running it ahead of
  # dispatch starves the thing the daemon exists to do. Observed: heartbeat 638s stale with
  # wave21 sitting queued.
  launched=0
  if [ "${waves:-0}" -lt "$MAX_WAVES" ] && [ -n "$slots" ]; then
    next=$(ls -1v "$Q"/*.tsv 2>/dev/null | head -1)
    if [ -n "$next" ]; then
      arms=$(wave_arms "$next")
      if [ "${arms:-0}" -lt 1 ]; then
        say "REFUSING $(basename "$next"): cannot read an arm list from it"
      elif [ $(( live + arms )) -gt "$slots" ]; then
        say "waiting: $(basename "$next") needs $arms arms, $live live of $slots slots"
      elif collides "$next"; then
        :   # collides() says why
      else
      # Claim the file BEFORE launching: fb-wave.sh detaches and returns immediately, so
      # moving it afterwards yanks the matrix out from under fb-admit.sh.
        base=$(basename "$next")
        if mv "$next" "$DONE/$base" 2>/dev/null; then
          say "launching $base: $arms arms, $live live of $slots slots, $waves/$MAX_WAVES waves"
          if ./fb-wave.sh "$DONE/$base" >> "$LOG" 2>&1; then
            say "launched $base"
            launched=1
          else
            mv "$DONE/$base" "$Q/$base" 2>/dev/null
            say "launch REFUSED for $base -- requeued"
          fi
        fi
      fi
    fi
  fi
  if [ "$launched" -eq 1 ]; then sleep 10; continue; fi

  # Everything below needs a QUIET machine. The pipeline blocks this loop until it finishes,
  # so running it while agents are live would stall dispatch for the length of a critique --
  # which is exactly the starvation the concurrency change above exists to end.
  if [ "${live:-0}" -eq 0 ] && [ "${waves:-0}" -eq 0 ]; then
    # Nothing queued: finish the tail of what is already dispatched. The subjective tier
    # (critique -> promote -> prove) has no other driver.  (bead farmerbob-k9f)
    pending=$(for sc in "$HOME/.local/share/farmerbob/logs"/*.score.json; do
                [ -f "$sc" ] || continue
                t=$(basename "$sc" .score.json)
                [ -f .fb/prompts/"$t".md ] || continue
                f=$(fb_target "$t")
                [ -n "$f" ] || continue
                [ -f "$f" ] && continue
                [ -s "$HOME/.local/share/farmerbob/logs/$t.claims.json" ] && continue
                [ -f "$DONE/.skip.$t" ] && continue
                echo "$t"
              done | head -1)
    if [ -n "$pending" ]; then
      crate=$(awk -F'\t' -v t="$pending" '$1==t{print $2}' "$DONE"/*.tsv .fb/wave*.tsv 2>/dev/null | head -1)
      beat "pipeline $pending (blocks this loop until it finishes)"
      say "running missing pipeline stages for $pending"
      if ! ./fb-pipeline.sh "$pending" "${crate:-farmerbob-core}" >> "$LOG" 2>&1; then
        say "pipeline FAILED for $pending -- skipping"
        touch "$DONE/.skip.$pending"
      fi
      sleep 10
      continue
    fi

    say "IDLE, queue empty -- nothing to launch; orchestrator must stock .fb/queue/"
  else
    say "busy: $live agents, $waves dispatcher(s)"
  fi
  sleep 120
done
