# fb-launch — THE launcher dispatch. Sourced, never copied.
#
# Reimplementing this is how fb-prove came to run `ori opencode run -m gpt-5.6-luna` for
# codex-luna, which is a CLI arm needing `codex exec`. Every arm produced zero tests and the
# failure looked like a capability result. Same structural lesson as fb-verdict: the logic
# that decides HOW to invoke an arm belongs in one place.
#
#   fb_launch <arm> <prompt> <workdir>
. /home/gabe/Documents/farmerbob/fb-isolate.sh

fb_launch() {
  local arm="$1" prompt="$2" wd="$3" repo=/home/gabe/Documents/farmerbob
  local model
  model=$(python3 -c "
import tomllib,sys
print(tomllib.load(open('$repo/sources.toml','rb'))['source'].get('$arm',{}).get('model',''))")
  cd "$wd" || return 127
  case "$arm" in
    codex-luna)      fb_isolated "$wd" codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.6-luna "$prompt" ;;
    gemini-38-flash) fb_isolated "$wd" agy -p "$prompt" --model gemini-3.8-flash-high --add-dir "$wd" \
                         --dangerously-skip-permissions --output-format text ;;
    glm-53-flash)    fb_isolated "$wd" zcode --prompt "$prompt" ;;
    ifm-*)           set -a; . "$HOME/.config/farmerbob/secrets.env"; set +a
                     fb_isolated "$wd" opencode run -m "$model" "$prompt" ;;
    or-*)            fb_isolated "$wd" ori opencode run -m "$model" "$prompt" ;;
    *)               echo "fb_launch: unknown arm $arm" >&2; return 127 ;;
  esac
}
