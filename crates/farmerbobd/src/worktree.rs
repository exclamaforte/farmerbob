use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[allow(dead_code)]
/// Specification for creating a git worktree.
#[derive(Debug, Clone)]
pub struct WorktreeSpec {
    pub repo: PathBuf,
    pub root: PathBuf,
    pub run_id: String,
    pub task: String,
    pub agent: String,
    pub base: String,
}

#[allow(dead_code)]
/// Represents a created worktree.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

#[allow(dead_code)]
/// Entry from `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[allow(dead_code)]
/// Statistics from `git diff --numstat`.
#[derive(Debug, Clone, Default)]
pub struct DiffStat {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[allow(dead_code)]
/// Generate a sanitized branch name: `fb/<task>/<agent>/<run-id>`.
pub fn branch_name(task: &str, agent: &str, run_id: &str) -> String {
    let sanitize = |s: &str| {
        s.chars()
            .map(|c| match c {
                ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\' => '-',
                c if c.is_control() => '-',
                c => c,
            })
            .collect::<String>()
    };

    let task = sanitize(task).trim_matches(&['.', '/'][..]).to_string();
    let agent = sanitize(agent).trim_matches(&['.', '/'][..]).to_string();
    let run_id = sanitize(run_id).trim_matches(&['.', '/'][..]).to_string();

    let branch = format!("fb/{}/{}/{}", task, agent, run_id);

    // Remove any remaining `..` sequences
    let branch = branch.replace("..", ".");

    // Ensure it doesn't end with `.lock`
    let branch = branch.trim_end_matches(".lock").to_string();

    // Remove leading/trailing slashes and dots
    branch.trim_matches(&['.', '/'][..]).to_string()
}

/// Resolve a commit-ish to a full SHA.
#[allow(dead_code)]
fn resolve_commit(repo: &Path, base: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", base])
        .current_dir(repo)
        .output()
        .context("failed to execute git rev-parse")?;

    if !output.status.success() {
        anyhow::bail!(
            "failed to resolve base '{}': {}",
            base,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[allow(dead_code)]
/// Create a new worktree.
pub fn create(spec: WorktreeSpec) -> Result<Worktree> {
    let base_commit = resolve_commit(&spec.repo, &spec.base)?;

    let branch = branch_name(&spec.task, &spec.agent, &spec.run_id);
    let path = spec.root.join(&branch);

    // Ensure the path is within the allowed root
    let canonical_root = spec
        .root
        .canonicalize()
        .context("failed to canonicalize root")?;
    let canonical_path = path.canonicalize().or_else(|_| {
        // Path doesn't exist yet, canonicalize parent
        path.parent()
            .unwrap_or(&path)
            .canonicalize()
            .context("failed to canonicalize path parent")
    })?;

    if !canonical_path.starts_with(&canonical_root) {
        anyhow::bail!("worktree path escapes allowed root");
    }

    // Check if path already exists
    if path.exists() {
        anyhow::bail!("worktree path already exists: {}", path.display());
    }

    // Create the worktree
    let output = Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            &base_commit,
        ])
        .current_dir(&spec.repo)
        .output()
        .context("failed to execute git worktree add")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already exists")
            || stderr.contains("branch") && stderr.contains("exists")
        {
            anyhow::bail!("branch '{}' already exists", branch);
        }
        anyhow::bail!("git worktree add failed: {}", stderr);
    }

    Ok(Worktree {
        path,
        branch,
        base_commit,
    })
}

#[allow(dead_code)]
/// Remove a worktree and prune.
pub fn remove(repo: &Path, path: &Path, force: bool) -> Result<()> {
    // Ensure path is within a reasonable boundary (must be absolute and exist)
    let canonical_repo = repo.canonicalize().context("failed to canonicalize repo")?;
    let canonical_path = path.canonicalize().context("failed to canonicalize path")?;

    // The path should be a subdirectory of the repo's parent or the worktree root
    // We just verify it's not the main repo itself
    if canonical_path == canonical_repo {
        anyhow::bail!("cannot remove the main repository");
    }

    let path_str = path.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);

    let output = Command::new("git")
        .args(&args)
        .current_dir(repo)
        .output()
        .context("failed to execute git worktree remove")?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Prune stale worktree entries
    let output = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo)
        .output()
        .context("failed to execute git worktree prune")?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree prune failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

#[allow(dead_code)]
/// Archive a branch before deletion.
pub fn archive_branch(repo: &Path, branch: &str, task: &str, run_id: &str) -> Result<String> {
    let archive_ref = format!(
        "refs/fb/archive/{}/{}",
        sanitize_ref_component(task),
        sanitize_ref_component(run_id)
    );

    // Create the archive ref pointing to the same commit as the branch
    let output = Command::new("git")
        .args([
            "update-ref",
            &archive_ref,
            &format!("refs/heads/{}", branch),
        ])
        .current_dir(repo)
        .output()
        .context("failed to execute git update-ref")?;

    if !output.status.success() {
        anyhow::bail!(
            "git update-ref failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(archive_ref)
}

#[allow(dead_code)]
fn sanitize_ref_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '/' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect::<String>()
        .trim_matches(&['.', '/'][..])
        .to_string()
        .replace("..", ".")
}

#[allow(dead_code)]
/// List all worktrees for a repository.
pub fn list(repo: &Path) -> Result<Vec<WorktreeEntry>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
        .context("failed to execute git worktree list")?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout)?;
    parse_worktree_list(&stdout)
}

#[allow(dead_code)]
/// Parse `git worktree list --porcelain` output.
pub fn parse_worktree_list(input: &str) -> Result<Vec<WorktreeEntry>> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut current_detached = false;
    let mut current_locked = false;
    let mut current_prunable = false;

    for line in input.lines() {
        if line.is_empty() {
            // End of an entry
            if let (Some(path), Some(head)) = (current_path.take(), current_head.take()) {
                entries.push(WorktreeEntry {
                    path,
                    head,
                    branch: current_branch.take(),
                    detached: current_detached,
                    locked: current_locked,
                    prunable: current_prunable,
                });
            }
            current_detached = false;
            current_locked = false;
            current_prunable = false;
            continue;
        }

        if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path_str));
        } else if let Some(head_str) = line.strip_prefix("HEAD ") {
            current_head = Some(head_str.to_string());
        } else if let Some(branch_str) = line.strip_prefix("branch ") {
            current_branch = Some(branch_str.to_string());
        } else if line == "detached" {
            current_detached = true;
        } else if line == "locked" {
            current_locked = true;
        } else if line == "prunable" {
            current_prunable = true;
        }
    }

    // Handle last entry if no trailing newline
    if let (Some(path), Some(head)) = (current_path, current_head) {
        entries.push(WorktreeEntry {
            path,
            head,
            branch: current_branch,
            detached: current_detached,
            locked: current_locked,
            prunable: current_prunable,
        });
    }

    Ok(entries)
}

#[allow(dead_code)]
/// Get diffstat between a branch and base.
pub fn diffstat(repo: &Path, branch: &str, base: &str) -> Result<DiffStat> {
    let output = Command::new("git")
        .args(["diff", "--numstat", base, branch])
        .current_dir(repo)
        .output()
        .context("failed to execute git diff")?;

    if !output.status.success() {
        anyhow::bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout)?;
    parse_diffstat(&stdout)
}

#[allow(dead_code)]
/// Parse `git diff --numstat` output.
pub fn parse_diffstat(input: &str) -> Result<DiffStat> {
    let mut stat = DiffStat::default();

    for line in input.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let insertions = parts[0].parse().unwrap_or(0);
            let deletions = parts[1].parse().unwrap_or(0);
            // parts[2] is the filename
            if insertions > 0 || deletions > 0 {
                stat.files_changed += 1;
                stat.insertions += insertions;
                stat.deletions += deletions;
            }
        }
    }

    Ok(stat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_name_basic() {
        assert_eq!(
            branch_name("task1", "agent1", "run1"),
            "fb/task1/agent1/run1"
        );
    }

    #[test]
    fn branch_name_spaces() {
        assert_eq!(
            branch_name("my task", "my agent", "run 1"),
            "fb/my-task/my-agent/run-1"
        );
    }

    #[test]
    fn branch_name_special_chars() {
        assert_eq!(
            branch_name("task~", "agent^", "run:"),
            "fb/task-/agent-/run-"
        );
    }

    #[test]
    fn branch_name_control_chars() {
        assert_eq!(
            branch_name("task\x00", "agent\x1f", "run\x7f"),
            "fb/task-/agent-/run-"
        );
    }

    #[test]
    fn branch_name_dots_and_slashes() {
        assert_eq!(
            branch_name(".task.", "/agent/", "./run/."),
            "fb/task/agent/run"
        );
    }

    #[test]
    fn branch_name_double_dots() {
        assert_eq!(
            branch_name("ta..sk", "ag..ent", "ru..n"),
            "fb/ta.sk/ag.ent/ru.n"
        );
    }

    #[test]
    fn branch_name_lock_suffix() {
        assert_eq!(
            branch_name("task", "agent", "run.lock"),
            "fb/task/agent/run"
        );
    }

    #[test]
    fn branch_name_empty_components() {
        // Empty components get sanitized away, resulting in just "fb"
        assert_eq!(branch_name("", "", ""), "fb");
    }

    #[test]
    fn parse_worktree_list_basic() {
        let input = r#"worktree /path/to/repo
HEAD abc123
branch refs/heads/main

worktree /path/to/worktree1
HEAD def456
branch refs/heads/feature

"#;
        let entries = parse_worktree_list(input).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/path/to/repo"));
        assert_eq!(entries[0].head, "abc123");
        assert_eq!(entries[0].branch, Some("refs/heads/main".to_string()));
        assert!(!entries[0].detached);
        assert_eq!(entries[1].path, PathBuf::from("/path/to/worktree1"));
        assert_eq!(entries[1].branch, Some("refs/heads/feature".to_string()));
    }

    #[test]
    fn parse_worktree_list_detached() {
        let input = r#"worktree /path/to/repo
HEAD abc123
detached

"#;
        let entries = parse_worktree_list(input).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].detached);
        assert_eq!(entries[0].branch, None);
    }

    #[test]
    fn parse_worktree_list_locked_prunable() {
        let input = r#"worktree /path/to/repo
HEAD abc123
branch refs/heads/main
locked
prunable

"#;
        let entries = parse_worktree_list(input).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].locked);
        assert!(entries[0].prunable);
    }

    #[test]
    fn parse_worktree_list_bare() {
        let input = r#"worktree /path/to/bare
HEAD abc123
bare

"#;
        let entries = parse_worktree_list(input).unwrap();
        assert_eq!(entries.len(), 1);
        // bare repos have no branch and are detached
        assert_eq!(entries[0].branch, None);
    }

    #[test]
    fn parse_diffstat_basic() {
        let input = "3\t1\tfile1.rs\n5\t2\tfile2.rs\n";
        let stat = parse_diffstat(input).unwrap();
        assert_eq!(stat.files_changed, 2);
        assert_eq!(stat.insertions, 8);
        assert_eq!(stat.deletions, 3);
    }

    #[test]
    fn parse_diffstat_empty() {
        let input = "";
        let stat = parse_diffstat(input).unwrap();
        assert_eq!(stat.files_changed, 0);
        assert_eq!(stat.insertions, 0);
        assert_eq!(stat.deletions, 0);
    }

    #[test]
    fn parse_diffstat_binary_files() {
        let input = "-\t-\timage.png\n10\t5\tsrc/main.rs\n";
        let stat = parse_diffstat(input).unwrap();
        assert_eq!(stat.files_changed, 1);
        assert_eq!(stat.insertions, 10);
        assert_eq!(stat.deletions, 5);
    }

    #[test]
    fn parse_diffstat_only_deletions() {
        let input = "0\t5\told.txt\n";
        let stat = parse_diffstat(input).unwrap();
        assert_eq!(stat.files_changed, 1);
        assert_eq!(stat.insertions, 0);
        assert_eq!(stat.deletions, 5);
    }

    #[test]
    fn parse_diffstat_only_insertions() {
        let input = "5\t0\tnew.txt\n";
        let stat = parse_diffstat(input).unwrap();
        assert_eq!(stat.files_changed, 1);
        assert_eq!(stat.insertions, 5);
        assert_eq!(stat.deletions, 0);
    }
}
