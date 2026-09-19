#!/usr/bin/env bash
# fb-archive — capture every live worktree's work as a patch, before anything reaps it.
#
# WHY THIS EXISTS. A candidate's work lives ONLY in its worktree, uncommitted. Nothing in
# this harness ever committed it, so `git worktree remove --force` -- which fb-dispatch runs
# at the start of every run, which fb-speccheck runs, and which an adjudicator runs by hand
# to clean a field -- destroys it with no trace and no warning.
#
# It happened: score-delta/codex-luna wrote 272 lines against its declared target, scored
# OUT-OF-SCOPE on seventeen cargo-fmt departures, and the adjudicator reaped the worktree to
# requeue the task. The branch fb/score-delta/codex-luna survived and pointed at the BASE
# commit, because nothing had ever been committed to it. The work was simply gone, and the
# only reason anyone knew what it had contained was its handoff.
#
# A measurement harness that cannot reproduce the artefact it measured is not a measurement
# harness. This captures the artefact.
#
#   fb-archive.sh [worktree-name ...]     default: every worktree on disk
set -uo pipefail
# Derived, not hardcoded. Twenty-five scripts in this harness write the literal
# /home/gabe/Documents/farmerbob, which is why farmerbob can only be pointed at itself
# (bead: target-repo). This one does not add to that count: FB_REPO wins, otherwise the
# git repository this script lives in.
REPO="${FB_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && git rev-parse --show-toplevel)}"
[ -n "$REPO" ] || { echo "fb-archive: cannot locate the repository; set FB_REPO" >&2; exit 1; }
FB_HOME="${FB_HOME:-$HOME/.local/share/farmerbob}"
WT_ROOT="${FB_WT_ROOT:-$FB_HOME/worktrees}"
ARCHIVE="${FB_ARCHIVE:-$FB_HOME/archive}"
mkdir -p "$ARCHIVE"

archive_one() {
  local name="$1" wt="$WT_ROOT/$1" out="$ARCHIVE/$1.patch" meta="$ARCHIVE/$1.json"
  [ -d "$wt" ] || { echo "  $name: gone"; return 1; }
  # `git -C <wt> diff HEAD` covers tracked changes. An arm that CREATES its target leaves it
  # untracked, and a plain diff would report nothing at all -- the exact case where the work
  # is most valuable and least recoverable. Add it to the index first, in the worktree's own
  # index, which is per-worktree and therefore harmless to this repository's.
  git -C "$wt" add -AN . >/dev/null 2>&1
  # Exclude what the HARNESS removed, not what the candidate wrote. fb-dispatch deletes
  # CLAUDE.md, AGENTS.md, .beads/, .cursor/, .codex/, .agents/ and strips the hidden suites
  # out of .fb/ in every worktree, so an unfiltered diff is ~25k lines of the harness talking
  # to itself -- 413MB across 457 worktrees on the first run, and the candidate's own file
  # buried somewhere inside it. .fb/handoff.md is kept: the candidate wrote that.
  git -C "$wt" diff HEAD --binary -- . \
      ':(exclude).agents' ':(exclude).beads' ':(exclude).cursor' ':(exclude).codex' \
      ':(exclude)CLAUDE.md' ':(exclude)AGENTS.md' \
      ':(exclude,glob).fb/**' ':(glob).fb/handoff.md' > "$out" 2>/dev/null
  if [ ! -s "$out" ]; then
    rm -f "$out"
    echo "  $name: nothing to archive (no-op run)"
    return 0
  fi
  local base lines
  base=$(git -C "$wt" rev-parse HEAD 2>/dev/null)
  lines=$(grep -c '' "$out")
  printf '{"worktree":"%s","base":"%s","patch_lines":%s,"archived":"%s"}\n' \
    "$name" "$base" "$lines" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$meta"
  echo "  $name: $lines lines -> $out"
}

if [ "$#" -gt 0 ]; then
  for n in "$@"; do archive_one "$n"; done
else
  n=0
  for d in "$WT_ROOT"/*/; do
    [ -d "$d" ] || continue
    archive_one "$(basename "$d")" && n=$((n+1))
  done
  echo "archived from $n worktree(s) -> $ARCHIVE"
fi
