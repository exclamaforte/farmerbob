#!/usr/bin/env bash
# fb-critique — cross-review. Each implementer critiques ANOTHER implementer's patch.
#
# The critic is the SAME ARM that implemented this task, run in ITS OWN WORKTREE. Two reasons:
# it already knows the spec and the repository, so the review is cheaper and better grounded;
# and running in its own tree keeps the provider-side context warm rather than paying to
# re-establish it.
#
# Assignment is a derangement -- nobody reviews themselves -- and the patch is frozen, so
# every finding refers to the artefact actually being selected.
#
# The critique is SUBJECTIVE ONLY. Everything measurable is already in the objective table
# (fb-objective.sh); a claim about behaviour is a hypothesis that gets executed, not a finding.
#   (beads farmerbob-ops, farmerbob-ljq)
#
#   fb-critique.sh <bead> <crate> <target-rel>
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target}"
REPO=/home/gabe/Documents/farmerbob
WT="$HOME/.local/share/farmerbob/worktrees"
LOGS="$HOME/.local/share/farmerbob/logs"
SLOTS=${FB_SLOTS:-6}
. "$REPO/fb-eligible.sh"
. "$REPO/fb-launch.sh"

# candidates that actually produced an implementation
ARMS=()
for d in "$WT/$BEAD--"*; do
  [ -d "$d" ] || continue; a="${d##*/$BEAD--}"
  [ -f "$d/.fb-task.md" ] && continue
  [ -s "$d/$TARGET" ] || continue
  fb_eligible "$a" 2>/dev/null || continue
  ARMS+=("$a")
done
N=${#ARMS[@]}
echo "$N candidates with an implementation"
[ "$N" -lt 2 ] && { echo "need >= 2 to cross-review"; exit 1; }

# derangement: arm i reviews arm i+1, wrapping. Nobody reviews themselves.
declare -A REVIEWS
for i in "${!ARMS[@]}"; do
  REVIEWS["${ARMS[$i]}"]="${ARMS[$(( (i + 1) % N ))]}"
done

mkdir -p "$LOGS/critiques/$BEAD"
for critic in "${ARMS[@]}"; do
  subject="${REVIEWS[$critic]}"
  while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ]; do sleep 2; done
  (
    cw="$WT/$BEAD--$critic"                 # critic works in its OWN worktree: warm context
    sw="$WT/$BEAD--$subject"
    # Show the DELIVERABLE, not "whatever git happens to call a change".
    #
    # This used to be `git diff HEAD -- crates/$CRATE`, with a fallback to reading $TARGET
    # if that came back empty. For a `creates` task the deliverable is UNTRACKED, so the
    # diff never contained it -- what it contained was the single tracked line the arm added
    # to lib.rs:
    #
    #     +pub mod matrix;
    #
    # The fallback existed for exactly this case and never fired, because it tested for an
    # EMPTY patch and one worthless line is not empty. The guard was defeated by the very
    # line that made the patch useless. On `matrix` this produced four reviews of which
    # three were worthless and two were fabricated: one critic invented line numbers for a
    # file it had never seen, another asserted a defect that the code plainly does not have.
    # Only glm-53-flash produced anything real, and only because it ignored the prompt and
    # read the file from disk itself.  (bead farmerbob-4ur)
    #
    # Rule: the patch is the declared target. If the target is tracked, its diff; if it is
    # untracked, its contents. Anything else the arm changed is reported separately as a
    # scope violation, because that is what it is.
    target_diff=$(cd "$sw" && git diff HEAD -- "$TARGET" 2>/dev/null)
    if [ -n "$target_diff" ]; then
      patch="$target_diff"
    elif [ -f "$sw/$TARGET" ]; then
      patch="=== NEW FILE: $TARGET ($(wc -l < "$sw/$TARGET") lines) ===
$(cat "$sw/$TARGET")"
    else
      patch=""
    fi

    # Files the arm changed that are NOT its deliverable. The critic should know: a review
    # of a one-file task by an arm that rewrote thirty-six files is reviewing the wrong
    # thing, and the scope violation is itself the finding.  (bead farmerbob-jxp)
    # Declaring the new module in its own crate's lib.rs is REQUIRED for a creates-task,
    # not a scope violation. Excluding it keeps the flag meaningful: every clean arm would
    # otherwise be reported as having strayed, and a warning that fires on everyone is
    # read by no one.
    own_lib="$(dirname "$TARGET")/lib.rs"
    # git may refuse the worktree entirely -- 103 of them have lost their admin directory
    # (farmerbob-13p). An empty `outside` would then read as "this arm stayed in scope",
    # which is a claim nobody measured. Say which it is.
    scope_readable=yes
    (cd "$sw" && git rev-parse --git-dir >/dev/null 2>&1) || scope_readable=no
    outside=$( { cd "$sw" && git diff --name-only HEAD -- crates/ 2>/dev/null
                 cd "$sw" && git ls-files --others --exclude-standard crates/ 2>/dev/null; } \
               | grep -vxF "$TARGET" | grep -vxF "$own_lib" | sort -u )
    # A deliverable may legitimately span several files. fnd-store's spec says "split into
    # modules if you like", and two arms did -- so the declared target became a 12-line list
    # of `mod` declarations and every substantive behaviour sat in files the critic never saw.
    # Two critics diagnosed it themselves rather than reviewing the header:
    #   or-deepseek-v4-flash: "every substantive behaviour lives in modules that are NOT part
    #     of the shown patch ... the reviewable artefact is a header."
    #   glm-53-flash: "if it is this candidate's output, the frozen patch has lost virtually
    #     all of its work and a patch-level comparison is invalid."
    # Showing the target alone was the fix for farmerbob-4ur; this is that fix's own next
    # failure. Carry the sibling sources too, bounded, so a split implementation is reviewable.
    #   (bead farmerbob-4ur, second occurrence)
    if [ "$scope_readable" = yes ] && [ -n "$outside" ]; then
      tdir=$(dirname "$TARGET")
      sibs=$(printf '%s\n' "$outside" | grep "^$tdir/" | grep '\.rs$' | head -12)
      for sib in $sibs; do
        [ -f "$sw/$sib" ] || continue
        patch="$patch

=== ALSO PART OF THIS DELIVERABLE: $sib ($(wc -l < "$sw/$sib") lines) ===
$(head -c 20000 "$sw/$sib")"
      done
    fi

    if [ "$scope_readable" = no ]; then
      patch="$patch

=== Scope could not be checked: git cannot read this worktree, so whether the arm stayed
=== within $TARGET is UNKNOWN, not confirmed."
    elif [ -n "$outside" ]; then
      patch="$patch

=== THIS ARM ALSO CHANGED $(printf '%s' "$outside" | grep -c .) FILE(S) OUTSIDE ITS DECLARED
=== DELIVERABLE ($TARGET). The task asked for that file only. The changes below are
=== out of scope and are shown as names, not content:
$outside"
    fi

    # An instrument that cannot perform its check must say so rather than produce a value.
    # A critique written against no patch is not a weak critique, it is a fabricated one,
    # and it costs a cycle and pollutes the critic's precision record.
    if [ -z "$patch" ]; then
      echo "  $critic: REFUSING -- $subject has no readable deliverable at $TARGET" >&2
      exit 3
    fi
    handoff=$(cat "$sw/.fb/handoff.md" 2>/dev/null || echo "(no handoff written)")

    p="$LOGS/critiques/$BEAD/$critic.prompt.md"
    python3 - "$REPO/.fb/prompts/_critique.md" "$p" <<PY
import sys
tpl = open(sys.argv[1]).read()
patch = """$(printf '%s' "$patch" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 60000)"""
handoff = """$(printf '%s' "$handoff" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 4000)"""
open(sys.argv[2], "w").write(tpl.replace("{PATCH}", patch).replace("{HANDOFF}", handoff).replace("{OUT}", "$cw/.fb/critique.md"))
PY
    rm -f "$cw/.fb/critique.md"; mkdir -p "$cw/.fb"
    P="$(cat "$p")"
    # CONTINUE the critic's own implementation session. The critic is the same arm that
    # implemented this task, running in its own worktree -- so it has just spent a full run
    # against this spec, and starting cold made it re-read from scratch what it already knew.
    # The comment above claimed the worktree kept context warm; it kept the DIRECTORY warm and
    # the session was new every time. Continuation is best effort: if no session survives, the
    # launcher starts fresh and the prompt is self-contained.
    ( fb_launch "$critic" "$P" "$cw" continue ) > "$LOGS/critiques/$BEAD/$critic.log" 2>&1
    if [ -s "$cw/.fb/critique.md" ]; then
      cp "$cw/.fb/critique.md" "$LOGS/critiques/$BEAD/$critic.on.$subject.md"
      printf '  %-22s reviewed %-22s %s claims, %s words\n' "$critic" "$subject" \
        "$(grep -c '^CLAIM:' "$cw/.fb/critique.md" 2>/dev/null)" "$(wc -w < "$cw/.fb/critique.md")"
    else
      printf '  %-22s reviewed %-22s (no critique written)\n' "$critic" "$subject"
    fi
  ) &
done
wait
echo "-> $LOGS/critiques/$BEAD/"
