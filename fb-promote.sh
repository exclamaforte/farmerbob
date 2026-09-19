#!/usr/bin/env bash
# fb-promote — turn critics' CLAIMs into executed evidence.
#
# A claim is a hypothesis, not a finding. Each is classified before any test is written:
#
#   CONTRADICTED  two independent critics assert OPPOSITE expectations of the same behaviour
#                 -> the SPEC is underdetermined, not the code. Routes to task authoring.
#                    Neither arm is scored. This is SpecAmbiguous, and by the rule the router
#                    itself implements, it outranks a demonstrated defect.
#   CONFIRMATORY  EXPECT == ACTUAL: the critic documented correct behaviour rather than
#                 alleging a defect. Costs a cycle to confirm something that already passes.
#   TESTABLE      a genuine single-sided allegation -> generate a test, run it, record the
#                 outcome and credit or debit the critic.
#
#   fb-promote.sh <bead>
set -uo pipefail
BEAD="${1:?bead}"

# As of this commit the classification lives in Rust -- `fb promote` -- and this script
# DELEGATES to it. Verified field-for-field against this shell on port-crossx's real claims:
# 7 claims from 3 critics, identical critic/subject/claim/where/trigger/expect/actual/kind on
# every one, identical contradiction set.
#
# Ported because every repair that came back in this project came back in bash: three fixes
# each caused a second occurrence of the bug they fixed, and all three were shell. Bash cannot
# express the distinction the harness depends on most -- measured zero versus not measured --
# having only an empty string and a 0.
#
# Falls back to the inline Python below when the binary is not built, so a clean checkout
# still works.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb
if [ -x "$FB_BIN" ]; then
  exec "$FB_BIN" promote "$BEAD"
fi

# NO SHELL FALLBACK. Everything below this point used to be a second, pre-port
# implementation of `fb promote`, and it was DEAD: the exec above always fired. Three
# times this week a bug was chased into that dead code before the exec was noticed.
# If the binary is missing, say so and stop -- a silent second implementation is how
# two answers to one question come to disagree.
echo "fb binary not built at $FB_BIN -- run: cargo build -p fb" >&2
exit 127
