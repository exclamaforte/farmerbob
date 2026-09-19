#!/usr/bin/env bash
# fb-liveness — count processes by what they ARE, not by what their command line mentions.
#
# WHY. `pgrep -f 'fb-admit\.sh'` matches any process whose FULL command line contains that
# string, which includes every shell that merely mentions it -- an orchestrator tool call, a
# grep, a waiter loop, this comment's own script. On 2026-09-19 a polling loop written that
# way matched ITSELF and waited thirty minutes on an agent that had already finished, while
# reporting the machine as busy.
#
# The same pattern decided whether the autopilot was allowed to launch. An autopilot that
# wrongly believes waves are running never dispatches; one that wrongly believes the machine
# is idle dispatches into a full one. Both were reachable from any shell that happened to
# name the script.
#
# This is bead farmerbob-7e2, "Process-name matching is not process identity", and
# farmerbob-05p, where mem_peak_mb sampled foreign cgroups for the same reason.
#
#   fb_running <script-basename>   -> prints the count of processes EXECUTING that script

# True only when the process is executing `script`: it is argv[0], or argv[1] with argv[0]
# an interpreter. A shell whose later arguments merely contain the name does not count.
fb_is_running() {
  local pid="$1" script="$2" argv0 argv1
  [ -r "/proc/$pid/cmdline" ] || return 1
  # shellcheck disable=SC2207
  local args=($(tr '\0' '\n' < "/proc/$pid/cmdline" 2>/dev/null))
  argv0="${args[0]:-}"; argv1="${args[1]:-}"
  case "${argv0##*/}" in "$script") return 0 ;; esac
  case "${argv0##*/}" in
    bash|sh|dash|zsh)
      # `bash -c '...'` is a shell running a STRING, never the script itself, whatever that
      # string contains. This is the case that produced every false positive.
      [ "$argv1" = "-c" ] && return 1
      case "${argv1##*/}" in "$script") return 0 ;; esac
      ;;
  esac
  return 1
}

# Count processes executing `script`, excluding this process and its parent.
fb_running() {
  local script="$1" n=0 pid
  for pid in $(pgrep -f "$script" 2>/dev/null); do
    [ "$pid" = "$$" ] && continue
    [ "$pid" = "$PPID" ] && continue
    fb_is_running "$pid" "$script" && n=$((n + 1))
  done
  printf '%s' "$n"
}
