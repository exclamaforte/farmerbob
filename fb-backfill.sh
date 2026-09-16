#!/usr/bin/env bash
# fb-backfill — run the subjective tier for tasks that were adjudicated without it.
#
# Between 09:36 and 14:20 the critique/promote/prove stages silently stopped happening
# (farmerbob-k9f): the parallel wave path implements Dispatch only, and fifteen tasks were
# decided on objective metrics alone. The candidate worktrees still exist, so the reviews
# are recoverable -- and worth recovering twice over: any defect found is in code already
# merged to master, and the leaderboard has no data at all on which arms critique well.
#
# Sequential on purpose. Critics are unconfined (farmerbob-vi7) and admission control does
# not account for them, so running several tasks' worth concurrently alongside a live wave
# is how the box gets oversubscribed.
set -uo pipefail
cd /home/gabe/Documents/farmerbob
LOGS="$HOME/.local/share/farmerbob/logs"
export FB_SLOTS=${FB_SLOTS:-3}

TASKS="${*:-ux-json bandit-route prior crossx limit-detect speed promote liveness \
defect-sens store-mig cost-attr corpus divergence adjudicate cli-tree}"

for t in $TASKS; do
  [ -f ".fb/prompts/$t.md" ] || { echo "skip $t: no spec"; continue; }
  if [ -s "$LOGS/$t.claims.json" ]; then echo "skip $t: already has claims"; continue; fi
  tgt=$(grep -ohE '<!-- fb:creates [^ ]+ -->' ".fb/prompts/$t.md" | awk '{print $3}' | head -1)
  [ -n "$tgt" ] || { echo "skip $t: no fb:creates target"; continue; }
  crate=$(awk -F/ '{print $2}' <<< "$tgt")
  echo "======== $t ($crate, $tgt)"
  bash ./fb-critique.sh "$t" "$crate" "$tgt" 2>&1 | sed 's/^/  /'
  bash ./fb-promote.sh  "$t" "$crate" "$tgt" 2>&1 | sed 's/^/  /'
  bash ./fb-prove.sh    "$t" "$crate" "$tgt" 2>&1 | sed 's/^/  /'
done
echo "======== backfill complete"
