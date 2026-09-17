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
REPO_GIT_WORKTREES="${REPO_GIT_WORKTREES:-/home/gabe/Documents/farmerbob/.git/worktrees}"

fb_isolated() {
  local wt="$1"; shift
  local root="$HOME/.local/share/farmerbob/worktrees"
  if [ "${FB_NO_ISOLATE:-0}" = 1 ] || ! command -v bwrap >/dev/null 2>&1; then
    "$@"; return $?
  fi
  # The tmpfs hides every sibling worktree -- which is the point -- but it also hides them
  # from GIT, whose admin directories live in the SHARED .git and stay fully writable under
  # --dev-bind. Inside this namespace `git worktree prune` sees ~180 worktrees whose "gitdir
  # file points to non-existent location" and deletes all of their admin directories. The
  # files survive; the repository forgets they are worktrees, so `git diff` answers "fatal:
  # not a git repository" forever after.
  #
  # That is what destroyed 103 of 181 candidate worktrees. It is unrecoverable: the base
  # commit each candidate branched from is gone, so no diff can ever be reconstructed, and
  # the scorer read the resulting git failure as "the arm wrote nothing" -- 16 arms were
  # falsely recorded as having done nothing at all.  (beads farmerbob-13p, farmerbob-bdv)
  #
  # Verified by dry-run: `git worktree prune --dry-run` inside this sandbox reports 77
  # removals; outside it reports none.
  #
  # Fix: the worktree admin directory is read-only in here, with THIS run's own entry bound
  # back writable so its own git still works. A prune then fails on every sibling instead of
  # succeeding on all of them. Two mounts, constant cost, and it cannot miss a sibling the
  # way an explicit per-sibling exclusion list could.
  local admin="$REPO_GIT_WORKTREES" own
  own="$admin/$(basename "$wt")"
  local -a guard=()
  if [ -d "$admin" ]; then
    guard+=(--ro-bind "$admin" "$admin")
    [ -d "$own" ] && guard+=(--bind "$own" "$own")
  fi
  bwrap --dev-bind / / \
        --tmpfs "$root" \
        --bind "$wt" "$wt" \
        "${guard[@]}" \
        -- "$@"
}
