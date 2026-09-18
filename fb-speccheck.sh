#!/usr/bin/env bash
# fb-speccheck — have arms critique a SPEC before anyone implements it.
#
# Runs BEFORE dispatch. Each eligible arm reads the spec plus the code the task will touch and
# returns FINDINGS: defects in the specification, not in any implementation. The adjudicator
# rules on each, revises the spec, and only then dispatches the implementation wave.
#
# Why this exists: five consecutive merged tasks each carried a defect in their own spec, and
# every one surfaced only after three arms had implemented against it -- a pinned type with no
# home, a boundary that read as a requirement to half the field, a clause naming a field that
# does not exist, three rules with no legal move, and a frozen signature contradicting the
# behaviour change ordered in the same document. All five were visible in the text.
# (docs/PROPOSALS.md)
#
# Unlike fb-critique, the arms here have NO worktree of their own for the task -- nothing has
# been implemented. Each runs in a scratch directory containing only the prompt, so a spec
# critic cannot accidentally start the work.
#
#   fb-speccheck.sh <bead> <crate> [arm,arm,...]
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:?crate}"; WANT="${3:-}"
REPO=/home/gabe/Documents/farmerbob
LOGS="$HOME/.local/share/farmerbob/logs"
WT="$HOME/.local/share/farmerbob/worktrees"
SPEC="$REPO/.fb/prompts/$BEAD.md"
SLOTS=${FB_SLOTS:-4}
. "$REPO/fb-target.sh"
. "$REPO/fb-eligible.sh"
. "$REPO/fb-launch.sh"

[ -f "$SPEC" ] || { echo "no spec at $SPEC" >&2; exit 1; }

# Which arms. An explicit list wins; otherwise every eligible arm, which keeps this stage
# cheap to run on whatever is available rather than pinning a roster in a second place.
ARMS=()
if [ -n "$WANT" ]; then
  IFS=, read -r -a want <<< "$WANT"
  for a in "${want[@]}"; do fb_eligible "$a" 2>/dev/null && ARMS+=("$a"); done
else
  while read -r a; do fb_eligible "$a" 2>/dev/null && ARMS+=("$a"); done < <(
    python3 -c "
import tomllib
for k in sorted(tomllib.load(open('$REPO/sources.toml','rb'))['source']): print(k)")
fi
N=${#ARMS[@]}
echo "spec critique of $BEAD: $N arm(s)"
[ "$N" -lt 1 ] && { echo "no eligible arms" >&2; exit 1; }

# The code the task will touch. For a `modifies` task that is the target file; for a `creates`
# task the file does not exist yet, so show the module it will sit beside -- lib.rs plus any
# type the spec names by path. Showing nothing would make every "wrong-reference" finding a
# guess, which is the one kind of finding this stage must not encourage.
TARGET=$(fb_target "$BEAD" 2>/dev/null)
VERB=$(fb_target_verb "$BEAD" 2>/dev/null)
code=""
if [ -n "$TARGET" ] && [ -f "$REPO/$TARGET" ]; then
  code="=== $TARGET ($(wc -l < "$REPO/$TARGET") lines) ===
$(head -c 60000 "$REPO/$TARGET")"
else
  code="=== $TARGET does not exist yet (fb:${VERB:-creates}) ==="
fi
# Types the spec names as `crate::x::Y` or `x::Y` and that exist as modules: carry them, so a
# reference check is a check rather than a recollection.
for m in $(grep -ohE '\b(crate::)?[a-z_]+::[A-Z][A-Za-z]+' "$SPEC" \
           | sed -e 's/^crate:://' -e 's/::.*//' | sort -u | head -6); do
  f="$REPO/crates/$CRATE/src/$m.rs"
  [ -f "$f" ] || continue
  [ "crates/$CRATE/src/$m.rs" = "$TARGET" ] && continue
  # Carry the file's REAL TEXT, with only its test module stripped. The first version of this
  # filtered to `grep -E '^\s*(pub |impl |})'` to keep the prompt small. Enum variants are
  # indented and carry no `pub`, so every enum arrived as `pub enum Disposal {` followed by
  # `}` -- and the first arm to run this stage correctly reported that `Disposal` was
  # uninhabited and could not classify anything. Two of its five findings were that. The
  # filter's output was indistinguishable from a real empty enum, which is this project's
  # recurring bug wearing yet another hat, and it was introduced by the stage built to catch
  # exactly this class of defect.
  code="$code

=== crates/$CRATE/src/$m.rs, named by the spec ($(wc -l < "$f") lines, tests omitted) ===
$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "$f" | head -c 24000)"
done

mkdir -p "$LOGS/speccheck/$BEAD"
for arm in "${ARMS[@]}"; do
  while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ]; do sleep 2; done
  (
    # Run at the arm's FUTURE worktree path, so the implementation run can CONTINUE this
    # session instead of starting cold. All four launchers key continuation to the working
    # directory, so "same session" and "same directory" are the same requirement.
    #
    # The directory is removed afterwards and the session is not: verified rather than
    # assumed, by deleting the directory between two turns and confirming the second still
    # recalled the first. fb-dispatch does `git worktree add` at this path, which refuses a
    # non-empty directory, so leaving the contents behind would break dispatch while leaving
    # nothing behind still preserves continuity.
    #
    # It is a bare directory, NOT a checkout. Nothing has been implemented, and handing a
    # spec critic a working tree invites it to start the task instead of reviewing the
    # question.
    cw="$WT/$BEAD--$arm"
    rm -rf "$cw"; mkdir -p "$cw/.fb"
    p="$LOGS/speccheck/$BEAD/$arm.prompt.md"
    python3 - "$REPO/.fb/prompts/_speccheck.md" "$p" "$SPEC" <<PY
import sys
tpl  = open(sys.argv[1]).read()
spec = open(sys.argv[3]).read()[:60000]
code = """$(printf '%s' "$code" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 70000)"""
open(sys.argv[2], "w").write(
    tpl.replace("{SPEC}", spec).replace("{CODE}", code).replace("{OUT}", "$cw/.fb/speccheck.md"))
PY
    P="$(cat "$p")"
    ( fb_launch "$arm" "$P" "$cw" ) > "$LOGS/speccheck/$BEAD/$arm.log" 2>&1
    if [ -s "$cw/.fb/speccheck.md" ]; then
      cp "$cw/.fb/speccheck.md" "$LOGS/speccheck/$BEAD/$arm.findings.md"
      n=$(grep -c '^ *FINDING:' "$cw/.fb/speccheck.md" 2>/dev/null)
      if grep -q 'NO SPEC DEFECTS FOUND' "$cw/.fb/speccheck.md"; then
        printf '  %-22s no spec defects found\n' "$arm"
      else
        printf '  %-22s %s finding(s)\n' "$arm" "$n"
      fi
    else
      printf '  %-22s (no report written)\n' "$arm"
    fi
    # Remove the directory, keep the session. fb-dispatch will `git worktree add` here.
    rm -rf "$cw"
  ) &
done
wait
echo "-> $LOGS/speccheck/$BEAD/"
