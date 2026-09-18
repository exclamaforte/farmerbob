# fb-launch — THE launcher dispatch. Sourced, never copied.
#
# Reimplementing this is how fb-prove came to run `ori opencode run -m gpt-5.6-luna` for
# codex-luna, which is a CLI arm needing `codex exec`. Every arm produced zero tests and the
# failure looked like a capability result. Same structural lesson as fb-verdict: the logic
# that decides HOW to invoke an arm belongs in one place.
#
# SESSION CONTINUATION. Pass a non-empty 4th argument to CONTINUE this arm's previous session
# in this working directory instead of starting cold.
#
# Every stage used to start a fresh agent. fb-critique's own comment claimed running a critic
# in its own worktree "keeps the provider-side context warm rather than paying to re-establish
# it" -- but that was only true of the DIRECTORY. The session was new every time, so an arm
# critiquing a rival had to re-read a spec it had just spent twenty minutes implementing
# against, and an arm implementing had no memory of the spec critique it had just written.
#
# All four launchers can continue, and all four key it to the working directory:
#   agy      --continue          "Continue the most recent conversation"
#   zcode    --continue          "Resume the latest session for the current directory"
#   opencode -c/--continue       "continue the last session"
#   codex    exec resume --last
# So continuation and worktree reuse are the same mechanism: an arm continues where it last
# worked. A stage that wants continuity must run in the same directory as the stage before it.
#
# Continuing is BEST EFFORT and must never be load-bearing. If no prior session exists the
# launcher starts fresh, and the prompt is always self-contained -- a stage that only works
# when the agent remembers is a stage whose failure state is a subtly worse answer rather
# than an obvious one.
#
#   fb_launch <arm> <prompt> <workdir> [continue]
. /home/gabe/Documents/farmerbob/fb-isolate.sh

fb_launch() {
  local arm="$1" prompt="$2" wd="$3" cont="${4:-}" repo=/home/gabe/Documents/farmerbob
  local model
  model=$(python3 -c "
import tomllib,sys
print(tomllib.load(open('$repo/sources.toml','rb'))['source'].get('$arm',{}).get('model',''))")
  cd "$wd" || return 127
  # Per-launcher continuation flags. Held as arrays so an empty value expands to nothing
  # rather than to an empty argument, which opencode reads as a missing option value.
  local agy_c=() zcode_c=() oc_c=() claude_c=()
  if [ -n "$cont" ]; then
    agy_c=(--continue); zcode_c=(--continue); oc_c=(--continue); claude_c=(--continue)
  fi
  case "$arm" in
    # codex resumes through a SUBCOMMAND rather than a flag: `codex exec resume --last`.
    # There is no flag form, so this branch is spelled twice instead of being parameterised.
    codex-luna)
      if [ -n "$cont" ]; then
        /home/gabe/Documents/farmerbob/fb-isolated "$wd" codex exec resume --last \
            --dangerously-bypass-approvals-and-sandbox -m gpt-5.6-luna "$prompt"
      else
        /home/gabe/Documents/farmerbob/fb-isolated "$wd" codex exec \
            --dangerously-bypass-approvals-and-sandbox -m gpt-5.6-luna "$prompt"
      fi ;;
    gemini-38-flash) /home/gabe/Documents/farmerbob/fb-isolated "$wd" agy -p "$prompt" --print-timeout 45m --model gemini-3.8-flash-high --add-dir "$wd" \
                         "${agy_c[@]}" --dangerously-skip-permissions --output-format text ;;
    glm-53-flash)    /home/gabe/Documents/farmerbob/fb-isolated "$wd" zcode "${zcode_c[@]}" --prompt "$prompt" ;;
    claude-sonnet)   /home/gabe/Documents/farmerbob/fb-isolated "$wd" claude -p "$prompt" \
                         --permission-mode bypassPermissions --model sonnet \
                         --effort "${FB_CLAUDE_EFFORT:-xhigh}" "${claude_c[@]}" --output-format json ;;
    # --dir makes the worktree opencode's project root; --auto stops an UNRELATED refusal
    # from killing the run. Without them a single touch outside the tree ends the agent:
    # "permission requested: external_directory (...); auto-rejecting" and it stops there.
    # Five implementer runs and every critic on prior/bandit-route/crossx died this way, all
    # scored as the model producing nothing.  (beads farmerbob-1bd, farmerbob-k9f)
    ifm-*)           set -a; . "$HOME/.config/farmerbob/secrets.env"; set +a
                     /home/gabe/Documents/farmerbob/fb-isolated "$wd" opencode run --dir "$wd" --auto "${oc_c[@]}" -m "$model" "$prompt" ;;
    or-*)            /home/gabe/Documents/farmerbob/fb-isolated "$wd" ori opencode run --dir "$wd" --auto "${oc_c[@]}" -m "$model" "$prompt" ;;
    *)               echo "fb_launch: unknown arm $arm" >&2; return 127 ;;
  esac
}
