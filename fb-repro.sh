#!/usr/bin/env bash
# fb-repro — reproduce one cross-examination cell and print why it failed.
#
#   fb-repro.sh <task> <impl-arm> <suite-arm>
#
# Grafts <suite-arm>'s tests onto <impl-arm>'s implementation and runs them, which is the
# question every crossx FAIL raises: what did that suite actually catch?
#
# The tree is built in a mktemp dir and REMOVED ON EXIT, including on Ctrl-C. That is the
# entire reason this file exists: /tmp is a tmpfs here, so a leftover tree is RAM, and about
# twenty hand-run copies of this same pipeline accumulated 4.6GB -- starving the machine that
# fb-admit was carefully budgeting, with nothing in its arithmetic able to see why.
set -uo pipefail
. /home/gabe/Documents/farmerbob/fb-target.sh
export PATH="$HOME/.cargo/bin:$PATH"
cd /home/gabe/Documents/farmerbob

T="${1:?task}"; IMPL="${2:?impl arm}"; SUITE="${3:?suite arm}"
WT="$HOME/.local/share/farmerbob/worktrees"
REL=$(fb_target "$T")
[ -n "$REL" ] || { fb_target_why "$T"; exit 2; }
CRATE=$(awk -F/ '{print $2}' <<< "$REL")

for a in "$IMPL" "$SUITE"; do
  [ -s "$WT/$T--$a/$REL" ] || { echo "$T--$a has no $REL"; exit 2; }
done

D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT INT TERM
(cd "$WT/$T--$IMPL" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) \
  | (cd "$D" && tar xf -) 2>/dev/null

# strip the implementation's own tests, so only the grafted suite runs
for f in "$D/$(dirname "$REL")"/*.rs; do
  [ -f "$f" ] || continue
  awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" > "$f.s" && mv "$f.s" "$f"
done

# carry the suite file's own top-level imports: a test module inherits them, and without
# them the graft fails to compile for reasons that have nothing to do with the candidate
SRC="$WT/$T--$SUITE/$REL"
USES=$(awk '/#\[cfg\(test\)\]/{exit} /^use /{print}' "$SRC")
awk '/#\[cfg\(test\)\]/{f=1} f' "$SRC" \
  | sed -e 's/^\( *\)mod tests/\1mod repro/' \
  | awk -v uses="$USES" '{print} /^ *mod repro *\{/ && !d {print uses; d=1}' >> "$D/$REL"

echo "== $SUITE's suite against $IMPL's $REL"
cargo test --manifest-path "$D/Cargo.toml" -p "$CRATE" 2>&1 \
  | grep -E '^error|^---- |panicked at|assertion|left:|right:|test result:' | head -20
