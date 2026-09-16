#!/usr/bin/env bash
# fb-dispatch — the manual stand-in for farmerbob, used to bootstrap farmerbob.
# Runs one implementer in its own git worktree and records timing + exit status.
#
#   fb-dispatch.sh <source-id> <bead-id> <prompt-file>
#
# Reads sources.toml for the launcher. Everything it does by hand here is a
# thing farmerbob is meant to do properly: worktree provisioning, per-run
# confinement, structured result capture.
set -uo pipefail

SRC="${1:?source id}"; BEAD="${2:?bead id}"; PROMPT_FILE="${3:?prompt file}"
REPO="/home/gabe/Documents/farmerbob"
WT_ROOT="${FB_WT_ROOT:-$HOME/.local/share/farmerbob/worktrees}"
LOG_ROOT="${FB_LOG_ROOT:-$HOME/.local/share/farmerbob/logs}"
BASE="${FB_BASE:-HEAD}"
MEM_MAX="${FB_MEM_MAX:-6G}"
CPU_QUOTA="${FB_CPU_QUOTA:-400%}"

mkdir -p "$WT_ROOT" "$LOG_ROOT"
RUN="${BEAD}--${SRC}"
WT="$WT_ROOT/$RUN"
LOG="$LOG_ROOT/$RUN.log"
META="$LOG_ROOT/$RUN.json"
BRANCH="fb/$BEAD/$SRC"

# --- worktree -------------------------------------------------------------
git -C "$REPO" worktree remove --force "$WT" 2>/dev/null
git -C "$REPO" branch -D "$BRANCH" 2>/dev/null
git -C "$REPO" worktree add -q -b "$BRANCH" "$WT" "$BASE" || { echo "worktree failed"; exit 1; }

PROMPT="$(cat "$PROMPT_FILE")"

# --- launcher per source (from sources.toml) ------------------------------
launch() {
  case "$SRC" in
    codex-luna)
      codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.6-luna "$PROMPT" ;;
    gemini-38-flash)
      command agy -p "$PROMPT" --model gemini-3.8-flash-high --add-dir "$WT" \
              --dangerously-skip-permissions --output-format text ;;
    glm-53-flash)
      zcode --prompt "$PROMPT" ;;
    ifm-*)
      set -a; . "$HOME/.config/farmerbob/secrets.env"; set +a
      opencode run -m "$(fb_model)" "$PROMPT" ;;
    or-*)
      ori opencode run -m "$(fb_model)" "$PROMPT" ;;
    *) echo "unknown source $SRC"; return 127 ;;
  esac
}
fb_model() {
  python3 - "$SRC" <<'PY'
import sys,tomllib
s=tomllib.load(open('/home/gabe/Documents/farmerbob/sources.toml','rb'))['source']
print(s[sys.argv[1]]['model'])
PY
}

# --- run, confined --------------------------------------------------------
START=$(date +%s)
cd "$WT" || exit 1
systemd-run --user --scope --quiet --unit="fb-$RUN-$$" \
  -p MemoryMax="$MEM_MAX" -p CPUQuota="$CPU_QUOTA" -p TasksMax=2048 \
  -- bash -c "$(declare -f launch fb_model); cd '$WT'; SRC='$SRC'; WT='$WT'; PROMPT='$(printf '%s' "$PROMPT" | sed "s/'/'\\\\''/g")'; launch" \
  >"$LOG" 2>&1
RC=$?
END=$(date +%s)

# --- result ---------------------------------------------------------------
FILES=$(git -C "$WT" status --porcelain | wc -l)
DIFF=$(git -C "$WT" diff --stat HEAD 2>/dev/null | tail -1)
python3 - "$META" "$SRC" "$BEAD" "$RC" "$((END-START))" "$WT" "$BRANCH" "$FILES" "$DIFF" <<'PY'
import json,sys
p,src,bead,rc,dur,wt,br,files,diff=sys.argv[1:10]
json.dump({"source":src,"bead":bead,"rc":int(rc),"duration_s":int(dur),
           "worktree":wt,"branch":br,"files_changed":int(files),"diffstat":diff},
          open(p,'w'),indent=1)
PY
printf '%-22s rc=%-3s %4ss  files=%-3s %s\n' "$SRC" "$RC" "$((END-START))" "$FILES" "$DIFF"
