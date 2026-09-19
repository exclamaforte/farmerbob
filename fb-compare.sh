#!/usr/bin/env bash
# fb-compare — rescore every candidate for a bead from its worktree, uniformly.
# Prototype of `fb compare`. Does NOT trust the agent or the original run record:
# it rebuilds and reruns the tests itself.
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
BEAD="${1:?bead id}"; CRATE="${2:-farmerbob-core}"
LOG_ROOT="$HOME/.local/share/farmerbob/logs"
OUT="$LOG_ROOT/$BEAD.compare.json"

echo "[" > "$OUT"; first=1
for META in "$LOG_ROOT/$BEAD--"*.json; do
  [ -f "$META" ] || continue
  case "$META" in *.compare.json) continue;; esac
  WT=$(python3 -c "import json;print(json.load(open('$META'))['worktree'])")
  SRC=$(python3 -c "import json;print(json.load(open('$META'))['source'])")
  DUR=$(python3 -c "import json;print(json.load(open('$META'))['duration_s'])")
  RC=$(python3 -c "import json;print(json.load(open('$META'))['rc'])")
  [ -d "$WT" ] || continue

  LOC=$(git -C "$WT" diff --numstat HEAD -- crates/ | awk '{a+=$1} END{print a+0}')
  B="FAIL"; T="FAIL"; N=0; CLIPPY=0
  TLOG=$(mktemp)
  if (cd "$WT" && cargo build -p "$CRATE" >"$TLOG" 2>&1); then B="pass"; fi
  if [ "$B" = "pass" ] && (cd "$WT" && cargo test -p "$CRATE" >"$TLOG" 2>&1); then
    T="pass"; N=$(grep -oE '[0-9]+ passed' "$TLOG" | awk '{s+=$1} END{print s+0}')
  fi
  if [ "$B" = "pass" ]; then
    CLIPPY=$( (cd "$WT" && cargo clippy -p "$CRATE" 2>&1) | grep -cE '^(warning|error)' )
  fi
  rm -f "$TLOG"

  V="FAIL"
  if [ "$B" = "pass" ] && [ "$T" = "pass" ] && [ "$LOC" -gt 30 ] && [ "$N" -gt 0 ]; then V="PASS"
  elif [ "$LOC" -eq 0 ]; then V="NO-OP"
  elif [ "$B" != "pass" ]; then V="NO-COMPILE"
  elif [ "$N" -eq 0 ]; then V="NO-TESTS"; fi

  [ $first -eq 0 ] && echo "," >> "$OUT"; first=0
  python3 -c "
import json,sys
json.dump({'source':'$SRC','verdict':'$V','build':'$B','test':'$T','tests_run':$N,
'lines':$LOC,'clippy':$CLIPPY,'duration_s':$DUR,'rc':$RC,'worktree':'$WT'},sys.stdout,indent=1)" >> "$OUT"
  printf '%-24s %-11s build=%-5s tests=%-3s clippy=%-3s %5sL %4ss\n' "$SRC" "$V" "$B" "$N" "$CLIPPY" "$LOC" "$DUR"
done
echo "]" >> "$OUT"
echo "-> $OUT"
