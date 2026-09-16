#!/usr/bin/env bash
# fb-status — everything the orchestrator needs to decide what to do next, in one call.
# Written because two hours were lost to an idle machine while finished results sat unread.
set -uo pipefail
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
  f=$(grep -ohE '<!-- fb:creates [^ ]+ -->' ".fb/prompts/$t.md" 2>/dev/null | awk '{print $3}' | head -1)
  if [ -n "$f" ] && [ -f "$f" ]; then state="MERGED"; else state="** NEEDS ADJUDICATION **"; fi
  n=$(python3 -c "import json;d=json.load(open('$s'));print(sum(1 for r in d if r.get('verdict')=='PASS'))" 2>/dev/null || echo '?')
  printf '  %-16s %-26s %s passing\n' "$t" "$state" "$n"
done

echo "== queue =="
for p in .fb/prompts/*.md; do
  b=$(basename "$p" .md); case "$b" in _*) continue ;; esac
  [ -f "$LOGS/$b.score.json" ] && continue
  echo "  unspent spec: $b"
done
ls .fb/wave*.tsv 2>/dev/null | while read -r w; do
  n=$(awk 'NF' "$w" | wc -l)
  done_n=0
  while IFS=$'\t' read -r t _ _; do [ -f "$LOGS/$t.score.json" ] && done_n=$((done_n+1)); done < "$w"
  printf '  %-18s %s/%s tasks scored\n' "$(basename "$w")" "$done_n" "$n"
done

echo "== bd ready (leaf tasks only) =="
bd ready 2>/dev/null | grep -v '\[epic\]' | head -8 | sed 's/^/  /'
