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
# As of this commit the rule lives in Rust -- farmerbob_core::gate -- and this function
# DELEGATES to it via `fb gate`. Five reimplementations of one rule was four too many, and
# the shell could not express the distinction that mattered: Option<u32> separates "zero
# tests ran" from "we did not measure", where bash has only an empty string.
#
# Falls back to the inline rule if the binary is not built, so the harness still works on a
# clean checkout -- but the fallback is the OLD rule and cannot return INDETERMINATE.
FB_BIN=/home/gabe/Documents/farmerbob/target/debug/fb

fb_verdict() {
  local build="${1:-FAIL}" tests="${2:-FAIL}" executed="${3:-0}" lines="${4:-0}"
  executed=${executed:-0}; lines=${lines:-0}

  if [ -x "$FB_BIN" ]; then
    local built=false passed=false
    [ "$build" = pass ] && built=true
    [ "$tests" = pass ] && passed=true
    case "$("$FB_BIN" gate --built "$built" --tests-passed "$passed" \
                      --tests-run "$executed" --lines-added "$lines" 2>/dev/null \
            | awk '{print $1}')" in
      Pass)          echo PASS ;;
      NoOp)          echo NO-OP ;;
      NoCompile)     echo NO-COMPILE ;;
      TestsFail)     echo TESTS-FAIL ;;
      NoTests)       echo NO-TESTS ;;
      WrongTarget)   echo WRONG-TARGET ;;
      Indeterminate) echo INDETERMINATE ;;
      *)             echo FAIL ;;
    esac
    return
  fi

  # fallback: the pre-Rust rule
  if   [ "$lines"    -eq 0        ]; then echo NO-OP
  elif [ "$build"    != pass      ]; then echo NO-COMPILE
  elif [ "$tests"    != pass      ]; then echo TESTS-FAIL
  elif [ "$executed" -eq 0        ]; then echo NO-TESTS
  else                                    echo PASS
  fi
}
