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
    patch=$(cd "$sw" && git diff HEAD -- "crates/$CRATE" 2>/dev/null)
    [ -z "$patch" ] && patch=$(sed -n '1,400p' "$sw/$TARGET" 2>/dev/null)
    handoff=$(cat "$sw/.fb/handoff.md" 2>/dev/null || echo "(no handoff written)")

    p="$LOGS/critiques/$BEAD/$critic.prompt.md"
    python3 - "$REPO/.fb/prompts/_critique.md" "$p" <<PY
import sys
tpl = open(sys.argv[1]).read()
patch = """$(printf '%s' "$patch" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 60000)"""
handoff = """$(printf '%s' "$handoff" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 4000)"""
open(sys.argv[2], "w").write(tpl.replace("{PATCH}", patch).replace("{HANDOFF}", handoff))
PY
    rm -f "$cw/.fb/critique.md"; mkdir -p "$cw/.fb"
    MODEL=$(python3 -c "
import tomllib;print(tomllib.load(open('$REPO/sources.toml','rb'))['source']['$critic'].get('model',''))")
    P="$(cat "$p")"
    (
      cd "$cw"
      case "$critic" in
        codex-luna)      codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.6-luna "$P" ;;
        gemini-38-flash) command agy -p "$P" --model gemini-3.8-flash-high --add-dir "$cw" \
                             --dangerously-skip-permissions --output-format text ;;
        glm-53-flash)    zcode --prompt "$P" ;;
        ifm-*)           set -a; . "$HOME/.config/farmerbob/secrets.env"; set +a
                         opencode run -m "$MODEL" "$P" ;;
        or-*)            ori opencode run -m "$MODEL" "$P" ;;
      esac
    ) > "$LOGS/critiques/$BEAD/$critic.log" 2>&1
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
