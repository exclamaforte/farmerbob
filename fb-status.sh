#!/usr/bin/env bash
# fb-status — everything the orchestrator needs to decide what to do next, in one call.
# Written because two hours were lost to an idle machine while finished results sat unread.
set -uo pipefail
. /home/gabe/Documents/farmerbob/fb-target.sh
cd /home/gabe/Documents/farmerbob
LOGS="$HOME/.local/share/farmerbob/logs"

live=$(systemctl --user list-units --type=scope --no-legend 2>/dev/null \
       | grep -o 'fb-[a-z0-9-]*--[a-z0-9_-]*' | sed 's/^fb-//' | sort -u)
nlive=$(printf '%s' "$live" | grep -c . || true)
waves=$(pgrep -cf "fb-admit\.sh" 2>/dev/null | head -1); waves=${waves:-0}

echo "== live =="
if [ "$nlive" -eq 0 ]; then echo "  no agents running (dispatcher processes: $waves)"
else printf '%s\n' "$live" | sed 's/^/  /'; fi

echo "== tasks with results, by adjudication state =="
for s in "$LOGS"/*.score.json; do
  [ -f "$s" ] || continue
  t=$(basename "$s" .score.json)
  # a task is merged when its declared file exists on master
  f=$(fb_target "$t"); verb=$(fb_target_verb "$t")
  # A task is not adjudicable until the SUBJECTIVE tier has run. The critique/promote/prove
  # stages silently stopped happening for 15 tasks when the parallel wave path replaced
  # `fb trial`, and nothing reported their absence because the objective numbers kept
  # flowing. A missing stage must be visible here.  (bead farmerbob-k9f)
  #
  # An explicit adjudication record wins over any inference. A task can be finished WITHOUT
  # a merge -- probe's twelve candidates were all Indeterminate because git had lost their
  # worktrees, so there was no winner to merge and never will be. Inferring state from the
  # filesystem had no way to say that, and the board went on demanding adjudication of a
  # task already adjudicated.  (bead farmerbob-13p)
  if [ -f ".fb/adjudicated/$t" ]; then
    state="$(head -1 ".fb/adjudicated/$t")"
  # "the declared file exists" is evidence of a merge only for a `creates` task. For a
  # `modifies` task the file exists BEFORE any work is done, so existence proves nothing
  # and would report MERGED for every unstarted edit task.
  elif [ "$verb" = creates ] && [ -n "$f" ] && [ -f "$f" ]; then state="MERGED"
  elif [ ! -f "$LOGS/$t.claims.json" ]; then state="** NEEDS CRITIQUE **"
  else state="** NEEDS ADJUDICATION **"; fi
  n=$(python3 -c "import json;d=json.load(open('$s'));print(sum(1 for r in d if r.get('verdict')=='PASS'))" 2>/dev/null || echo '?')
  printf '  %-16s %-26s %s passing\n' "$t" "$state" "$n"
done

echo "== queue =="
for p in .fb/prompts/*.md; do
  b=$(basename "$p" .md); case "$b" in _*) continue ;; esac
  [ -f "$LOGS/$b.score.json" ] && continue
  # A `creates` spec whose target ALREADY EXISTS is dead: dispatch's precondition rejects it,
  # or worse it runs and every arm correctly no-ops, measuring nothing. Merging a winner is
  # what kills them -- bandit-route was authored against a base where router.rs did not exist
  # and has been unrunnable ever since router.rs was merged. Listing it as available work
  # invites spending four arms on a guaranteed no-op.  (bead farmerbob-m71)
  tgt=$(fb_target "$b" 2>/dev/null)
  if [ "$(fb_target_verb "$b" 2>/dev/null)" = creates ] && [ -n "$tgt" ] && [ -f "$tgt" ]; then
    echo "  unspent spec: $b   ** STALE: $tgt already exists, a creates-task would no-op **"
  else
    echo "  unspent spec: $b"
  fi
done
ls .fb/wave*.tsv 2>/dev/null | while read -r w; do
  n=$(awk 'NF' "$w" | wc -l)
  done_n=0
  while IFS=$'\t' read -r t _ _; do [ -f "$LOGS/$t.score.json" ] && done_n=$((done_n+1)); done < "$w"
  printf '  %-18s %s/%s tasks scored\n' "$(basename "$w")" "$done_n" "$n"
done

echo "== bd ready (leaf tasks only) =="
bd ready 2>/dev/null | grep -v '\[epic\]' | head -8 | sed 's/^/  /'
