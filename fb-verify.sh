#!/usr/bin/env bash
# fb-verify — the behavioural verifier role.
#
# An arm writes tests from the SPEC ALONE, never seeing any candidate's code, then that
# suite is run against every candidate. Independence by construction: a suite written after
# reading an implementation inherits that implementation's misreading of the spec.
#
# Unlike cross-examination (fb-crossx), API divergence is a FINDING here rather than noise --
# the spec is the authority, so a candidate whose signatures do not match it has failed a
# requirement rather than merely being incomparable.
#
#   fb-verify.sh <bead> <crate> <target-file-rel> <verifier-arm> [spec.md]
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
. /home/gabe/Documents/farmerbob/fb-verdict.sh
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target file}"; VARM="${4:?verifier arm}"
SPEC="${5:-.fb/prompts/$BEAD.md}"
REPO=/home/gabe/Documents/farmerbob
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
LOG_ROOT="$HOME/.local/share/farmerbob/logs"
OUT="$LOG_ROOT/$BEAD.verify.json"
SUITE="$LOG_ROOT/$BEAD.verifier-suite.rs"

# ---- 1. the verifier writes tests from the spec, at the BASE commit -------------------
if [ ! -s "$SUITE" ] || [ "${FB_REGEN:-0}" = 1 ]; then
  P=$(mktemp); trap 'rm -f "$P"' EXIT
  python3 - "$REPO/.fb/prompts/_verifier.md" "$SPEC" "$TARGET" "$CRATE" > "$P" <<'PY'
import sys,re
tpl=open(sys.argv[1]).read()
spec=open(sys.argv[2]).read()
spec=re.sub(r'^<!-- fb:(creates|modifies).*?-->\s*$','',spec,flags=re.M)   # strip harness directives
# strip the scoring rubric: the verifier is not being scored on it and it leaks the mechanism
spec=spec.split('## How this will be scored')[0]
print(tpl.replace('{TARGET_FILE}',sys.argv[3]).replace('{CRATE}',sys.argv[4]).replace('{SPEC}',spec.strip()))
PY
  echo "dispatching verifier $VARM on the spec (no candidate code in context)"
  "$REPO/fb-dispatch.sh" "$VARM" "verify-$BEAD" "$P" "$CRATE" || true
  VW="$WT_ROOT/verify-$BEAD--$VARM"
  awk '/#\[cfg\(test\)\]/{f=1} f' "$VW/$TARGET" 2>/dev/null \
    | sed -e 's/use super::\*;/use crate::*; use crate::vprelude::*;/' \
          -e 's/^\( *\)mod [a-z_]*/\1mod verifier/' > "$SUITE"
fi
NT=$(grep -c '#\[test\]' "$SUITE" 2>/dev/null || echo 0)
echo "verifier suite: $NT tests, $(wc -l < "$SUITE") lines  -> $SUITE"
[ "$NT" -eq 0 ] && { echo "verifier produced no tests"; exit 1; }

# ---- 2. run it against every terminal candidate ---------------------------------------
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
printf '%-24s %-12s %s\n' ARM RESULT DETAIL
: > "$TMP/rows"
for W in "$WT_ROOT/$BEAD--"*; do
  [ -d "$W" ] || continue
  A="${W##*/$BEAD--}"
  [ -f "$W/.fb-task.md" ] && { printf '%-24s %-12s\n' "$A" LIVE; continue; }
  C="$TMP/c"; rm -rf "$C"; mkdir -p "$C"
  (cd "$W" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$C" && tar xf -) 2>/dev/null
  SD="$C/crates/$CRATE/src"
  [ -d "$SD" ] || { printf '%-24s %-12s\n' "$A" NO-CRATE; continue; }
  for f in "$SD"/*.rs; do awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" > "$f.s" && mv "$f.s" "$f"; done
  cat >> "$SD/lib.rs" <<'PRE'
#[cfg(test)]
#[allow(unused_imports)]
pub mod vprelude { pub use std::path::PathBuf; pub use std::time::Duration;
                   pub use chrono::{DateTime, Utc}; pub use uuid::Uuid; }
PRE
  cat "$SUITE" >> "$SD/lib.rs"
  if (cd "$C" && timeout 400 cargo test -p "$CRATE" verifier >"$TMP/o" 2>&1); then
    n=$(grep -oE '[0-9]+ passed' "$TMP/o"|head -1|awk '{print $1}')
    n=${n:-0}
    # zero executed tests is not a pass. cargo reports "ok. 0 passed" for an empty filter,
    # which is exactly the failure this project was founded on (bead farmerbob-slh).
    if [ "$n" -eq 0 ]; then
      printf '%-24s %-12s %s\n' "$A" NO-TESTS-RAN "suite did not execute -- harness fault, not a result"
      echo "$A notests 0" >> "$TMP/rows"
    else
      printf '%-24s %-12s %s/%s passed\n' "$A" PASS "$n" "$NT"; echo "$A pass $n" >> "$TMP/rows"
    fi
  elif grep -qE '^error\[E[0-9]+\]:|could not compile' "$TMP/o"; then
    w=$(grep -m1 -oE 'cannot find [a-z]+ `[A-Za-z_]+`|no method named `[a-z_]+`|takes [0-9]+ arguments?' "$TMP/o")
    printf '%-24s %-12s %s\n' "$A" SPEC-DEVIATION "${w:-does not match the specified interface}"
    echo "$A deviation 0" >> "$TMP/rows"
  else
    fails=$(grep -cE '^test verifier::.* FAILED' "$TMP/o")
    n=$(grep -oE '[0-9]+ passed' "$TMP/o"|head -1|awk '{print $1}')
    names=$(grep -oE '^test verifier::[a-z_]+' "$TMP/o" | sed 's/test verifier:://' | head -3 | tr '\n' ' ')
    printf '%-24s %-12s %s/%s passed; failed: %s\n' "$A" FAIL "${n:-0}" "$NT" "$names"
    echo "$A fail ${n:-0}" >> "$TMP/rows"
  fi
done

# ---- 3. flag tests that fail everywhere: suspect the VERIFIER, not the candidates -------
python3 - "$OUT" "$NT" "$TMP/rows" <<'PY'
import sys,json
rows=[l.split() for l in open(sys.argv[3]).read().splitlines() if l.strip()]
nt=int(sys.argv[2])
res={a:{"result":r,"passed":int(p),"of":nt} for a,r,p in rows}
ok=[a for a,v in res.items() if v["result"]=="pass"]
json.dump({"verifier_tests":nt,"candidates":res,"all_failed":len(ok)==0 and len(res)>0},
          open(sys.argv[1],'w'), indent=1)
print()
if res and not ok:
    print("!! NO candidate passed. Suspect the VERIFIER's reading of the spec, or an")
    print("   ambiguous spec -- escalate before recording any arm as failed.")
else:
    print(f"{len(ok)}/{len(res)} candidates satisfied the spec-derived suite")
PY
echo "-> $OUT"
