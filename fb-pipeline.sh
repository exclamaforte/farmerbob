#!/usr/bin/env bash
# fb-pipeline — run EVERY stage for one task, in order, skipping what already exists.
#
# The wave path (fb-wave -> fb-admit -> fb-dispatch) implements Dispatch only. Score, then
# the subjective tier -- Critique, Promote, Prove -- used to be driven by `fb trial` and
# were silently lost when waves replaced it: fifteen tasks were adjudicated on objective
# metrics alone and nothing reported the absence.  (bead farmerbob-k9f)
#
# This is the missing tail. It is idempotent, so the autopilot can call it repeatedly.
#
#   fb-pipeline.sh <task> <crate> <target-rel>
set -uo pipefail
cd /home/gabe/Documents/farmerbob
T="${1:?task}"; CRATE="${2:-farmerbob-core}"; TARGET="${3:-}"
LOGS="$HOME/.local/share/farmerbob/logs"

# Infer the target from the spec's own declaration rather than making the caller repeat it.
if [ -z "$TARGET" ]; then
  TARGET=$(grep -ohE '<!-- fb:creates [^ ]+ -->' ".fb/prompts/$T.md" 2>/dev/null | awk '{print $3}' | head -1)
fi
[ -n "$TARGET" ] || { echo "$T: cannot determine target file"; exit 2; }
echo "== pipeline $T ($CRATE, $TARGET)"

# The set of gate-passing candidates, as a stable fingerprint. An artefact is valid only
# for the field it was computed over: reviewer was cross-examined 2x2 while five passing
# candidates sat in the worktrees, and the stale matrix reported a TIE between two arms
# neither of which was best. Staleness inverted the answer rather than delaying it.
field() {
  python3 -c "
import json,sys
try: d=json.load(open('$LOGS/$T.score.json'))
except Exception: sys.exit(0)
print(','.join(sorted(r['source'] for r in d if r.get('verdict')=='PASS')))" 2>/dev/null
}

run_stage() { # run_stage <name> <artefact> <command...>
  local name="$1" artefact="$2"; shift 2
  local now sig="$artefact.field"
  now=$(field)
  if [ -s "$artefact" ]; then
    if [ -n "$now" ] && [ "$now" != "$(cat "$sig" 2>/dev/null)" ]; then
      echo "  $name: field changed -> re-running"
      echo "     was: $(cat "$sig" 2>/dev/null || echo '<unrecorded>')"
      echo "     now: $now"
    else
      echo "  $name: already done"; return 0
    fi
  fi
  echo "  $name: running"
  if "$@" >> "$LOGS/$T.pipeline.log" 2>&1; then echo "  $name: ok"; printf '%s' "$now" > "$sig"
  else echo "  $name: FAILED (see $LOGS/$T.pipeline.log)"; return 1; fi
}

run_stage score    "$LOGS/$T.score.json"   bash ./fb-score.sh   "$T" "$CRATE"
run_stage crossx   "$LOGS/$T.crossx.json"  bash ./fb-crossx.sh  "$T" "$CRATE" "$TARGET"
run_stage critique "$LOGS/$T.claims.json"  bash ./fb-critique.sh "$T" "$CRATE" "$TARGET"
run_stage promote  "$LOGS/$T.promoted.json" bash ./fb-promote.sh "$T" "$CRATE" "$TARGET"
run_stage prove    "$LOGS/$T.proved.json"  bash ./fb-prove.sh   "$T" "$CRATE" "$TARGET"
echo "== $T ready for adjudication"
