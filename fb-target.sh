# fb-target — THE reader for a spec's declared deliverable. Sourced, never copied.
#
# A spec declares its deliverable one of two ways:
#   <!-- fb:creates  path -->   the file must NOT exist on the base; the arm creates it
#   <!-- fb:modifies path -->   the file DOES exist on the base; the arm edits it
#
# Seven readers independently grepped for `fb:creates` alone. Consequences, all real:
#   - fb-pipeline  refused every fb:modifies task, so probe (12 candidates), fnd-store and
#     gpu-lease were unpipelineable from the day they were written and sat in NEEDS
#     CRITIQUE forever while the autopilot retried and failed identically every cycle;
#   - fb-escalate  refused to promote their findings;
#   - fb-status    fell through to NEEDS ADJUDICATION for them.
# Same shape as the verdict function reimplemented four times, and as the stale-marker
# liveness test still live in five scripts. One rule, one implementation.
#   (beads farmerbob-0d2, farmerbob-9m7)
#
#   fb_target <task>  -> echoes the declared path, or nothing
#   fb_target_verb <task> -> echoes "creates" | "modifies" | nothing
#
# fb-dispatch.sh deliberately does NOT use this: its precondition check is specific to
# `creates` (the file must be absent on the base), which is meaningless for `modifies`.

fb_spec() { echo "/home/gabe/Documents/farmerbob/.fb/prompts/$1.md"; }

fb_target() {
  local s; s=$(fb_spec "${1:?task}")
  [ -f "$s" ] || return 1
  grep -ohE '<!-- fb:(creates|modifies) [^ ]+ -->' "$s" 2>/dev/null | awk '{print $3}' | head -1
}

fb_target_verb() {
  local s; s=$(fb_spec "${1:?task}")
  [ -f "$s" ] || return 1
  grep -ohE '<!-- fb:(creates|modifies) [^ ]+ -->' "$s" 2>/dev/null \
    | awk '{print $2}' | head -1 | sed 's/^fb://'
}

# Why the target could not be read. The two causes need different fixes and reporting them
# with one message is how five stuck tasks stayed invisible for days.
fb_target_why() {
  local t="${1:?task}" s; s=$(fb_spec "$t")
  if [ ! -f "$s" ]; then echo "$t: no spec at $s"
  else echo "$t: spec exists but declares no <!-- fb:creates|fb:modifies PATH --> target"; fi
}
