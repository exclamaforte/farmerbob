//! Pure lexical checks for handing a resumed session its implementation turn.

use std::path::{Component, Path, PathBuf};

/// Where a session says it is, and where the harness needs it to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The working directory the resumed session reports.
    pub session_cwd: PathBuf,
    /// The worktree this turn must run in.
    pub worktree: PathBuf,
    /// The orchestrator's own repository. Never a legal place to implement.
    pub repo: PathBuf,
}

/// Whether the implementation turn may proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// The session is in its worktree. Continue it.
    Continue,
    /// The session is somewhere it must not write. Start a fresh session in the
    /// worktree instead of continuing this one, and say why.
    StartFresh(Reason),
}

/// Why a session may not be continued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The session is in the orchestrator's repository.
    InOrchestratorRepo,
    /// The session is in a different task's worktree.
    InForeignWorktree,
    /// The session is somewhere else entirely.
    Elsewhere,
}

/// Decide whether the resumed session may be handed the implementation turn.
pub fn resume(p: &Placement) -> Resume {
    if within(&p.worktree, &p.session_cwd) {
        return Resume::Continue;
    }

    // The repository check deliberately precedes the sibling check. This makes
    // the observed repository case unambiguous even when a legal configuration
    // places worktrees below the repository.
    if within(&p.repo, &p.session_cwd) {
        return Resume::StartFresh(Reason::InOrchestratorRepo);
    }

    if sibling_worktree(&p.worktree, &p.session_cwd) {
        return Resume::StartFresh(Reason::InForeignWorktree);
    }

    Resume::StartFresh(Reason::Elsewhere)
}

/// Whether `child` is `ancestor` or lies beneath it.
///
/// The comparison is lexical over normalized path components and never touches
/// the filesystem.
pub fn within(ancestor: &Path, child: &Path) -> bool {
    let ancestor = normalized_components(ancestor);
    let child = normalized_components(child);

    ancestor.len() <= child.len()
        && ancestor
            .iter()
            .zip(child.iter())
            .all(|(ancestor, child)| ancestor == child)
}

fn sibling_worktree(worktree: &Path, child: &Path) -> bool {
    let worktree = normalized_components(worktree);
    let child = normalized_components(child);

    let Some(worktree_leaf) = worktree.last() else {
        return false;
    };
    let parent_len = worktree.len() - 1;

    child.len() > parent_len
        && child[..parent_len] == worktree[..parent_len]
        && child[parent_len] != *worktree_leaf
}

fn normalized_components(path: &Path) -> Vec<Component<'_>> {
    let is_absolute = path.is_absolute();
    let mut normalized = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(normalized.last(), Some(Component::Normal(_))) {
                    normalized.pop();
                } else if !is_absolute {
                    normalized.push(Component::ParentDir);
                }
            }
            component => normalized.push(component),
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::{Placement, Reason, Resume, within};
    use std::path::{Path, PathBuf};

    #[test]
    fn within_uses_normalized_components() {
        assert!(within(Path::new("/x"), Path::new("/x/y/../z")));
        assert!(within(Path::new("/x"), Path::new("/x")));
        assert!(!within(Path::new("/x/foo"), Path::new("/x/foo-bar")));
        assert!(within(Path::new("/"), Path::new("/anywhere")));
    }

    #[test]
    fn resume_distinguishes_worktree_repo_sibling_and_elsewhere() {
        let worktree = PathBuf::from("/tasks/current");
        let repo = PathBuf::from("/repo");

        let placement = |session_cwd: &str| Placement {
            session_cwd: PathBuf::from(session_cwd),
            worktree: worktree.clone(),
            repo: repo.clone(),
        };

        assert_eq!(
            super::resume(&placement("/tasks/current/src")),
            Resume::Continue
        );
        assert_eq!(
            super::resume(&placement("/repo")),
            Resume::StartFresh(Reason::InOrchestratorRepo)
        );
        assert_eq!(
            super::resume(&placement("/tasks/other/src")),
            Resume::StartFresh(Reason::InForeignWorktree)
        );
        assert_eq!(
            super::resume(&placement("/tmp/scratch")),
            Resume::StartFresh(Reason::Elsewhere)
        );
    }

    #[test]
    fn worktree_wins_when_nested_in_repo() {
        let placement = Placement {
            session_cwd: PathBuf::from("/repo/tasks/current/src"),
            worktree: PathBuf::from("/repo/tasks/current"),
            repo: PathBuf::from("/repo"),
        };

        assert_eq!(super::resume(&placement), Resume::Continue);
    }
}
