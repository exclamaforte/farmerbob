#!/usr/bin/env bash
# fb-live — count agents actually working in farmerbob worktrees.
#
# `pgrep -x codex` also matches the ChatGPT desktop app's six helper processes, so a naive
# count reported 7 live agents when 2 were running. Process-name matching is not identity;
# the worktree a process is cwd'd into is.
WT="$HOME/.local/share/farmerbob/worktrees"
n=0
for p in $(pgrep -x opencode; pgrep -x zcode; pgrep -x agy; pgrep -x codex); do
  cwd=$(readlink -f "/proc/$p/cwd" 2>/dev/null) || continue
  case "$cwd" in "$WT"/*) n=$((n+1)) ;; esac
done
echo "$n"
