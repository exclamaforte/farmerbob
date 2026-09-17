#!/usr/bin/env bash
# fb-autopilot — keep agents working when the orchestrator is not around.
#
# The cron tick that adjudicates and dispatches lives inside a Claude session: it only fires
# while that REPL is idle, and it dies with the session. Twice now the machine has sat idle
# for hours with finished results nobody picked up. This is the part that does not depend on
# anyone waking up.
#
# It dispatches ONLY waves explicitly queued in .fb/queue/*.tsv, oldest first, and only when
# nothing else is running. It never adjudicates, never merges, never commits -- those need
# judgement. It just refuses to let the box be idle while there is queued work.
set -uo pipefail
cd /home/gabe/Documents/farmerbob
Q=.fb/queue; DONE=$Q/dispatched
LOG="$HOME/.local/share/farmerbob/logs/autopilot.log"
mkdir -p "$Q" "$DONE"

say() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >> "$LOG"; }
BEAT="$HOME/.local/share/farmerbob/logs/autopilot.heartbeat"
beat() { printf '%s pid=%s %s\n' "$(date -Is)" "$$" "$*" > "$BEAT"; }
say "autopilot up (pid $$)"; beat "starting"

while :; do
  live=$(systemctl --user list-units --type=scope --no-legend 2>/dev/null \
         | grep -c 'fb-[a-z0-9-]*--.*\.scope' || true)
  waves=$(pgrep -f 'fb-admit\.sh' 2>/dev/null | wc -l)

  beat "live=$live waves=$waves queued=$(ls -1 "$Q"/*.tsv 2>/dev/null | wc -l)"
  if [ "${live:-0}" -eq 0 ] && [ "${waves:-0}" -eq 0 ]; then
    # Queued waves first. A pipeline call blocks this loop for as long as a critique takes
    # -- ten minutes or more -- so running it ahead of dispatch starves the thing the daemon
    # exists to do. Observed: heartbeat 638s stale with wave21 sitting queued.
    next=$(ls -1v "$Q"/*.tsv 2>/dev/null | head -1)
    if [ -n "$next" ]; then
      # Claim the file BEFORE launching: fb-wave.sh detaches and returns immediately, so
      # moving it afterwards yanks the matrix out from under fb-admit.sh.
      base=$(basename "$next")
      if mv "$next" "$DONE/$base" 2>/dev/null; then
        say "idle; launching $base"
        if ./fb-wave.sh "$DONE/$base" >> "$LOG" 2>&1; then
          say "launched $base"
        else
          mv "$DONE/$base" "$Q/$base" 2>/dev/null
          say "launch REFUSED for $base -- requeued"
        fi
      fi
      sleep 10
      continue
    fi

    # Nothing queued: finish the tail of what is already dispatched. The subjective tier
    # (critique -> promote -> prove) has no other driver.  (bead farmerbob-k9f)
    pending=$(for sc in "$HOME/.local/share/farmerbob/logs"/*.score.json; do
                [ -f "$sc" ] || continue
                t=$(basename "$sc" .score.json)
                [ -f .fb/prompts/"$t".md ] || continue
                f=$(grep -ohE '<!-- fb:creates [^ ]+ -->' .fb/prompts/"$t".md 2>/dev/null | awk '{print $3}' | head -1)
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
