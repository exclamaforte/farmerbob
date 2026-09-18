#!/usr/bin/env bash
# fb-speclint — refuse to queue a spec that repeats a known spec-writing defect.
#
# Four separate task specs have shipped a boundary written as "state X and pin it", read by the
# implementer as a REQUIREMENT rather than as delegation. Each time the field split, each time a
# crossx matrix filled with noise, and once an arm scored survival=0.00 with no defect in its
# code at all. I filed a P1 about the phrasing and then wrote it three more times.
#
# A rule an author must remember at 2am is not a rule. This is the same lesson as the wave lock
# and the worktree claim: make it structural.
#
#   fb-speclint.sh <spec.md>...
set -uo pipefail
rc=0

for f in "$@"; do
  [ -f "$f" ] || { echo "$f: no such spec" >&2; rc=1; continue; }
  name=$(basename "$f")

  # 1. Delegation that does not read as delegation. The fix is to pin the answer, or to say
  #    DELEGATED and forbid asserting on it -- naming a boundary is not delegating it.
  while IFS=: read -r n line; do
    [ -n "$n" ] || continue
    echo "$name:$n: delegation-shaped instruction -- pin the answer, or say DELEGATED and add"
    echo "$name:$n:   'your tests may not assert on it'. Four specs have split a field on this."
    echo "$name:$n:   > $(printf '%s' "$line" | sed 's/^ *//' | cut -c1-90)"
    rc=1
  done < <(grep -nEi 'state (what|which|whether)[^.]*and pin it|pin (it|them) (yourself|explicitly for every)' "$f")

  # 2. A literal marker in prose. The dispatcher greps the whole prompt, so an EXAMPLE of a
  #    declaration is parsed as a declaration: wave60 refused all four of its own arms that way.
  n_markers=$(grep -cE '<!-- fb:(creates|modifies) [^ ]+ -->' "$f")
  if [ "$n_markers" -gt 1 ]; then
    echo "$name: $n_markers literal fb: markers; only line 1 may be one."
    echo "$name:   The dispatcher greps the whole prompt and will read the examples as declarations."
    rc=1
  fi
  if [ "$n_markers" -eq 0 ]; then
    echo "$name: no fb:creates/fb:modifies declaration -- the dispatcher cannot check preconditions."
    rc=1
  fi

  # 4. THE SPEC MUST CARRY THE CURRENT RUBRIC.
  #
  # Nothing reads .fb/prompts/_rubric.md. Every spec was carrying a FROZEN COPY of another
  # spec's tail, snapshotted days ago, so two rules added to _rubric.md this week -- the one
  # about suites running against other implementations, and the one about not quoting a field
  # the spec calls prose -- reached no arm at all. Both were added precisely because the defect
  # had recurred, and both recurred again afterwards.
  #
  # That is farmerbob-vg0 turned inside out: disclose what is scored, into a file the arms
  # never see. This check makes the disclosure verifiable instead of assumed.
  rubric=.fb/prompts/_rubric.md
  if [ -f "$rubric" ]; then
    while IFS= read -r heading; do
      [ -n "$heading" ] || continue
      if ! grep -qF "$heading" "$f"; then
        echo "$name: missing a current rubric section: $heading"
        echo "$name:   append .fb/prompts/_rubric.md, not a copy of another spec's tail."
        rc=1
      fi
    done < <(grep -E '^## ' "$rubric")
  fi

  # 3. A declared target that contradicts the base. Exactly farmerbob-m71, and the reason
  #    wave58 and wave60 each burned four slots in under fifteen seconds.
  verb=$(grep -ohE '<!-- fb:(creates|modifies) [^ ]+ -->' "$f" | head -1 | awk '{print $2}' | sed 's/^fb://')
  path=$(grep -ohE '<!-- fb:(creates|modifies) [^ ]+ -->' "$f" | head -1 | awk '{print $3}')
  if [ -n "$path" ]; then
    if [ "$verb" = creates ] && [ -e "$path" ]; then
      echo "$name: declares fb:creates $path but it EXISTS on the base -- every arm will refuse."; rc=1
    elif [ "$verb" = modifies ] && [ ! -e "$path" ]; then
      echo "$name: declares fb:modifies $path but it does NOT exist -- every arm will refuse."; rc=1
    fi
  fi
done

[ "$rc" -eq 0 ] && echo "speclint: clean"
exit $rc
