//! `fb revalidate` — check queued task specs against the live base.
//!
//! Bead farmerbob-m71: merging a winner can create the very file a queued
//! task was told to create, and four arms then correctly no-op at full
//! price. Dispatch already refuses such tasks one by one at launch time
//! (see `dispatch_cmd::precondition`); this command checks the WHOLE
//! queue up front, and the post-merge hook runs it automatically, because
//! merging is exactly the event that invalidates queued specs.
//!
//! The base-commit bookkeeping the bead also asks for is deliberately not
//! implemented: dispatch validates declarations against the live worktree
//! at every dispatch, which is stronger than comparing recorded SHAs (no
//! stale-read window between check and use). What matters is that no
//! tokens are spent on an invalid premise, and that is what is checked.
//!
//! Existence is read from git HEAD (`git cat-file -e`), not from the
//! working tree: the tree may hold uncommitted work, and the base is what
//! dispatch builds worktrees from. Read-only throughout: this command
//! launches nothing and writes nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One queued row's standing against the live base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowVerdict {
    /// Declarations hold on the live base.
    Valid,
    /// No spec file for the task: unknown, not invalid. The dispatcher
    /// will fail this row on its own when it cannot read the prompt.
    NoSpec,
    /// The spec declares nothing checkable. Other stages report that;
    /// not this command's refusal.
    NoDeclaration,
    /// A declaration contradicts the live base. Carries why.
    Invalid(String),
}

/// Check one spec's declarations. Pure: all I/O is behind `exists` and
/// the text is given.
pub fn check_row(spec: Option<&str>, exists: &impl Fn(&str) -> bool) -> RowVerdict {
    let Some(text) = spec else {
        return RowVerdict::NoSpec;
    };
    match crate::dispatch_cmd::precondition(text, exists) {
        crate::dispatch_cmd::Precondition::Holds => RowVerdict::Valid,
        crate::dispatch_cmd::Precondition::NoDeclaration => RowVerdict::NoDeclaration,
        crate::dispatch_cmd::Precondition::Invalid(why) => RowVerdict::Invalid(why),
    }
}

/// Whether `path` exists on the live base (git HEAD), via `cat-file -e`.
/// Anything git cannot confirm counts as absent: a base that cannot be
/// read is not a base to dispatch against, and failing closed here only
/// withholds a wave, never launches a bad one.
pub fn exists_on_base(repo: &Path, path: &str) -> bool {
    // Absolute paths and escapes never name repository content.
    if path.starts_with('/') || path == ".." || path.split('/').any(|c| c == "..") {
        return false;
    }
    Command::new("git")
        .current_dir(repo)
        .args(["cat-file", "-e", &format!("HEAD:{path}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// One row of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowReport {
    /// Queue file basename.
    pub matrix: String,
    /// Task name.
    pub task: String,
    /// Arms waiting on it.
    pub arms: Vec<String>,
    /// Standing.
    pub verdict: RowVerdict,
}

/// Check every queued matrix in `queue_dir` (top level only: `dispatched/`
/// already ran). Returns the reports; the caller decides what fails.
pub fn check_queue(repo: &Path, queue_dir: &Path) -> Vec<RowReport> {
    let mut reports = Vec::new();
    let Ok(entries) = std::fs::read_dir(queue_dir) else {
        return reports;
    };
    let mut matrices: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    matrices.sort();
    for matrix in matrices {
        let Ok(text) = std::fs::read_to_string(&matrix) else {
            continue;
        };
        let name = matrix
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for row in crate::admit_cmd::parse_matrix(&text) {
            let spec_path = repo.join(".fb/prompts").join(format!("{}.md", row.task));
            let spec = std::fs::read_to_string(&spec_path).ok();
            let verdict = check_row(spec.as_deref(), &|p| exists_on_base(repo, p));
            reports.push(RowReport {
                matrix: name.clone(),
                task: row.task.clone(),
                arms: row.arms.clone(),
                verdict,
            });
        }
    }
    reports
}

/// Render one report line. Never empty.
pub fn render(report: &RowReport) -> String {
    let arms = if report.arms.is_empty() {
        "(no arms)".to_string()
    } else {
        report.arms.join(",")
    };
    match &report.verdict {
        RowVerdict::Valid => format!("{} {} [{}]: VALID", report.matrix, report.task, arms),
        RowVerdict::NoSpec => format!(
            "{} {} [{}]: NO-SPEC (no .fb/prompts/{}.md)",
            report.matrix, report.task, arms, report.task
        ),
        RowVerdict::NoDeclaration => format!(
            "{} {} [{}]: NO-DECLARATION (nothing checkable declared)",
            report.matrix, report.task, arms
        ),
        RowVerdict::Invalid(why) => format!(
            "{} {} [{}]: INVALID ({}; premise gone, do not dispatch)",
            report.matrix, report.task, arms, why
        ),
    }
}

/// Check the queue and report. Returns 1 when any row is invalid, else 0.
/// Unreadable queue directories report nothing and pass: there is no
/// queue to invalidate.
pub fn run(repo: &Path, queue_dir: &Path, out: &mut dyn std::io::Write) -> i32 {
    let reports = check_queue(repo, queue_dir);
    let mut invalid = 0;
    for report in &reports {
        if matches!(report.verdict, RowVerdict::Invalid(_)) {
            invalid += 1;
        }
        let _ = writeln!(out, "{}", render(report));
    }
    let _ = writeln!(out, "revalidate: {} rows, {invalid} invalid", reports.len());
    if invalid > 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Declarations hold or fail exactly as the dispatcher would call
    /// them: this is the same `precondition`, not a second opinion.
    #[test]
    fn verdicts_mirror_the_dispatcher() {
        let holds = |_: &str| true;
        let missing = |_: &str| false;
        assert_eq!(
            check_row(Some("<!-- fb:creates new.rs -->\n"), &missing),
            RowVerdict::Valid
        );
        assert!(matches!(
            check_row(Some("<!-- fb:creates new.rs -->\n"), &holds),
            RowVerdict::Invalid(_)
        ));
        assert!(matches!(
            check_row(Some("<!-- fb:modifies gone.rs -->\n"), &missing),
            RowVerdict::Invalid(_)
        ));
        assert_eq!(
            check_row(Some("# nothing\n"), &missing),
            RowVerdict::NoDeclaration
        );
        assert_eq!(check_row(None, &missing), RowVerdict::NoSpec);
    }

    /// The report names the matrix, the task, the arms, and -- for the
    /// invalid case -- why the premise is gone.
    #[test]
    fn invalid_names_the_premise() {
        let report = RowReport {
            matrix: "wave9.tsv".to_string(),
            task: "lease".to_string(),
            arms: vec!["a".to_string(), "b".to_string()],
            verdict: RowVerdict::Invalid(
                "declares creates:crates/x.rs but it already exists on the base".to_string(),
            ),
        };
        let line = render(&report);
        assert!(line.contains("INVALID"), "{line}");
        assert!(line.contains("already exists"), "{line}");
        assert!(line.contains("a,b"), "{line}");
    }

    /// End to end over a scratch queue: dispatched/ is not re-checked,
    /// and rows report their standing.
    #[test]
    fn queue_walk_reports_rows_skips_dispatched() {
        let base = std::env::temp_dir().join(format!("fb-revalidate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let queue = base.join("queue");
        std::fs::create_dir_all(queue.join("dispatched")).unwrap();
        std::fs::write(queue.join("w1.tsv"), "task-a\tkrate\tarm1,arm2\n").unwrap();
        std::fs::write(
            queue.join("dispatched").join("w0.tsv"),
            "task-b\tkrate\tarm1\n",
        )
        .unwrap();
        // No .fb/prompts/task-a.md beside the test repo: every row is
        // NoSpec here (repo-relative lookup); the walk itself is what is
        // pinned -- dispatched/ skipped, rows reported.
        let reports = check_queue(&base, &queue);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].matrix, "w1.tsv");
        assert_eq!(reports[0].verdict, RowVerdict::NoSpec);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// exists_on_base refuses escapes and absolutes without asking git.
    #[test]
    fn escapes_never_name_content() {
        let base = std::env::temp_dir().join(format!("fb-revalidate-esc-{}", std::process::id()));
        assert!(!exists_on_base(&base, "/etc/passwd"));
        assert!(!exists_on_base(&base, "../x"));
        assert!(!exists_on_base(&base, "crates/../../x"));
    }

    /// Exit codes: 1 with an invalid row present, else 0 -- against a
    /// scratch git repo so the live base is controlled, not inherited.
    #[test]
    fn exit_code_follows_invalid_rows() {
        let base = std::env::temp_dir().join(format!("fb-revalidate-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let queue = base.join("queue");
        std::fs::create_dir_all(queue.join(".fb").join("prompts")).unwrap();
        let git_ok = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&base)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        assert!(git_ok(&["init", "-q"]));
        assert!(git_ok(&["config", "user.email", "t@t"]));
        assert!(git_ok(&["config", "user.name", "t"]));
        // The live base holds shipped.rs; the spec below is the bead's
        // scenario: a queued task told to create a file the merge made.
        // Specs resolve under the repo root, as admit resolves them.
        std::fs::create_dir_all(base.join("crates")).unwrap();
        std::fs::write(base.join("crates/shipped.rs"), "// winner\n").unwrap();
        std::fs::create_dir_all(base.join(".fb/prompts")).unwrap();
        std::fs::write(
            base.join(".fb/prompts/gone.md"),
            "<!-- fb:creates crates/fresh.rs -->\n",
        )
        .unwrap();
        std::fs::write(
            base.join(".fb/prompts/stale.md"),
            "<!-- fb:creates crates/shipped.rs -->\n",
        )
        .unwrap();
        assert!(git_ok(&["add", "-A"]));
        assert!(git_ok(&["commit", "-qm", "base"]));
        std::fs::write(queue.join("w.tsv"), "gone\tk\ta1\nstale\tk\ta2\n").unwrap();
        let mut out = Vec::new();
        assert_eq!(run(&base, &queue, &mut out), 1);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("gone") && text.contains("VALID"), "{text}");
        assert!(text.contains("stale") && text.contains("INVALID"), "{text}");
        assert!(text.contains("already exists"), "{text}");
        assert!(text.contains("1 invalid"), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
