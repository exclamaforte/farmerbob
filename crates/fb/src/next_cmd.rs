#![allow(dead_code)] // This module is a library-shaped surface until the CLI wiring task.

//! Gather the filesystem facts used by the orchestrator and render its attention list.

use farmerbob_core::target_decl;
use farmerbob_core::attention::{self, Facts, Item};
use farmerbob_core::board::{self, QueuedMatrix, TaskFiles};
use farmerbob_core::measurement::{Absent, Measurement};
use farmerbob_core::wave_compose::{self, Row};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the harness keeps its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The repository root, holding `.fb/`.
    pub repo: PathBuf,
    /// The logs directory holding `<task>.score.json` and friends.
    pub logs: PathBuf,
}

/// Gather the on-disk listing.
///
/// Task discovery is based on prompt files. A score, claims, or adjudication
/// file without a matching prompt is therefore ignored: without the prompt
/// there is no task name whose state can be observed. Names are sorted by
/// their UTF-8 byte order to make the input to `board::observe` deterministic.
///
/// Every score that cannot be read or parsed is represented by `Missing`.
/// Whether a task has been decided.
///
/// TWO ways, and the second is why `fb next` listed thirty finished tasks as
/// needing adjudication on its first real run:
///
///  1. an explicit record at `.fb/adjudicated/<task>`, which is the only way a
///     `modifies` task can say so -- its target existed before any work began;
///  2. for a `creates` task, the declared file EXISTING on disk. That is a merge,
///     and it is the rule `fb status` has always used.
///
/// Reading only (1) made this disagree with `fb status` about thirty tasks, and a
/// list whose first thirty entries are finished work is not a list anyone reads.
///
/// The declaration is read through `farmerbob_core::target_decl`, which is THE
/// reader -- fenced examples ignored, two declarations ambiguous. Not a fourth
/// regex.
fn is_adjudicated(repo: &Path, name: &str) -> bool {
    if repo.join(".fb/adjudicated").join(name).is_file() {
        return true;
    }
    let Ok(spec) = std::fs::read_to_string(repo.join(".fb/prompts").join(format!("{name}.md")))
    else {
        return false;
    };
    match target_decl::declared(&spec) {
        Ok(d) if target_decl::requires_absent(&d) => repo.join(target_decl::path(&d)).is_file(),
        _ => false,
    }
}

/// How many worktrees exist for `task`, of any arm.
///
/// Worktrees are named `<task>--<arm>` under the harness's worktree root. A
/// root that cannot be read yields 0, which is the same answer as "none": this
/// count only ever distinguishes dispatched from never-dispatched, and
/// `run_state_view` treats both as `NeverDispatched`.
fn worktree_count(task: &str) -> usize {
    let root = match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h).join(".local/share/farmerbob/worktrees"),
        None => return 0,
    };
    let prefix = format!("{task}--");
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                .count()
        })
        .unwrap_or(0)
}

pub fn gather(p: &Paths, live_agents: Measurement<usize>) -> Facts {
    let tasks = prompt_names(&p.repo)
        .into_iter()
        .map(|name| TaskFiles {
            has_spec: true,
            passing: passing_count(&p.logs.join(format!("{name}.score.json"))),
            has_claims: has_nonempty_claims(&p.logs.join(format!("{name}.claims.json"))),
            adjudicated: is_adjudicated(&p.repo, &name),
            failed_stages: Vec::new(),
            // Counted here rather than inferred: board::observe now asks
            // run_state_view whether a task's runs have FINISHED, and a
            // fabricated zero would read as "nothing was ever dispatched".
            worktrees: worktree_count(&name),
            // NOT substituted with 0. The caller of `gather` supplies the
            // machine-wide live count; a per-task count is a different
            // question and this layer has not asked it, so it says so.
            live: Measurement::Missing(Absent::NotAttempted),
            name,
        })
        .collect::<Vec<_>>();

    let queued = queue_names(&p.repo)
        .into_iter()
        .map(|name| {
            let path = p.repo.join(".fb/queue").join(&name);
            QueuedMatrix {
                runs: queue_run_count(&path),
                matrix: name,
            }
        })
        .collect::<Vec<_>>();

    board::observe(&tasks, &queued, live_agents)
}

/// Render the ranked attention list.
///
/// The core attention value owns both the explanation and the safety flag;
/// this function only formats those values and never performs another sort.
pub fn render(facts: &Facts, limit: usize) -> String {
    let ranked = attention::rank(facts);
    let count = if limit == 0 {
        ranked.len()
    } else {
        ranked.len().min(limit)
    };

    let mut output = String::new();
    for attention in ranked.iter().take(count) {
        output.push_str(&format!(
            "{:?}: {} [safe_while_busy={}]\n",
            attention.item, attention.why, attention.safe_while_busy
        ));
    }
    output
}

/// Gather, rank and render. Returns the exit code the caller should use.
pub fn run(
    p: &Paths,
    live_agents: Measurement<usize>,
    limit: usize,
    out: &mut dyn Write,
) -> i32 {
    if !directory_is_readable(&p.repo) || !directory_is_readable(&p.logs) {
        return 4;
    }

    let facts = gather(p, live_agents);
    let ranked = attention::rank(&facts);
    let rendered = render(&facts, limit);
    let _ = out.write_all(rendered.as_bytes());

    match ranked.first() {
        None | Some(attention::Attention {
            item: Item::QueueEmpty,
            ..
        }) => 1,
        Some(_) => 0,
    }
}

fn directory_is_readable(path: &Path) -> bool {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        if entry.is_err() {
            return false;
        }
    }
    true
}

fn prompt_names(repo: &Path) -> Vec<String> {
    let mut names = matching_files(&repo.join(".fb/prompts"), ".md");
    names.sort_unstable();
    names
        .into_iter()
        .filter_map(|name| name.strip_suffix(".md").map(str::to_string))
        .collect()
}

fn queue_names(repo: &Path) -> Vec<String> {
    let mut names = matching_files(&repo.join(".fb/queue"), ".tsv");
    names.sort_unstable();
    names
}

fn matching_files(dir: &Path, suffix: &str) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(suffix))
        .collect()
}

fn passing_count(path: &Path) -> Measurement<usize> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return Measurement::instrument_failed(&format!(
                "cannot read {}: {error}",
                path.display()
            ));
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Measurement::instrument_failed(&format!("invalid score JSON: {error}")),
    };
    let Some(entries) = value.as_array() else {
        return Measurement::instrument_failed("score JSON is not an array");
    };

    let mut passing = 0usize;
    for entry in entries {
        let Some(object) = entry.as_object() else {
            return Measurement::instrument_failed("score entry is not an object");
        };
        if object.get("verdict").and_then(Value::as_str) == Some("PASS") {
            passing = match passing.checked_add(1) {
                Some(count) => count,
                None => return Measurement::instrument_failed("passing count overflowed usize"),
            };
        }
    }
    Measurement::observed(passing)
}

fn has_nonempty_claims(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|contents| !contents.is_empty())
}

fn queue_run_count(path: &Path) -> usize {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return 0,
    };

    let rows = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(queue_row)
        .collect::<Vec<_>>();
    wave_compose::run_count(&rows)
}

fn queue_row(line: &str) -> Row {
    let columns = line.split('\t').collect::<Vec<_>>();
    let arms = columns
        .get(2)
        .copied()
        .filter(|arms| !arms.trim().is_empty())
        .map(|arms| arms.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    Row {
        task: columns.first().copied().unwrap_or_default().to_string(),
        crate_name: columns.get(1).copied().unwrap_or_default().to_string(),
        arms,
        target: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::attention::Item;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture(label: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "fb-next-cmd-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn paths(label: &str) -> Paths {
        let repo = fixture(label);
        let logs = fixture(&format!("{label}-logs"));
        fs::create_dir_all(repo.join(".fb/prompts")).expect("create prompt fixture");
        fs::create_dir_all(repo.join(".fb/queue")).expect("create queue fixture");
        fs::create_dir_all(repo.join(".fb/adjudicated")).expect("create adjudication fixture");
        fs::create_dir_all(&logs).expect("create log fixture");
        Paths { repo, logs }
    }

    fn remove(paths: &Paths) {
        let _ = fs::remove_dir_all(&paths.repo);
        let _ = fs::remove_dir_all(&paths.logs);
    }

    #[test]
    fn gather_distinguishes_missing_and_zero_scores() {
        let missing = paths("missing-score");
        fs::write(missing.repo.join(".fb/prompts/missing.md"), "spec").expect("write prompt");
        let zero = paths("zero-score");
        fs::write(zero.repo.join(".fb/prompts/zero.md"), "spec").expect("write prompt");
        fs::write(zero.logs.join("zero.score.json"), "[]").expect("write score");

        let missing_facts = gather(&missing, Measurement::not_attempted());
        let zero_facts = gather(&zero, Measurement::not_attempted());
        assert!(matches!(
            &missing_facts.items[0],
            Item::QueueEmpty
        ));
        assert!(matches!(
            &zero_facts.items[0],
            Item::RunPipeline { task, .. } if task == "zero"
        ));

        remove(&missing);
        remove(&zero);
    }

    #[test]
    fn gather_counts_all_nonblank_queue_rows() {
        let fixture = paths("queue-count");
        fs::write(
            fixture.repo.join(".fb/queue/wave.tsv"),
            "a\tcrate\tarm-a\n\n b\tcrate\tarm-b,arm-c\n",
        )
        .expect("write queue");
        let facts = gather(&fixture, Measurement::not_attempted());
        assert!(matches!(
            &facts.items[0],
            Item::LaunchWave { matrix, runs } if matrix == "wave.tsv" && *runs == 3
        ));
        remove(&fixture);
    }

    #[test]
    fn render_respects_rank_and_limit_without_reordering() {
        let facts = Facts {
            items: vec![
                Item::LaunchWave {
                    matrix: "later.tsv".to_string(),
                    runs: 1,
                },
                Item::Adjudicate {
                    task: "first".to_string(),
                    passing: 1,
                },
            ],
            live_agents: Measurement::observed(0),
        };
        let rendered = render(&facts, 1);
        assert!(rendered.contains("first"));
        assert!(!rendered.contains("later.tsv"));
        assert!(rendered.contains("safe_while_busy=false"));
    }

    #[test]
    fn run_distinguishes_unreadable_harness_from_idle_harness() {
        let fixture = paths("run-codes");
        let mut output = Vec::new();
        assert_eq!(
            run(
                &fixture,
                Measurement::not_attempted(),
                0,
                &mut output
            ),
            1
        );
        remove(&fixture);
        let missing = Paths {
            repo: fixture.repo,
            logs: fixture.logs,
        };
        assert_eq!(
            run(
                &missing,
                Measurement::not_attempted(),
                0,
                &mut output
            ),
            4
        );
    }
}
