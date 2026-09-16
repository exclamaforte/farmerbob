# fb-isolate — hide sibling worktrees from a run.
#
# Every candidate worktree lives under one root, so any run could reach a rival's in-progress
# implementation with `../`. Observed live: a critic tried to `cat` a file out of a different
# worktree. opencode's external-directory guard happened to block it, but three of the four
# launchers run with their sandboxes explicitly disabled because they need to write freely
# inside their own tree.
#
# systemd `--scope` gets no mount namespace, so InaccessiblePaths= is rejected there. bwrap
# gives one: bind the filesystem, cover the worktree root with a tmpfs, then bind back only
# this run's own tree. Siblings are not merely unreadable, they are not present.
#
# Same failure class as the hidden-suite leak: silent, and it makes results look BETTER.
#   (bead farmerbob-bdv)
#
#   fb_isolated <worktree> <cmd...>
fb_isolated() {
  local wt="$1"; shift
  local root="$HOME/.local/share/farmerbob/worktrees"
  if [ "${FB_NO_ISOLATE:-0}" = 1 ] || ! command -v bwrap >/dev/null 2>&1; then
    "$@"; return $?
  fi
  bwrap --dev-bind / / \
        --tmpfs "$root" \
        --bind "$wt" "$wt" \
        -- "$@"
}
