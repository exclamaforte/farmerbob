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

# Only candidates that PASSED the gate are cross-examined. A candidate whose build failed
# cannot run its own suite against its own code, which would trip the diagonal invariant and
# void a matrix that is otherwise sound -- ifm-k2-think did exactly that on adjudicate, where
# the other three candidates were fine.  (bead farmerbob-74l)
PASSED=$(python3 -c "
import json,sys
try: d=json.load(open('$HOME/.local/share/farmerbob/logs/$BEAD.score.json'))
except Exception: sys.exit(0)
print(' '.join(r['source'] for r in d if r.get('verdict')=='PASS'))" 2>/dev/null)

ARMS=(); for W in "$WT_ROOT/$BEAD--"*; do
  [ -d "$W" ] || continue
  [ -f "$W/.fb-task.md" ] && continue          # never score a live run
  [ -f "$W/$FILE" ] || continue
  a="${W##*/$BEAD--}"
  if [ -n "$PASSED" ]; then
    case " $PASSED " in *" $a "*) ;; *) echo "  skip $a (did not pass the gate)"; continue ;; esac
  fi
  ARMS+=("$a")
done
echo "candidates: ${ARMS[*]}"
[ "${#ARMS[@]}" -lt 2 ] && { echo "need >=2"; exit 1; }

# Collect each arm's tests from EVERY file in the crate, not just one. Implementations that
# split into modules keep their tests beside the code, and a test module living in run.rs
# that says `use super::*` means crate::run::*, which breaks when grafted elsewhere.
# Rewrite those imports to the crate root, which is what the pinned public API is exported
# from anyway.
SRCDIR_REL="crates/$CRATE/src"
TARGET_REL="$FILE"          # the module a suite is grafted into
slug() { printf '%s' "$1" | tr -c 'a-zA-Z0-9' '_'; }
for a in "${ARMS[@]}"; do
  # A test module inherits its parent FILE's imports. Grafted into lib.rs those are gone,
  # producing E0433 "cannot find type PathBuf/Utc/Uuid" -- tooling noise, not an API
  # mismatch. Re-supply the crate's common imports so that only genuine signature
  # divergence shows up as nocompile.
  rm -f "$TMP/suite.$a."*.rs; : > "$TMP/manifest.$a"
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
    # A test module inherits its parent FILE's top-level imports. The hand-maintained
    # xprelude guessed at which ones (PathBuf/Utc/Uuid) and missed serde_json::Value, so a
    # suite failed to compile against its OWN implementation -- an impossible result that
    # marks the instrument, not the candidate.  Carry the file's real imports instead of
    # predicting them.  (bead farmerbob-74l)
    USES=$(awk '/#\[cfg\(test\)\]/{exit} /^use /{print}' "$f")
    awk '/#\[cfg\(test\)\]/{f=1} f' "$f" \
      | sed -e "s/^\( *\)mod tests/\1mod xtests_${n}/" \
      | awk -v uses="$USES" '{print} /^ *mod xtests_[0-9]+ *\{/ && !done {print uses; done=1}' \
      >> "$TMP/suite.$a.$(slug "$rel").rs"
    echo >> "$TMP/suite.$a.$(slug "$rel").rs"
    grep -qxF "$rel" "$TMP/manifest.$a" || echo "$rel" >> "$TMP/manifest.$a"
    n=$((n+1))
  done
  printf '  suite %-22s %s lines, %s tests, %s file(s)\n' "$a" \
    "$(cat "$TMP/suite.$a."*.rs 2>/dev/null | wc -l)" \
    "$(cat "$TMP/suite.$a."*.rs 2>/dev/null | grep -c '#\[test\]')" \
    "$(wc -l < "$TMP/manifest.$a")"
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
  # A test module is a CHILD of the module it tests and can see its private items.
  # Grafting it into lib.rs stripped that privilege, so a suite touching a private field
  # failed to compile against its OWN implementation (or-hy3 on speed: E0616 on
  # first_write_ms) and the matrix read "no signal". Graft into the module under test,
  # where `use super::*` resolves exactly as the author wrote it.  (bead farmerbob-74l)
  # A task may declare several files (fb:creates + fb:modifies). Concatenating their tests
  # into one graft put quota.rs's tests inside limit_signal.rs, where `use super::*` names
  # the wrong module and BOTH arms failed against their own code. Each file's tests go home.
  local grafted=0 rel
  while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    local sfile="$TMP/suite.$suite.$(slug "$rel").rs"
    [ -f "$sfile" ] || continue
    if [ -f "$C/$rel" ]; then cat "$sfile" >> "$C/$rel"; grafted=$((grafted+1)); fi
  done < "$TMP/manifest.$suite"
  # The implementation does not have the file this suite tests: a genuine API divergence.
  [ "$grafted" -eq 0 ] && { echo nocompile > "$RES/$impl|$suite"; rm -rf "$C"; return; }
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
# THE PARTITION SHAPE. A defect shows as 1-of-N; an over-fitted suite as N-1-of-N. A clean
# split into camps that pass within themselves and fail across is NEITHER -- it is the spec
# admitting two readings, with each camp implementing one correctly. compare produced exactly
# that and the harness reported four discriminating suites and survival 1/3 for everyone,
# which says every arm found two defects when none did.  (bead: partition shape)
detect_partition() {
  python3 - "$RJSON_TMP" "${ARMS[*]}" <<'PYP'
import json, sys
R = json.load(open(sys.argv[1])); arms = sys.argv[2].split()
if len(arms) < 4: raise SystemExit(1)
# camp(a) = the set of suites a's IMPLEMENTATION passes
camps = {}
for a in arms:
    camps[a] = frozenset(s for s in arms if R.get(f"{a}|{s}") == "pass")
groups = {}
for a, c in camps.items(): groups.setdefault(c, []).append(a)
if len(groups) < 2: raise SystemExit(1)
for c, members in groups.items():
    # internally all-pass, and fails every arm outside the camp
    if set(members) - set(c): raise SystemExit(1)
    for a in members:
        for other in arms:
            if other in c: continue
            if R.get(f"{a}|{other}") != "fail": raise SystemExit(1)
print(" | ".join("{" + ",".join(sorted(m)) + "}" for m in groups.values()))
PYP
}

# THE DIAGONAL INVARIANT. Every arm's own suite must pass against its own implementation:
# it demonstrably did so inside the candidate's worktree, so a failure here is the transplant,
# never the candidate. Twice this produced a matrix that read "no signal" -- once from dropped
# file-level imports, once from grafting into lib.rs and losing private-field access -- and
# both times the harness reported two candidates as indistinguishable while measuring nothing.
# A broken instrument must say so instead of returning a confident null.  (bead farmerbob-74l)
DIAG_BAD=()
for a in "${ARMS[@]}"; do
  [ "${R["$a|$a"]}" = pass ] || DIAG_BAD+=("$a(${R["$a|$a"]})")
done
if [ "${#DIAG_BAD[@]}" -gt 0 ]; then
  echo
  echo "VOID: the diagonal is not all-pass -- ${DIAG_BAD[*]}"
  echo "A suite that cannot run against the code it shipped with is a grafting failure."
  echo "Scores are NOT written; fix the transplant before trusting any cell."
  exit 3
fi

RJSON_TMP=$(mktemp)
python3 -c "
import json,sys
R={}
for k in sys.argv[1:]:
    a,b,v=k.split('|'); R[f'{a}|{b}']=v
json.dump(R,open('$RJSON_TMP','w'))" $(for impl in "${ARMS[@]}"; do for suite in "${ARMS[@]}"; do printf '%s|%s|%s ' "$impl" "$suite" "${R["$impl|$suite"]}"; done; done)
if PART=$(detect_partition 2>/dev/null); then
  echo
  echo "SPEC-AMBIGUOUS: the field partitions into self-consistent camps -- $PART"
  echo "Each camp passes within itself and fails across, which is neither a defect (1-of-N)"
  echo "nor an over-fitted suite (N-1-of-N). Two readings of the spec, both implemented"
  echo "correctly. Treat every cross-camp failure as vetoed and fix the specification."
fi
rm -f "$RJSON_TMP"

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
