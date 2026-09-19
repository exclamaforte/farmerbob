#!/usr/bin/env bash
# fb-defects — measure each frozen test suite's SENSITIVITY against a validated defect set.
#
# Cross-examination found no behavioural disagreement among 13 implementations, and every
# candidate passes conformance. Neither measures whether a suite can CATCH A BUG, because
# nothing in the corpus was buggy. This injects known defects and counts detections.
#
# A defect counts only if the reference conformance suite detects it -- otherwise the
# mutation is inert or the spec is silent, and it belongs outside the denominator.
#
#   fb-defects.sh <bead> <crate> <target-rel>
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target}"
REPO=/home/gabe/Documents/farmerbob
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
DEFECTS="$REPO/.fb/defects/$BEAD"
OUT="$HOME/.local/share/farmerbob/logs/$BEAD.defects.json"
SLOTS=${FB_CX_SLOTS:-4}

# As of this commit the measurement lives in Rust -- `fb defects` -- and this script DELEGATES.
#
# Verified by differential before switching over, on the live `budget` defect set (8 patches,
# 4 candidate suites), both run FRESH within minutes of each other:
#   stdout  IDENTICAL, line for line, including the per-defect classification lines
#   JSON    BYTE-IDENTICAL, key order included
#
# Key order mattered here and was not a nicety. serde_json's map sorts its keys; Python's
# json.dump preserves insertion order. The port noticed and wrote its own emitter to hold the
# script's order. A rival arm filed a claim that it had NOT, and the byte-level diff is what
# settled it -- a semantic (sort_keys) diff would have passed either way and told me nothing.
#
# Two things this differential caught that no other gate could:
#   - arm discovery. An earlier Rust run found 3 candidate suites where the shell finds 4.
#   - a rival submission never invoked cargo at all. It emitted this script's exact wire
#     format with the numbers hardcoded to 0, and passed build, scope, clippy and its own
#     tests. (farmerbob-h70d)
#
# "Builds and its unit tests pass" is not evidence a port does its job. A port has an oracle
# nothing else in this project has -- the script it replaces -- and it gets RUN and DIFFED.
#
# Falls back to the shell below when the binary is not built.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
if [ -x "$FB_BIN" ]; then
  exec "$FB_BIN" defects "$BEAD" --crate "$CRATE" "$TARGET"
fi
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# reference = the merged implementation on master
REF="$TMP/ref"; mkdir -p "$REF"
(cd "$REPO" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$REF" && tar xf -)

apply() {  # apply <patchfile> <destdir>
  python3 - "$1" "$2/$TARGET" <<'PY'
import sys
spec = open(sys.argv[1]).read()
path = sys.argv[2]
src = open(path).read()
for block in spec.split('@@@')[1:]:
    old, new = block.split('--->')
    old, new = old.strip('\n'), new.strip('\n')
    if old not in src:
        print("ANCHOR-MISSING", file=sys.stderr); sys.exit(2)
    src = src.replace(old, new, 1)
open(path, 'w').write(src)
PY
}

run_suite() {  # run_suite <dir> <suitefile> -> pass|fail|nocompile
  local d="$1" suite="$2"
  local sd="$d/crates/$CRATE/src"
  # Graft into the TARGET MODULE, not lib.rs.
  #
  # A conformance suite is written as a child of the module it tests, so it says
  # `use super::Admission`. Appended to lib.rs, `super::` resolves to the CRATE ROOT and
  # every import fails: "unresolved imports super::Admission, super::Machine, super::Policy".
  # So every defect came back `nocompile`, every defect was EXCLUDED as inert, and the run
  # ended "0 validated defects, nothing to measure" -- for any task whose deliverable is a
  # module rather than lib.rs itself, which is all but a handful.
  #
  # That is why DefectSensitivity, the adjudicator's only DIRECT measure of suite quality and
  # its third criterion, has been available for one task in forty-eight. Not because nobody
  # wrote defect sets: because the validation step could not compile the suite it validates
  # against.
  #
  # fb-crossx.sh already solved exactly this and carries the bead for it. One rule, two
  # implementations, and only one of them got the fix.  (beads farmerbob-74l, farmerbob-jd2.11)
  local host="$sd/$(basename "$TARGET")"
  [ -f "$host" ] || host="$sd/lib.rs"
  cp "$host" "$host.bak"
  # Rename the grafted module. The merged reference already contains this very suite -- it was
  # escalated into the file when the task was adjudicated -- so appending it verbatim is
  # "the name `cx_budget_codex_luna` is defined multiple times", which reads as nocompile and
  # excludes the defect exactly like the import failure did. fb-crossx renames to xtests_N for
  # the same reason.
  sed 's/^\( *\)mod \([a-z_0-9]*\)/\1mod dfx_\2/' "$suite" >> "$host"
  local o="$d/out"
  local r
  if (cd "$d" && timeout 300 cargo test -p "$CRATE" >"$o" 2>&1); then r=pass
  elif grep -qE '^error\[E[0-9]+\]:|could not compile' "$o"; then r=nocompile
  else r=fail; fi
  mv "$host.bak" "$host"
  echo "$r"
}

# ---- 1. validate each defect against the reference conformance suite -------------------
echo "validating defects against the reference implementation"
VALID=(); MISSED=()
for p in "$DEFECTS"/*.patch; do
  [ -f "$p" ] || continue
  name=$(basename "$p" .patch)
  d="$TMP/v.$name"; rm -rf "$d"; cp -r "$REF" "$d"
  if ! apply "$p" "$d" 2>/dev/null; then printf '  %-28s SKIP (anchor missing)\n' "$name"; continue; fi
  r=$(run_suite "$d" "$REPO/.fb/conformance/$BEAD.rs")
  # THREE outcomes, not two.
  #
  #   fail       the reference suite detects it -> a valid, spec-relevant defect
  #   pass       the reference suite MISSES it -> either semantically inert, or a real defect
  #              the reference is blind to. These were discarded, and they are the only
  #              mutants that can DISCRIMINATE between candidate suites.
  #   nocompile  a syntax error, not a defect
  #
  # Keeping only `fail` selects for defects every competent suite catches, and guarantees the
  # measure reads ~100% for everyone. Measured: on `budget` all four suites caught 4 of 4, on
  # `gate` all four caught 12 of 12. The filter was removing exactly the evidence that would
  # have separated them -- 2 of gate's 24 mutants were excluded this way.
  #
  # A mutant the reference misses but SOME candidate catches proves that candidate's suite is
  # strictly better than the reference. That is the most informative result available here, so
  # it is now measured instead of thrown away.  (bead farmerbob-jd2.11)
  case "$r" in
    fail) printf '  %-28s valid (conformance detects it)\n' "$name"; VALID+=("$name") ;;
    pass) printf '  %-28s BEYOND the reference (it misses this one)\n' "$name"; MISSED+=("$name") ;;
    *)    printf '  %-28s excluded (%s) -- not a defect\n' "$name" "$r" ;;
  esac
  rm -rf "$d"
done
echo "  ${#VALID[@]} validated defects, ${#MISSED[@]} beyond the reference"
[ "${#VALID[@]}" -eq 0 ] && [ "${#MISSED[@]}" -eq 0 ] && { echo "nothing to measure"; exit 1; }

# ---- 2. every candidate suite against every validated defect ---------------------------
ARMS=()
for W in "$WT_ROOT/$BEAD--"*; do
  [ -d "$W" ] || continue; a="${W##*/$BEAD--}"
  [ -f "$W/.fb-task.md" ] && continue
  [ -s "$W/$TARGET" ] || continue
  # freeze the candidate's own suite
  awk '/#\[cfg\(test\)\]/{f=1} f' "$W/$TARGET" \
    | sed -e 's/^\( *\)mod tests/\1mod frozen/' \
          -e "s|use super::\*;|use crate::$(basename "$TARGET" .rs)::*;|" > "$TMP/s.$a.rs"
  [ "$(grep -c '#\[test\]' "$TMP/s.$a.rs")" -gt 0 ] && ARMS+=("$a")
done
echo "  ${#ARMS[@]} candidate suites"

for a in "${ARMS[@]}"; do
  for name in "${VALID[@]}" "${MISSED[@]}"; do
    [ -n "$name" ] || continue
    while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ]; do sleep 1; done
    (
      d="$TMP/r.$a.$name"; rm -rf "$d"; cp -r "$REF" "$d"
      apply "$DEFECTS/$name.patch" "$d" 2>/dev/null
      echo "$(run_suite "$d" "$TMP/s.$a.rs")" > "$TMP/o.$a.$name"
      rm -rf "$d"
    ) &
  done
done
wait

python3 - "$OUT" "$TMP" "${#VALID[@]}" "${ARMS[*]}" "${VALID[*]}" "${MISSED[*]}" <<'PY'
import json, os, sys
out, tmp, nvalid, arms, valid = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4].split(), sys.argv[5].split()
beyond = sys.argv[6].split() if len(sys.argv) > 6 else []

def verdict(a, n):
    p = os.path.join(tmp, f"o.{a}.{n}")
    return open(p).read().strip() if os.path.exists(p) else "nocompile"
res = {}
for a in arms:
    caught = comparable = 0
    missed = []
    for n in valid:
        p = os.path.join(tmp, f"o.{a}.{n}")
        v = open(p).read().strip() if os.path.exists(p) else "nocompile"
        if v == "nocompile":
            continue
        comparable += 1
        if v == "fail":
            caught += 1
        else:
            missed.append(n)
    # Defects the REFERENCE suite misses. Catching one proves this suite is strictly
    # better than the reference, and it is the only figure here that can separate a field
    # in which everyone catches the obvious defects.
    beyond_caught = sum(1 for n in beyond if verdict(a, n) == "fail")
    beyond_comparable = sum(1 for n in beyond if verdict(a, n) != "nocompile")
    res[a] = {"caught": caught, "comparable": comparable,
              "sensitivity": (caught / comparable) if comparable else None,
              "beyond_reference_caught": beyond_caught,
              "beyond_reference_of": beyond_comparable,
              "missed": missed}
json.dump({"validated_defects": nvalid, "beyond_reference_defects": len(beyond),
           "arms": res}, open(out, "w"), indent=1)
print()
print(f"{'SUITE':<24}{'CAUGHT':>9}{'OF':>5}{'SENSITIVITY':>13}{'BEYOND REF':>12}")
for a, v in sorted(res.items(),
                   key=lambda kv: (-kv[1]['beyond_reference_caught'], -(kv[1]['sensitivity'] or -1))):
    s = f"{v['sensitivity']:.0%}" if v['sensitivity'] is not None else "n/a"
    b = f"{v['beyond_reference_caught']}/{v['beyond_reference_of']}" if v['beyond_reference_of'] else "-"
    print(f"{a:<24}{v['caught']:>9}{v['comparable']:>5}{s:>13}{b:>12}")
if beyond:
    print()
    print("  BEYOND REF counts defects the reference conformance suite does NOT detect.")
    print("  Catching one proves a suite is strictly better than the reference; it is the")
    print("  only column here that can separate a field where everyone catches the rest.")
PY
echo "-> $OUT"
