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

# As of this commit the matrix lives in Rust -- `fb crossx` -- and this script DELEGATES.
# Verified on `consensus` BEFORE switching over: identical arms, identical cells, identical
# survival/discovery/suite classification, field for field, and 20 seconds against the
# shell's own runtime.
#
# That verification is not ceremony. The merged port passed the worktree score, built clean
# on merge and ran 18 of 18 unit tests green -- and then waited the full per-cell timeout on
# every cell, 80 minutes of sleeping to do 20 seconds of work, because its timeout killer
# could not be cancelled. Running it on real input and diffing is the only check that caught
# it.  (bead farmerbob-jd2.13)
#
# Falls back to the shell below when the binary is not built.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
if [ -x "$FB_BIN" ]; then
  exec "$FB_BIN" crossx "$BEAD" --crate "$CRATE" "$FILE"
fi

# NO SHELL FALLBACK. Everything below this point used to be a second, pre-port
# implementation of `fb crossx`, and it was DEAD: the exec above always fired. Three
# times this week a bug was chased into that dead code before the exec was noticed.
# If the binary is missing, say so and stop -- a silent second implementation is how
# two answers to one question come to disagree.
echo "fb binary not built at $FB_BIN -- run: cargo build -p fb" >&2
exit 127
