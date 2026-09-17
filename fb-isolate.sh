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
REPO_FB_ISOLATED="${REPO_FB_ISOLATED:-/home/gabe/Documents/farmerbob/fb-isolated}"

# THE FUNCTION DELEGATES TO THE EXECUTABLE. It used to carry its own copy of the bwrap
# invocation, and when the admin-directory guard was added it was added HERE only -- while
# every implementer dispatch goes through the `fb-isolated` executable, because systemd-run
# execs its argument directly and cannot exec a shell function. I then verified the fix by
# calling this function, the copy I had just fixed, and reported it as holding. It held
# nowhere that mattered: healthy worktrees went from 78 to 11 over the following waves.
#
# Same failure as the verdict function reimplemented four times and the spec-target reader
# reimplemented seven. One rule, one implementation -- and when a rule must exist in two
# forms because of an exec boundary, the second form CALLS the first rather than copying it.
#   (beads farmerbob-13p, farmerbob-9m7)
fb_isolated() {
  local wt="$1"; shift
  "$REPO_FB_ISOLATED" "$wt" "$@"
}
