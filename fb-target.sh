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
# fb-dispatch.sh uses `fb_declaration` below. It used to run its own `grep -oE` over EVERY
# fb:creates marker in the prompt, with no fence tracking and no first-wins, so a spec that
# DOCUMENTS the marker format was refused on its own examples: target-decl declared
# target_decl.rs on line 1 and was killed for "declares creates:.../window.rs", a path that
# appears only inside a fenced example block. That is the wave60 defect -- an EXAMPLE of a
# declaration parsed as a declaration -- recurring in a second reader, and fb-speclint did
# not catch it because speclint permits a FENCED marker while the dispatcher did not.

fb_spec() { echo "/home/gabe/Documents/farmerbob/.fb/prompts/$1.md"; }

# The ONE declaration a spec makes, as "<verb> <path>", or nothing.
#
# Fence-aware and first-wins, matching farmerbob_core::precondition, which is the only
# reader that already got this right. A marker inside a ``` block is an EXAMPLE and is not a
# declaration; every spec about the marker format contains several.
#
# Takes a FILE, so the same rule can be applied to a prompt that is not in .fb/prompts.
fb_declaration_in() {
  awk '
    /^[[:space:]]*```/ { fence = 1 - fence; next }
    fence { next }
    match($0, /<!-- fb:(creates|modifies) [^ ]+ -->/) {
      seg = substr($0, RSTART, RLENGTH)
      split(seg, a, " ")
      sub(/^fb:/, "", a[2])
      print a[2], a[3]
      exit
    }' "${1:?file}" 2>/dev/null
}

fb_declaration() {
  local s; s=$(fb_spec "${1:?task}")
  [ -f "$s" ] || return 1
  fb_declaration_in "$s"
}

fb_target() {
  local d; d=$(fb_declaration "${1:?task}") || return 1
  [ -n "$d" ] || return 0
  printf '%s\n' "${d#* }"
}

fb_target_verb() {
  local d; d=$(fb_declaration "${1:?task}") || return 1
  [ -n "$d" ] || return 0
  printf '%s\n' "${d%% *}"
}

# Why the target could not be read. The two causes need different fixes and reporting them
# with one message is how five stuck tasks stayed invisible for days.
fb_target_why() {
  local t="${1:?task}" s; s=$(fb_spec "$t")
  if [ ! -f "$s" ]; then echo "$t: no spec at $s"
  else echo "$t: spec exists but declares no <!-- fb:creates|fb:modifies PATH --> target"; fi
}
