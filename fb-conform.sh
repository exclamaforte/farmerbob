#!/usr/bin/env bash
# fb-conform — run farmerbob's OWN acceptance tests against every candidate.
# The agent's tests prove self-consistency; these prove conformance to the spec.
# Runs on a COPY so the candidate worktree is never mutated (bead farmerbob-qe3).
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:-farmerbob-core}"; SUITE="${3:?suite.rs}"
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
printf '%-24s %-9s %s\n' ARM RESULT FAILURES
for WT in "$WT_ROOT/$BEAD--"*; do
  [ -d "$WT" ] || continue
  SRC="${WT##*/$BEAD--}"
  [ -f "$WT/.fb-task.md" ] && { printf '%-24s %-9s\n' "$SRC" LIVE; continue; }
  C="$TMP/$SRC"; mkdir -p "$C"
  # copy only sources; never the agent's target/ or state
  (cd "$WT" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$C" && tar xf -) 2>/dev/null
  SRCDIR="$C/crates/$CRATE/src"
  [ -d "$SRCDIR" ] || { printf '%-24s %-9s\n' "$SRC" NO-CRATE; continue; }
  cat "$SUITE" >> "$SRCDIR/lib.rs"
  OUT="$TMP/$SRC.log"
  if (cd "$C" && cargo test -p "$CRATE" fb_conformance --offline >"$OUT" 2>&1 || cargo test -p "$CRATE" fb_conformance >"$OUT" 2>&1); then
    N=$(grep -oE '[0-9]+ passed' "$OUT"|head -1|awk '{print $1}')
    printf '%-24s %-9s %s/7 passed\n' "$SRC" "CONFORM" "${N:-?}"
  else
    if grep -q '^error\[E0' "$OUT" || grep -q 'cannot find' "$OUT"; then
      W=$(grep -oE 'cannot find [a-z]+ `[A-Za-z_]+`|no method named `[a-z_]+`|no function or associated item named `[a-z_]+`' "$OUT"|sort -u|head -2|tr '\n' ';')
      printf '%-24s %-9s API-MISMATCH %s\n' "$SRC" "FAIL" "$W"
    else
      F=$(grep -E '^test fb_conformance::.* FAILED' "$OUT"|sed 's/test fb_conformance:://;s/ ... FAILED//'|tr '\n' ' ')
      N=$(grep -oE '[0-9]+ passed' "$OUT"|head -1|awk '{print $1}')
      printf '%-24s %-9s %s/7 passed | failed: %s\n' "$SRC" "FAIL" "${N:-0}" "${F:-?}"
    fi
  fi
done
