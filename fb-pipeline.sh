#!/usr/bin/env bash
# fb-pipeline — run EVERY stage for one task, in order, skipping what already exists.
#
# The wave path (fb-wave -> fb-admit -> fb-dispatch) implements Dispatch only. Score, then
# the subjective tier -- Critique, Promote, Prove -- used to be driven by `fb trial` and
# were silently lost when waves replaced it: fifteen tasks were adjudicated on objective
# metrics alone and nothing reported the absence.  (bead farmerbob-k9f)
#
# This is the missing tail. It is idempotent, so the autopilot can call it repeatedly.
#
#   fb-pipeline.sh <task> <crate> <target-rel>
set -uo pipefail
# cargo lives only in ~/.cargo/bin. fb-dispatch and fb-score both export this; fb-pipeline
# never did, and nothing noticed until `fb differential` became a stage and tried to build a
# candidate. Every arm came back "could not spawn: No such file or directory" -- which the
# gate correctly reported as NOT MEASURED and a gap rather than a pass, so the defect
# announced itself instead of silently passing. That is the whole reason for the
# unrunnable-is-not-Ok rule.
export PATH="$HOME/.cargo/bin:$PATH"
. /home/gabe/Documents/farmerbob/fb-target.sh
cd /home/gabe/Documents/farmerbob
T="${1:?task}"; CRATE="${2:-farmerbob-core}"; TARGET="${3:-}"
LOGS="$HOME/.local/share/farmerbob/logs"

# Infer the target from the spec's own declaration rather than making the caller repeat it.
# BOTH verbs, not just fb:creates. Specs declare `fb:modifies` when the deliverable is an
# edit to an existing file, and this reader knew only `fb:creates` -- so probe, fnd-store and
# gpu-lease were unpipelineable from the day they were written and sat in NEEDS CRITIQUE
# indefinitely, with the same message as a task whose spec is simply missing. One reader,
# two vocabularies.  (bead farmerbob-z1p)
[ -n "$TARGET" ] || TARGET=$(fb_target "$T")
[ -n "$TARGET" ] || { fb_target_why "$T"; exit 2; }
echo "== pipeline $T ($CRATE, $TARGET)"

# The set of gate-passing candidates, as a stable fingerprint. An artefact is valid only
# for the field it was computed over: reviewer was cross-examined 2x2 while five passing
# candidates sat in the worktrees, and the stale matrix reported a TIE between two arms
# neither of which was best. Staleness inverted the answer rather than delaying it.
# The set of gate-PASSING candidates. TOTAL: it always prints exactly one of three things,
# and never an empty string.
#
# It used to print nothing both when no candidate passed and when the task had never been
# scored, and run_stage's guard was `[ -n "$now" ] && ...`, so an empty field could never
# trigger a re-run. Every task whose whole field failed was therefore frozen at whatever it
# was first scored as -- gpu-lease sat at four stale NO-OPs while eight worktrees existed.
# Same failure as always: "nothing passed" and "nothing was measured" produced one value.
field() {
  python3 -c "
import json,sys
try: d=json.load(open('$LOGS/$T.score.json'))
except Exception: print('UNSCORED'); sys.exit(0)
p=sorted(r['source'] for r in d if r.get('verdict')=='PASS')
print(','.join(p) if p else 'NONE')" 2>/dev/null
}

# The set of CANDIDATES on disk. This is what the score stage must be keyed on.
#
# score.json is the root every other artefact derives from, and it was the only stage with
# no freshness check at all -- `run_stage` skipped it whenever the file merely existed. The
# fingerprint could not have caught it either, because the fingerprint is COMPUTED FROM
# score.json: a stale root yields a stale fingerprint that every downstream artefact then
# validates against successfully. The check certified staleness as freshness.
#   (bead farmerbob-oad)
wtfield() {
  ls -d "$HOME/.local/share/farmerbob/worktrees/$T--"*/ 2>/dev/null \
    | sed "s|.*/$T--||; s|/$||" | sort | paste -sd, - | sed 's/^$/NOCANDIDATES/'
}

run_stage() { # run_stage <name> <artefact> <keyfn> <command...>
  local name="$1" artefact="$2" keyfn="$3"; shift 3
  local now sig="$artefact.field"
  now=$($keyfn)
  if [ -s "$artefact" ]; then
    if [ "$now" != "$(cat "$sig" 2>/dev/null)" ]; then
      echo "  $name: field changed -> re-running"
      echo "     was: $(cat "$sig" 2>/dev/null || echo '<unrecorded>')"
      echo "     now: $now"
    else
      echo "  $name: already done"; return 0
    fi
  fi
  echo "  $name: running"
  # A STAGE IS DONE WHEN ITS ARTEFACT EXISTS, NOT WHEN ITS COMMAND EXITS 0.
  #
  # This used to record the signature on exit status alone. `promote` declared
  # $T.promoted.json and fb-promote.sh writes $T.claims.json, so the declared artefact was
  # never created -- and sixty-six tasks recorded "promote: ok" with nothing at the path the
  # pipeline names. The `[ -s "$artefact" ]` cache check above was therefore false every time,
  # so promote re-ran and re-verified every claim on every invocation, forever.
  #
  # Nobody noticed because "ok" was printed either way: a stage whose failure state is
  # indistinguishable from its success state, in the pipeline's own bookkeeping. Counting
  # artefacts found it; reading the logs never would have.
  #
  # The two failures are now reported apart, because they need different fixes: a command that
  # fails is a bug in the stage, and a command that succeeds without writing is a bug in what
  # the pipeline believes the stage produces.
  if ! "$@" >> "$LOGS/$T.pipeline.log" 2>&1; then
    echo "  $name: FAILED (see $LOGS/$T.pipeline.log)"; return 1
  fi
  if [ ! -s "$artefact" ]; then
    echo "  $name: RAN BUT PRODUCED NOTHING at $artefact"
    echo "     the command exited 0 and the artefact is missing or empty."
    echo "     Either the stage is broken or the pipeline names the wrong file."
    return 1
  fi
  echo "  $name: ok"; printf '%s' "$now" > "$sig"
}

# DIFFERENTIAL. The only stage here that is not a gate on form.
#
# Every other check asks whether a candidate builds, whether its own tests pass, whether it
# stayed inside its deliverable, whether it is lint-clean. A CONSTANT SATISFIES ALL OF THEM,
# and one was submitted: port-defects/or-inkling shipped 311 lines that never invoked cargo,
# emitted the script's exact wire format with the numbers hardcoded to zero, and passed the
# lot. It was caught by another model reading it, which is not a gate. (farmerbob-h70d)
#
# A port is the one task in this project with a real oracle -- the script it replaces -- and
# consulting it used to depend on my remembering to. Applies only to specs that declare
# <!-- fb:differential <script> <subcommand> --> plus at least one <!-- fb:case ARGS -->.
#
# Its exit codes are FOUR, and the two that look alike are the point:
#   0 all measured arms agree   1 an arm diverges -- a finding, and a successful measurement
#   2 declared but unrunnable -- A GAP        3 not a port -- nothing to check
# 0 and 1 both mean the instrument worked, so both record the fingerprint. 2 is the only one
# that is a stage failure, because it is the only one where something should have been
# checked and was not.
run_differential() {
  # Keyed on wtfield, not field. This stage measures EVERY worktree, including candidates
  # the gate failed -- an arm that does not build is exactly the arm most worth recording as
  # NOT MEASURED. Keying it on the PASSING set would have left it stale whenever a failing
  # candidate changed, and the passing set is a strict subset of what the artefact covers:
  # the first real run measured 5 arms under a 3-arm fingerprint.
  local artefact="$LOGS/$T.differential.json" sig now
  sig="$artefact.field"; now=$(wtfield)
  if [ -s "$artefact" ] && [ "$now" = "$(cat "$sig" 2>/dev/null)" ]; then
    echo "  differential: already done"; return 0
  fi
  ./target/debug/fb differential "$T" 2>&1 | sed 's/^/  /'
  case "${PIPESTATUS[0]}" in
    0) echo "  differential: all measured arms agree"; printf '%s' "$now" > "$sig" ;;
    1) echo "  differential: DIVERGENCE FOUND -- see above"; printf '%s' "$now" > "$sig" ;;
    3) echo "  differential: n/a, this task declares no oracle" ;;
    *) echo "  differential: DECLARED BUT COULD NOT RUN -- this is a gap, not a pass"; return 1 ;;
  esac
}

# score is keyed on the CANDIDATES ON DISK; everything downstream on who PASSED.
run_stage score    "$LOGS/$T.score.json"    wtfield bash ./fb-score.sh   "$T" "$CRATE"
run_differential
run_stage crossx   "$LOGS/$T.crossx.json"   field   bash ./fb-crossx.sh  "$T" "$CRATE" "$TARGET"
run_stage critique "$LOGS/$T.claims.json"   field   bash ./fb-critique.sh "$T" "$CRATE" "$TARGET"
run_stage promote  "$LOGS/$T.promoted.json" field   bash ./fb-promote.sh "$T" "$CRATE" "$TARGET"
run_stage prove    "$LOGS/$T.proved.json"   field   bash ./fb-prove.sh   "$T" "$CRATE" "$TARGET"

# ESCALATE. A confirmed finding should sharpen the gate for every future candidate on this
# task, not settle one adjudication and vanish. fb-prove already wrote the test and ran it
# against the merged reference; escalation keeps that artefact instead of discarding it, and
# credits the critic that found it. Idempotent, so it is safe on every pipeline run.
#   (bead farmerbob-mqr)
./fb-escalate.sh auto "$T" 2>&1 | sed 's/^/  escalate: /'

echo "== $T ready for adjudication"
