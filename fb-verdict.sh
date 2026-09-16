# fb-verdict — THE verdict function. Sourced, never copied.
#
# Four tools independently reimplemented this and each reintroduced the same bugs:
#   - an absolute `lines > 30` threshold that graded on VOLUME, failing correct
#     implementations of small tasks (fixed in fb-score, then found still live in fb-dispatch)
#   - the empty-test pass: cargo reports "ok. 0 passed" when a filter matches nothing, which
#     is the first bug ever filed in this project and it reappeared verbatim in fb-verify
#
# The verdict decides what the bandit learns. It gets one implementation.   (bead farmerbob-9m7)
#
#   fb_verdict <build:pass|FAIL> <tests:pass|FAIL> <tests_executed> <lines_changed>
#   -> echoes one of: PASS NO-OP NO-COMPILE TESTS-FAIL NO-TESTS FAIL
fb_verdict() {
  local build="${1:-FAIL}" tests="${2:-FAIL}" executed="${3:-0}" lines="${4:-0}"
  executed=${executed:-0}; lines=${lines:-0}
  if   [ "$lines"    -eq 0        ]; then echo NO-OP
  elif [ "$build"    != pass      ]; then echo NO-COMPILE
  elif [ "$tests"    != pass      ]; then echo TESTS-FAIL
  elif [ "$executed" -eq 0        ]; then echo NO-TESTS
  else                                    echo PASS
  fi
}
