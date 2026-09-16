#!/usr/bin/env bash
# fb-crossx — cross-examination: run every candidate's test suite against every candidate's
# implementation. N^2 local executions, zero tokens, no model opinions.
#
#   suite_A on impl_B fails  =>  B has a bug, OR A's test is wrong. The column says which:
#     a test failing on 1 of N   -> real defect in that one
#     a test failing on N-1 of N -> the test is wrong (over-fitted to its author's internals)
#
#   survival  = fraction of OTHER arms' discriminating suites this impl passes
#   discovery = number of other impls this arm's suite breaks (discounted if it breaks ~all)
#
# Runs on copies; never touches a candidate worktree.        (bead farmerbob-xle)
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
. /home/gabe/Documents/farmerbob/fb-verdict.sh
BEAD="${1:?bead}"; CRATE="${2:-farmerbob-core}"; FILE="${3:?relative path of the file under test}"
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
REPO=/home/gabe/Documents/farmerbob
OUT="$HOME/.local/share/farmerbob/logs/$BEAD.crossx.json"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

ARMS=(); for W in "$WT_ROOT/$BEAD--"*; do
  [ -d "$W" ] || continue
  [ -f "$W/.fb-task.md" ] && continue          # never score a live run
  [ -f "$W/$FILE" ] || continue
  ARMS+=("${W##*/$BEAD--}")
done
echo "candidates: ${ARMS[*]}"
[ "${#ARMS[@]}" -lt 2 ] && { echo "need >=2"; exit 1; }

# Collect each arm's tests from EVERY file in the crate, not just one. Implementations that
# split into modules keep their tests beside the code, and a test module living in run.rs
# that says `use super::*` means crate::run::*, which breaks when grafted elsewhere.
# Rewrite those imports to the crate root, which is what the pinned public API is exported
# from anyway.
SRCDIR_REL="crates/$CRATE/src"
for a in "${ARMS[@]}"; do
  # A test module inherits its parent FILE's imports. Grafted into lib.rs those are gone,
  # producing E0433 "cannot find type PathBuf/Utc/Uuid" -- tooling noise, not an API
  # mismatch. Re-supply the crate's common imports so that only genuine signature
  # divergence shows up as nocompile.
  cat > "$TMP/suite.$a.rs" <<'PRE'
#[cfg(test)]
#[allow(unused_imports)]
mod xprelude { pub use std::path::PathBuf; pub use std::time::Duration;
               pub use chrono::{DateTime, Utc}; pub use uuid::Uuid; }
PRE
  # Which files hold this candidate's work. Prefer the task's own declaration -- the spec
  # states `fb:creates` / `fb:modifies`, which is precise and survives the candidate being
  # committed or merged (at which point a diff against HEAD is empty). Fall back to the
  # worktree diff, then to the single target file.
  #
  # Collecting EVERY file in the crate was wrong: it swept up the inherited domain-model
  # tests, which reference types from sibling modules (ExperimentId lives in crate::ids,
  # not crate::experiment) and cannot compile once grafted elsewhere.
  n=0
  DECLARED=$(grep -ohE '<!-- fb:(creates|modifies) [^ ]+ -->' "$REPO/.fb/prompts/$BEAD.md" 2>/dev/null | awk '{print $3}')
  CHANGED=$( { git -C "$WT_ROOT/$BEAD--$a" diff --name-only HEAD -- "$SRCDIR_REL" 2>/dev/null
               git -C "$WT_ROOT/$BEAD--$a" ls-files --others --exclude-standard "$SRCDIR_REL" 2>/dev/null
             } | sort -u )
  FILES="${DECLARED:-$CHANGED}"
  FILES="${FILES:-$FILE}"
  for rel in $FILES; do
    f="$WT_ROOT/$BEAD--$a/$rel"
    [ -f "$f" ] || continue
    case "$f" in *.rs) ;; *) continue ;; esac
    # `use super::*` resolves against the module the tests were WRITTEN in, not the one they
    # are grafted into. A suite from vrouter.rs means crate::vrouter::*; one from lib.rs means
    # crate::*. Rewriting every case to `crate::*` silently pointed at the wrong module and
    # made every suite look API-incompatible.
    base=$(basename "$f" .rs)
    if [ "$base" = lib ]; then modpath="crate"; else modpath="crate::$base"; fi
    # A test module inherits its parent FILE's top-level imports. The hand-maintained
    # xprelude guessed at which ones (PathBuf/Utc/Uuid) and missed serde_json::Value, so a
    # suite failed to compile against its OWN implementation -- an impossible result that
    # marks the instrument, not the candidate.  Carry the file's real imports instead of
    # predicting them.  (bead farmerbob-74l)
    USES=$(awk '/#\[cfg\(test\)\]/{exit} /^use /{print}' "$f")
    awk '/#\[cfg\(test\)\]/{f=1} f' "$f" \
      | sed -e "s/^\( *\)mod tests/\1mod xtests_${n}/" \
            -e "s|use super::\*;|use ${modpath}::*; use crate::xprelude::*;|" \
            -e "s|use super::|use ${modpath}::|" \
      | awk -v uses="$USES" '{print} /^ *mod xtests_[0-9]+ *\{/ && !done {print uses; done=1}' \
      >> "$TMP/suite.$a.rs"
    echo >> "$TMP/suite.$a.rs"
    n=$((n+1))
  done
  printf '  suite %-22s %s lines, %s tests\n' "$a" "$(wc -l < "$TMP/suite.$a.rs")" "$(grep -c '#\[test\]' "$TMP/suite.$a.rs")"
done

# N^2 cargo cycles. Measured: one cold cycle peaks at ~881M across rustc+cargo, so a machine
# with this much free memory runs several concurrently. Verification is cpu/memory-bound
# while the agents are network-bound, so the two are limited independently.
CX_SLOTS=${FB_CX_SLOTS:-4}
RES="$TMP/res"; mkdir -p "$RES"
run_one() {
  local impl="$1" suite="$2"
  local C="$TMP/run.$impl.$suite"; rm -rf "$C"; mkdir -p "$C"
  (cd "$WT_ROOT/$BEAD--$impl" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$C" && tar xf -) 2>/dev/null
  for f in "$C/$SRCDIR_REL"/*.rs; do
    [ -f "$f" ] || continue
    awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" > "$f.stripped" && mv "$f.stripped" "$f"
  done
  cat "$TMP/suite.$suite.rs" >> "$C/$SRCDIR_REL/lib.rs"
  local o="$C/out"
  if (cd "$C" && timeout 300 cargo test -p "$CRATE" >"$o" 2>&1); then echo pass > "$RES/$impl|$suite"
  elif grep -qE '^error\[E[0-9]+\]:|could not compile' "$o"; then echo nocompile > "$RES/$impl|$suite"
  else echo fail > "$RES/$impl|$suite"; fi
  rm -rf "$C"
}
for impl in "${ARMS[@]}"; do
  for suite in "${ARMS[@]}"; do
    while [ "$(jobs -rp | wc -l)" -ge "$CX_SLOTS" ]; do sleep 1; done
    run_one "$impl" "$suite" &
  done
done
wait
declare -A R
for impl in "${ARMS[@]}"; do
  for suite in "${ARMS[@]}"; do
    R["$impl|$suite"]=$(cat "$RES/$impl|$suite" 2>/dev/null || echo nocompile)
  done
done
for impl in "${ARMS[@]}"; do
  for suite in "${ARMS[@]}"; do
    if false; then
      :
    fi
  done
done

echo; printf '%-24s' 'impl \ suite'; for s in "${ARMS[@]}"; do printf '%-10s' "${s:0:9}"; done; echo
for impl in "${ARMS[@]}"; do
  printf '%-24s' "${impl:0:23}"
  for suite in "${ARMS[@]}"; do
    v=${R["$impl|$suite"]}
    case "$v" in pass) printf '%-10s' " ." ;; fail) printf '%-10s' " FAIL" ;; *) printf '%-10s' " nocomp" ;; esac
  done; echo
done

RJSON=$(for k in "${!R[@]}"; do printf '"%s":"%s",' "$k" "${R[$k]}"; done)
RJSON="{${RJSON%,}}"
export RJSON
python3 - "$OUT" "${ARMS[*]}" <<PY
import json,sys,os
arms=sys.argv[2].split()
R=json.loads(os.environ['RJSON'])
res={}
for a in arms:
    others=[o for o in arms if o!=a]
    # nocompile is an API MISMATCH, not evidence about correctness -- the suite was written
    # against a different surface. Only a genuine assertion failure counts as a discovery,
    # and only a compiling suite can say anything about survival.
    def verdict(i, s): return R.get(f"{i}|{s}")
    compiles = lambda s: [i for i in arms if i != s and verdict(i, s) != "nocompile"]
    # a suite discriminates if, among impls it COMPILES against, it fails some but not all
    disc = []
    for s in arms:
        c = compiles(s)
        if len(c) < 2: continue
        f = sum(1 for i in c if verdict(i, s) == "fail")
        if 0 < f < len(c): disc.append(s)
    rel = [s for s in disc if s != a and verdict(a, s) != "nocompile"]
    surv = sum(1 for s in rel if verdict(a, s) == "pass")
    broke = [i for i in others if verdict(i, a) == "fail"]
    incompat = [i for i in others if verdict(i, a) == "nocompile"]
    overfit = len(broke) == len(compiles(a)) and len(compiles(a)) > 1
    res[a]={"survival": (surv/len(rel)) if rel else None, "survived":surv, "of":len(rel),
            "discovery": 0 if overfit else len(broke), "broke":broke,
            "api_incompatible_with": len(incompat),
            "suite_discriminating": a in disc, "suite_overfitted": overfit}
json.dump(res, open(sys.argv[1],'w'), indent=1)
print()
print(f"{'ARM':<24}{'SURVIVAL':>10}{'DISCOVERY':>11}{'API-INCOMPAT':>14}  SUITE")
for a,v in sorted(res.items(), key=lambda kv: (-(kv[1]['survival'] or 0), -kv[1]['discovery'])):
    s = f"{v['survived']}/{v['of']}" if v['of'] else "n/a"
    tag = "OVER-FITTED" if v['suite_overfitted'] else ("discriminating" if v['suite_discriminating'] else "no signal")
    print(f"{a:<24}{s:>10}{v['discovery']:>11}{v['api_incompatible_with']:>14}  {tag}")
PY
echo "-> $OUT"
