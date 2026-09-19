#!/usr/bin/env bash
# fb-score — score every candidate for a bead by enumerating WORKTREES, not run
# records. Survives a dispatch wrapper dying, which run-record-driven scoring does not.
#   fb-score.sh <bead> [crate]
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
. /home/gabe/Documents/farmerbob/fb-verdict.sh
BEAD="${1:?bead}"; CRATE="${2:-farmerbob-core}"
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
LOG_ROOT="$HOME/.local/share/farmerbob/logs"
OUT="$LOG_ROOT/$BEAD.score.json"

# As of this commit the measurement lives in Rust -- `fb score` -- and this script
# DELEGATES to it. Verified field-by-field against the shell on the 6-candidate `reviewer`
# task: identical verdict, build, test, tests_run, lines, new_files, crates_touched,
# clippy and duration on every arm.
#
# What the shell could not express, and the Rust does:
#   * liveness as a BELIEF with an authority (farmerbob_core::liveness) rather than a
#     boolean, so an orphaned marker is released by the process evidence instead of
#     outranking it forever;
#   * clippy and duration as Measurement, so "the crate does not compile so it has no lint
#     count" is a different fact from "zero warnings", and a missing run record is Missing
#     rather than the sentinel -1 that any averaging consumer would happily fold in;
#   * INDETERMINATE, which the shell's five-way if/elif chain had no branch for.
#
# Falls back to the inline shell below if the binary is not built, so the harness still
# works on a clean checkout -- but the fallback cannot return INDETERMINATE and reads the
# marker as liveness on its own.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
if [ -x "$FB_BIN" ]; then
  exec "$FB_BIN" score "$BEAD" --crate "$CRATE"
fi

# NO SHELL FALLBACK. Everything below this point used to be a second, pre-port
# implementation of `fb score`, and it was DEAD: the exec above always fired. Three
# times this week a bug was chased into that dead code before the exec was noticed.
# If the binary is missing, say so and stop -- a silent second implementation is how
# two answers to one question come to disagree.
echo "fb binary not built at $FB_BIN -- run: cargo build -p fb" >&2
exit 127
