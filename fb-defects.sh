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
  cp "$sd/lib.rs" "$sd/lib.rs.bak"
  cat "$suite" >> "$sd/lib.rs"
  local o="$d/out"
  local r
  if (cd "$d" && timeout 300 cargo test -p "$CRATE" >"$o" 2>&1); then r=pass
  elif grep -qE '^error\[E[0-9]+\]:|could not compile' "$o"; then r=nocompile
  else r=fail; fi
  mv "$sd/lib.rs.bak" "$sd/lib.rs"
  echo "$r"
}

# ---- 1. validate each defect against the reference conformance suite -------------------
echo "validating defects against the reference implementation"
VALID=()
for p in "$DEFECTS"/*.patch; do
  [ -f "$p" ] || continue
  name=$(basename "$p" .patch)
  d="$TMP/v.$name"; rm -rf "$d"; cp -r "$REF" "$d"
  if ! apply "$p" "$d" 2>/dev/null; then printf '  %-28s SKIP (anchor missing)\n' "$name"; continue; fi
  r=$(run_suite "$d" "$REPO/.fb/conformance/$BEAD.rs")
  if [ "$r" = fail ]; then printf '  %-28s valid (conformance detects it)\n' "$name"; VALID+=("$name")
  else printf '  %-28s EXCLUDED (%s) -- inert, or the spec is silent\n' "$name" "$r"; fi
  rm -rf "$d"
done
echo "  ${#VALID[@]} validated defects"
[ "${#VALID[@]}" -eq 0 ] && { echo "nothing to measure"; exit 1; }

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
  for name in "${VALID[@]}"; do
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

python3 - "$OUT" "$TMP" "${#VALID[@]}" "${ARMS[*]}" "${VALID[*]}" <<'PY'
import json, os, sys
out, tmp, nvalid, arms, valid = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4].split(), sys.argv[5].split()
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
    res[a] = {"caught": caught, "comparable": comparable,
              "sensitivity": (caught / comparable) if comparable else None,
              "missed": missed}
json.dump({"validated_defects": nvalid, "arms": res}, open(out, "w"), indent=1)
print()
print(f"{'SUITE':<24}{'CAUGHT':>9}{'OF':>5}{'SENSITIVITY':>13}")
for a, v in sorted(res.items(), key=lambda kv: -(kv[1]['sensitivity'] or -1)):
    s = f"{v['sensitivity']:.0%}" if v['sensitivity'] is not None else "n/a"
    print(f"{a:<24}{v['caught']:>9}{v['comparable']:>5}{s:>13}")
PY
echo "-> $OUT"
