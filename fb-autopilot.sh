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
say "autopilot up (pid $$)"

while :; do
  live=$(systemctl --user list-units --type=scope --no-legend 2>/dev/null \
         | grep -c 'fb-[a-z0-9-]*--.*\.scope' || true)
  waves=$(pgrep -f 'fb-admit\.sh' 2>/dev/null | wc -l)

  if [ "${live:-0}" -eq 0 ] && [ "${waves:-0}" -eq 0 ]; then
    next=$(ls -1 "$Q"/*.tsv 2>/dev/null | head -1)
    if [ -n "$next" ]; then
      # Claim the file BEFORE launching. fb-wave.sh detaches and returns immediately, so
      # moving it afterwards yanks the matrix out from under fb-admit.sh, which then reports
      # "no eligible runs" against a path that no longer exists. Claim, then launch from the
      # claimed path; a refused launch is moved back.
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
    else
      say "idle, queue empty -- waiting for the orchestrator to stock .fb/queue/"
    fi
  fi
  sleep 120
done
