//! `fb scope <task> [--arm <arm>]` — assess whether a run stayed in scope.
//!
//! Assesses one arm's worktree, or every arm of a task when `arm` is `None`,
//! against the task's declared deliverable. Runs both `farmerbob_core::scope::assess`
//! on file paths and `farmerbob_core::lib_diff::classify` on the sibling `lib.rs`
//! diff, printing a verdict per arm.
//!
//! # Exit Codes
//!
//! - `0`: every assessed worktree is clean
//! - `1`: any worktree departed from scope
//! - `2`: nothing could be assessed, or some worktrees were unassessable while none departed

use farmerbob_core::lib_diff::{DiffLine, LibChange, classify, permitted};
use farmerbob_core::precondition;
use farmerbob_core::scope::{
    Change, Declared, Departure, Scope, assess, is_clean, module_declaration_for,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Read the task's declared target from prompt markdown using `precondition::declarations`.
///
/// Recognizes lines starting with `<!-- fb:creates <path> -->` or
/// `<!-- fb:modifies <path> -->`, ignoring comments in prose or fenced code blocks.
/// Returns the first declared path, or `None` if no target is declared.
fn parse_target_from_spec(prompt_text: &str) -> Option<String> {
    precondition::declarations(prompt_text)
        .into_iter()
        .next()
        .map(|d| d.path)
}

/// Reads `.fb/prompts/<task>.md` from the repo root or git common-dir.
fn find_task_prompt(task: &str) -> Option<String> {
    let repo_prompt = crate::paths::repo()
        .join(".fb/prompts")
        .join(format!("{task}.md"));
    if let Ok(content) = std::fs::read_to_string(&repo_prompt) {
        return Some(content);
    }
    // Fallback if running inside a worktree where FB_REPO is not set
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        && output.status.success()
    {
        let common = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let common_path = Path::new(&common);
        if let Some(parent) = common_path.parent() {
            let candidate = parent.join(".fb/prompts").join(format!("{task}.md"));
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                return Some(content);
            }
        }
    }
    // Fallback: walk up directory tree from current dir
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            let candidate = dir.join(".fb/prompts").join(format!("{task}.md"));
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                return Some(content);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

/// Extracts the arm name from a `<task>--<arm>` directory name.
///
/// Splits on the FIRST `--` after the task prefix so that arm names
/// containing `--` (e.g. `bandit-route--gemini-38-flash`) are preserved.
fn parse_arm_name(entry_name: &str, task: &str) -> Option<String> {
    let prefix = format!("{task}--");
    let rest = entry_name.strip_prefix(&prefix)?;
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Finds all worktrees matching `<task>--*` in `wt_root`.
fn find_worktrees(wt_root: &Path, task: &str) -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    let Ok(entries) = std::fs::read_dir(wt_root) else {
        return result;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() || ft.is_symlink() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(arm) = parse_arm_name(&name, task) {
                result.push((arm, entry.path()));
            }
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

/// Converts git tracked and untracked output into `Change` records.
fn build_changes<F>(tracked_output: &str, untracked_output: &str, file_exists: F) -> Vec<Change>
where
    F: Fn(&str) -> bool,
{
    let mut changes = Vec::new();
    let mut seen = HashSet::new();

    for line in tracked_output.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        let deleted = !file_exists(path);
        if seen.insert(path.to_string()) {
            changes.push(Change {
                path: path.to_string(),
                deleted,
            });
        }
    }

    for line in untracked_output.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        if seen.insert(path.to_string()) {
            changes.push(Change {
                path: path.to_string(),
                deleted: false,
            });
        }
    }

    changes
}

/// Converts unified diff text into `DiffLine` values.
///
/// Strips the leading `+` or `-` marker, omitting file headers (`+++` / `---`)
/// and context lines.
fn parse_diff_lines(diff_output: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for line in diff_output.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            lines.push(DiffLine {
                text: text.to_string(),
                added: true,
            });
        } else if let Some(text) = line.strip_prefix('-') {
            lines.push(DiffLine {
                text: text.to_string(),
                added: false,
            });
        }
    }
    lines
}

/// The assessment outcome for a single arm.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArmAssessment {
    Clean {
        changed_nothing: bool,
    },
    Departed {
        scope: Scope,
        lib_change: Option<LibChange>,
    },
    WorktreeMissing(PathBuf),
    GitFailed {
        command: String,
        error: String,
    },
}

/// Evaluates scope and lib_diff to produce an `ArmAssessment`.
fn evaluate_verdict(
    changes: &[Change],
    scope: Scope,
    lib_change: Option<LibChange>,
) -> ArmAssessment {
    let scope_ok = is_clean(&scope);
    let lib_ok = match &lib_change {
        Some(lc) => permitted(lc),
        None => true,
    };

    if scope_ok && lib_ok {
        ArmAssessment::Clean {
            changed_nothing: changes.is_empty(),
        }
    } else {
        ArmAssessment::Departed { scope, lib_change }
    }
}

/// Runs git in `wt_path` to assess one worktree against `target`.
fn assess_arm(wt_path: &Path, target: &str) -> ArmAssessment {
    if !wt_path.exists() {
        return ArmAssessment::WorktreeMissing(wt_path.to_path_buf());
    }

    // Step 2: git diff --name-only HEAD -- crates/
    //
    // The `crates/` pathspec is NOT decoration and its absence made this command
    // unusable. Worktrees here are provisioned SPARSE: about sixteen entries are
    // checked out and git reports every other file in the repository as deleted. Run
    // without the pathspec, `fb scope` reported 777 departures for an arm that had
    // done nothing wrong -- .beads, .agents, docs, every path the provisioner did not
    // materialise.
    //
    // The two readers that already existed both scope it, and now all three agree:
    //     fb-score.sh:67     git -C "$WT" diff --name-only HEAD -- crates/
    //     fb-critique.sh:96  git diff --name-only HEAD -- crates/
    //
    // The spec pinned the command without the pathspec, so all three candidates
    // implemented exactly what was asked and every unit test passed -- they construct
    // `Change` values directly, because the spec's own Rules forbade shelling out to
    // git. The defect was invisible until the merged code was run against a real
    // worktree. (filed)
    let diff_tracked = match Command::new("git")
        .current_dir(wt_path)
        .args(["diff", "--name-only", "HEAD", "--", "crates/"])
        .output()
    {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return ArmAssessment::GitFailed {
                command: "git diff --name-only HEAD".to_string(),
                error: if err.is_empty() {
                    format!("exit code {:?}", out.status.code())
                } else {
                    err
                },
            };
        }
        Err(e) => {
            return ArmAssessment::GitFailed {
                command: "git diff --name-only HEAD".to_string(),
                error: e.to_string(),
            };
        }
    };

    // Step 2: git ls-files --others --exclude-standard -- crates/
    // Same pathspec, same reason: an unchecked-out file is not a change.
    let untracked = match Command::new("git")
        .current_dir(wt_path)
        .args([
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            "crates/",
        ])
        .output()
    {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return ArmAssessment::GitFailed {
                command: "git ls-files --others --exclude-standard".to_string(),
                error: if err.is_empty() {
                    format!("exit code {:?}", out.status.code())
                } else {
                    err
                },
            };
        }
        Err(e) => {
            return ArmAssessment::GitFailed {
                command: "git ls-files --others --exclude-standard".to_string(),
                error: e.to_string(),
            };
        }
    };

    let tracked_text = String::from_utf8_lossy(&diff_tracked.stdout);
    let untracked_text = String::from_utf8_lossy(&untracked.stdout);

    let changes = build_changes(&tracked_text, &untracked_text, |p| wt_path.join(p).exists());

    // Step 3: scope::assess
    let declared = Declared {
        target: target.to_string(),
    };
    let assessed_scope = assess(&declared, &changes);

    // Step 4: When changed set includes lib.rs beside target, run lib_diff::classify
    let neighbouring_lib = module_declaration_for(target);
    let lib_change = if let Some(ref lib_path) = neighbouring_lib {
        if changes.iter().any(|c| c.path == *lib_path) {
            let lib_diff_out = match Command::new("git")
                .current_dir(wt_path)
                .args(["diff", "HEAD", "--", lib_path])
                .output()
            {
                Ok(out) if out.status.success() => out,
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    return ArmAssessment::GitFailed {
                        command: format!("git diff HEAD -- {lib_path}"),
                        error: if err.is_empty() {
                            format!("exit code {:?}", out.status.code())
                        } else {
                            err
                        },
                    };
                }
                Err(e) => {
                    return ArmAssessment::GitFailed {
                        command: format!("git diff HEAD -- {lib_path}"),
                        error: e.to_string(),
                    };
                }
            };

            let mut diff_lines = parse_diff_lines(&String::from_utf8_lossy(&lib_diff_out.stdout));
            // If git diff against HEAD was empty, but the file is untracked, lines are additions
            if diff_lines.is_empty()
                && wt_path.join(lib_path).is_file()
                && untracked_text.lines().any(|l| l.trim() == lib_path)
                && let Ok(content) = std::fs::read_to_string(wt_path.join(lib_path))
            {
                for line in content.lines() {
                    diff_lines.push(DiffLine {
                        text: line.to_string(),
                        added: true,
                    });
                }
            }

            Some(classify(&diff_lines))
        } else {
            None
        }
    } else {
        None
    };

    evaluate_verdict(&changes, assessed_scope, lib_change)
}

/// Prints the verdict and details for one arm.
fn print_arm_verdict(arm: &str, assessment: &ArmAssessment) {
    match assessment {
        ArmAssessment::Clean { changed_nothing } => {
            if *changed_nothing {
                println!("{arm}: CLEAN (changed nothing)");
            } else {
                println!("{arm}: CLEAN");
            }
        }
        ArmAssessment::Departed { scope, lib_change } => {
            println!("{arm}: NOT CLEAN");
            for dep in &scope.departures {
                match dep {
                    Departure::Foreign { path } => println!("  departure: foreign file `{path}`"),
                    Departure::Deleted { path } => println!("  departure: deleted file `{path}`"),
                }
            }
            if let Some(LibChange::Beyond { lines }) = lib_change {
                for line in lines {
                    let sign = if line.added { '+' } else { '-' };
                    println!("  offending lib.rs line: {sign}{}", line.text);
                }
            }
        }
        ArmAssessment::WorktreeMissing(path) => {
            println!(
                "{arm}: UNASSESSABLE: worktree does not exist: {}",
                path.display()
            );
        }
        ArmAssessment::GitFailed { command, error } => {
            println!("{arm}: UNASSESSABLE: {command} failed: {error}");
        }
    }
}

/// Determines the aggregate process exit code across all assessed arms.
///
/// Returns:
/// - `0`: every assessed worktree is clean
/// - `1`: any worktree departed
/// - `2`: nothing could be assessed, or some worktrees were unassessable while none departed
fn aggregate_exit_code(assessments: &[ArmAssessment]) -> i32 {
    if assessments.is_empty() {
        return 2;
    }
    let any_departed = assessments
        .iter()
        .any(|a| matches!(a, ArmAssessment::Departed { .. }));
    if any_departed {
        return 1;
    }
    let any_unassessable = assessments.iter().any(|a| {
        matches!(
            a,
            ArmAssessment::WorktreeMissing(_) | ArmAssessment::GitFailed { .. }
        )
    });
    if any_unassessable {
        return 2;
    }
    0
}

/// Assess one arm's worktree, or every arm of a task when `arm` is `None`.
///
/// Returns a process exit code: 0 when every assessed worktree is clean,
/// 1 when any departed, 2 when nothing could be assessed.
pub fn run_cmd(task: &str, arm: Option<&str>) -> i32 {
    let prompt_text = match find_task_prompt(task) {
        Some(t) => t,
        None => {
            println!("fb scope: spec not found for task `{task}` in .fb/prompts/{task}.md");
            return 2;
        }
    };

    let target = match parse_target_from_spec(&prompt_text) {
        Some(t) => t,
        None => {
            println!("fb scope: task `{task}` declares no target in .fb/prompts/{task}.md");
            return 2;
        }
    };

    let wt_root = crate::paths::worktrees();

    let worktrees = match arm {
        Some(arm_name) => {
            let path = wt_root.join(format!("{task}--{arm_name}"));
            vec![(arm_name.to_string(), path)]
        }
        None => find_worktrees(&wt_root, task),
    };

    if worktrees.is_empty() {
        println!(
            "fb scope: no worktrees found for task `{task}` in {}",
            wt_root.display()
        );
        return 2;
    }

    let mut assessments = Vec::new();
    for (arm_name, wt_path) in &worktrees {
        let assessment = assess_arm(wt_path, &target);
        print_arm_verdict(arm_name, &assessment);
        assessments.push(assessment);
    }

    aggregate_exit_code(&assessments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: false,
        }
    }

    fn deleted(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: true,
        }
    }

    fn diff_add(text: &str) -> DiffLine {
        DiffLine {
            text: text.to_string(),
            added: true,
        }
    }

    fn diff_rem(text: &str) -> DiffLine {
        DiffLine {
            text: text.to_string(),
            added: false,
        }
    }

    // Clause 1: A worktree whose only change is the declared target is CLEAN, and run_cmd returns 0.
    #[test]
    fn clause1_target_only_change_is_clean() {
        let target = "crates/fb/src/scope_cmd.rs";
        let changes = [change(target)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let verdict = evaluate_verdict(&changes, sc, None);
        assert_eq!(
            verdict,
            ArmAssessment::Clean {
                changed_nothing: false
            }
        );
        assert_eq!(aggregate_exit_code(&[verdict]), 0);
    }

    // Clause 2: A worktree that also adds `pub mod y;` to neighbouring lib.rs is CLEAN.
    #[test]
    fn clause2_target_and_neighbouring_lib_pub_mod_is_clean() {
        let target = "crates/farmerbob-core/src/new_module.rs";
        let lib = "crates/farmerbob-core/src/lib.rs";
        let changes = [change(target), change(lib)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let lib_diff = [diff_add("pub mod new_module;")];
        let lib_change = classify(&lib_diff);
        let verdict = evaluate_verdict(&changes, sc, Some(lib_change));
        assert_eq!(
            verdict,
            ArmAssessment::Clean {
                changed_nothing: false
            }
        );
        assert_eq!(aggregate_exit_code(&[verdict]), 0);
    }

    // Clause 3: A worktree whose lib.rs diff contains anything else is NOT clean, and offending lines reported.
    #[test]
    fn clause3_neighbouring_lib_with_other_edits_is_not_clean() {
        let target = "crates/farmerbob-core/src/new_module.rs";
        let lib = "crates/farmerbob-core/src/lib.rs";
        let changes = [change(target), change(lib)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let lib_diff = [
            diff_add("pub mod new_module;"),
            diff_add("pub(crate) conn: Connection,"),
        ];
        let lib_change = classify(&lib_diff);
        let verdict = evaluate_verdict(&changes, sc, Some(lib_change));
        match &verdict {
            ArmAssessment::Departed { scope, lib_change } => {
                assert!(
                    scope.departures.is_empty(),
                    "lib.rs was allowed path by scope"
                );
                match lib_change {
                    Some(LibChange::Beyond { lines }) => {
                        assert_eq!(lines.len(), 1);
                        assert_eq!(lines[0].text, "pub(crate) conn: Connection,");
                    }
                    _ => panic!("expected LibChange::Beyond"),
                }
            }
            _ => panic!("expected Departed"),
        }
        assert_eq!(aggregate_exit_code(&[verdict]), 1);
    }

    // Clause 3: The store-batch case (visibility modifications and deletions in lib.rs).
    #[test]
    fn clause3_store_batch_edits_are_beyond() {
        let target = "crates/farmerbob-core/src/batch.rs";
        let lib = "crates/farmerbob-core/src/lib.rs";
        let changes = [change(target), change(lib)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let lib_diff = [
            diff_rem("    conn: Connection,"),
            diff_add("    pub(crate) conn: Connection,"),
        ];
        let lib_change = classify(&lib_diff);
        let verdict = evaluate_verdict(&changes, sc, Some(lib_change));
        assert!(matches!(verdict, ArmAssessment::Departed { .. }));
        assert_eq!(aggregate_exit_code(&[verdict]), 1);
    }

    // Clause 4: A worktree that changed a file which is neither target nor lib.rs is not clean.
    #[test]
    fn clause4_foreign_path_changed_is_not_clean() {
        let target = "crates/fb/src/scope_cmd.rs";
        let foreign = "crates/fb/src/other.rs";
        let changes = [change(target), change(foreign)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let verdict = evaluate_verdict(&changes, sc, None);
        match &verdict {
            ArmAssessment::Departed { scope, .. } => {
                assert_eq!(
                    scope.departures,
                    vec![Departure::Foreign {
                        path: foreign.to_string()
                    }]
                );
            }
            _ => panic!("expected Departed"),
        }
        assert_eq!(aggregate_exit_code(&[verdict]), 1);
    }

    // Clause 4: Deleting an unrelated file is a departure.
    #[test]
    fn clause4_deleted_foreign_path_is_not_clean() {
        let target = "crates/fb/src/scope_cmd.rs";
        let foreign = "crates/fb/src/unrelated.rs";
        let changes = [change(target), deleted(foreign)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let verdict = evaluate_verdict(&changes, sc, None);
        match &verdict {
            ArmAssessment::Departed { scope, .. } => {
                assert_eq!(
                    scope.departures,
                    vec![Departure::Deleted {
                        path: foreign.to_string()
                    }]
                );
            }
            _ => panic!("expected Departed"),
        }
        assert_eq!(aggregate_exit_code(&[verdict]), 1);
    }

    // Clause 5: A task whose spec declares no target returns None from parser.
    #[test]
    fn clause5_spec_declaring_no_target_returns_none() {
        let prompt = "# Task without deliverable\nJust implement something.\n";
        assert_eq!(parse_target_from_spec(prompt), None);
    }

    // Clause 5: Marker discussed in prose mid-sentence must NOT be read as declaring a target.
    #[test]
    fn clause5_marker_in_prose_is_ignored() {
        let prompt =
            "# Task\nThe dispatcher uses <!-- fb:creates crates/x.rs --> in prompt markers.\n";
        assert_eq!(parse_target_from_spec(prompt), None);
    }

    // Clause 5: Marker inside fenced code blocks is ignored.
    #[test]
    fn clause5_marker_inside_code_fence_is_ignored() {
        let prompt = "# Task\n```markdown\n<!-- fb:creates crates/x.rs -->\n```\n";
        assert_eq!(parse_target_from_spec(prompt), None);
    }

    // Clause 6: A worktree that does not exist returns 2 and does not count as clean.
    #[test]
    fn clause6_missing_worktree_yields_exit_code_2() {
        let assessment = ArmAssessment::WorktreeMissing(PathBuf::from("/fake/path"));
        assert_eq!(aggregate_exit_code(&[assessment]), 2);
    }

    // Clause 7: Git failure returns 2 for that arm.
    #[test]
    fn clause7_git_failure_yields_exit_code_2() {
        let assessment = ArmAssessment::GitFailed {
            command: "git diff --name-only HEAD".to_string(),
            error: "fatal: not a git repository".to_string(),
        };
        assert_eq!(aggregate_exit_code(&[assessment]), 2);
    }

    // Clause 8: With multiple arms, exit code is 1 if ANY departed.
    #[test]
    fn clause8_multiple_arms_one_departed_yields_exit_code_1() {
        let arm1 = ArmAssessment::Clean {
            changed_nothing: false,
        };
        let arm2 = ArmAssessment::Departed {
            scope: Scope {
                target_changed: true,
                allowed: vec![],
                departures: vec![Departure::Foreign {
                    path: "bad.rs".to_string(),
                }],
            },
            lib_change: None,
        };
        let arm3 = ArmAssessment::Clean {
            changed_nothing: true,
        };
        assert_eq!(aggregate_exit_code(&[arm1, arm2, arm3]), 1);
    }

    // Boundary: A worktree with NO changes is clean and distinguishes changed_nothing.
    #[test]
    fn boundary_worktree_changed_nothing_is_clean_and_distinguished() {
        let target = "crates/fb/src/scope_cmd.rs";
        let changes: [Change; 0] = [];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let verdict = evaluate_verdict(&changes, sc, None);
        assert_eq!(
            verdict,
            ArmAssessment::Clean {
                changed_nothing: true
            }
        );
        assert_eq!(aggregate_exit_code(&[verdict]), 0);
    }

    // Boundary: An empty lib.rs diff is LibChange::Untouched, which is clean.
    #[test]
    fn boundary_empty_lib_diff_is_untouched_and_clean() {
        let target = "crates/farmerbob-core/src/x.rs";
        let lib = "crates/farmerbob-core/src/lib.rs";
        let changes = [change(target), change(lib)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let lib_change = classify(&[]);
        assert_eq!(lib_change, LibChange::Untouched);
        let verdict = evaluate_verdict(&changes, sc, Some(lib_change));
        assert_eq!(
            verdict,
            ArmAssessment::Clean {
                changed_nothing: false
            }
        );
        assert_eq!(aggregate_exit_code(&[verdict]), 0);
    }

    // Boundary: Target that is itself a lib.rs has module_declaration_for == None, skipping step 4.
    #[test]
    fn boundary_target_itself_is_lib_rs_skips_step4() {
        let target = "crates/farmerbob-core/src/lib.rs";
        assert_eq!(module_declaration_for(target), None);
        let changes = [change(target)];
        let sc = assess(
            &Declared {
                target: target.to_string(),
            },
            &changes,
        );
        let verdict = evaluate_verdict(&changes, sc, None);
        assert_eq!(
            verdict,
            ArmAssessment::Clean {
                changed_nothing: false
            }
        );
        assert_eq!(aggregate_exit_code(&[verdict]), 0);
    }

    // Boundary: Arm name containing `--` splits on the FIRST `--` after the task prefix.
    #[test]
    fn boundary_arm_name_containing_double_dash() {
        let task = "scope-cmd";
        let dir_name = "scope-cmd--gemini--38--flash";
        assert_eq!(
            parse_arm_name(dir_name, task),
            Some("gemini--38--flash".to_string())
        );

        let multi_dash_task = "my--task";
        let dir_name2 = "my--task--arm--sub";
        assert_eq!(
            parse_arm_name(dir_name2, multi_dash_task),
            Some("arm--sub".to_string())
        );
    }

    // Boundary: Empty assessments list yields exit code 2.
    #[test]
    fn boundary_empty_assessments_yields_exit_code_2() {
        assert_eq!(aggregate_exit_code(&[]), 2);
    }

    // Boundary: Combination of Clean and Unassessable arms yields exit code 2 (not 0).
    #[test]
    fn boundary_clean_and_unassessable_arms_yields_exit_code_2() {
        let clean = ArmAssessment::Clean {
            changed_nothing: false,
        };
        let missing = ArmAssessment::WorktreeMissing(PathBuf::from("/missing"));
        assert_eq!(aggregate_exit_code(&[clean, missing]), 2);
    }

    // Boundary: Combination of Departed and Unassessable arms yields exit code 1.
    #[test]
    fn boundary_departed_and_unassessable_arms_yields_exit_code_1() {
        let departed = ArmAssessment::Departed {
            scope: Scope {
                target_changed: true,
                allowed: vec![],
                departures: vec![Departure::Foreign {
                    path: "bad.rs".to_string(),
                }],
            },
            lib_change: None,
        };
        let missing = ArmAssessment::WorktreeMissing(PathBuf::from("/missing"));
        assert_eq!(aggregate_exit_code(&[departed, missing]), 1);
    }

    // Marker forms: both fb:creates and fb:modifies are accepted at start of line.
    #[test]
    fn marker_forms_both_creates_and_modifies_accepted() {
        let creates_prompt = "<!-- fb:creates crates/fb/src/scope_cmd.rs -->\n# Title\n";
        assert_eq!(
            parse_target_from_spec(creates_prompt),
            Some("crates/fb/src/scope_cmd.rs".to_string())
        );

        let modifies_prompt = "<!-- fb:modifies crates/fb/src/main.rs -->\n# Title\n";
        assert_eq!(
            parse_target_from_spec(modifies_prompt),
            Some("crates/fb/src/main.rs".to_string())
        );
    }

    // Diff line parsing: unified diff header and context lines skipped.
    #[test]
    fn diff_line_parsing_unified_diff() {
        let diff = "\
diff --git a/crates/core/src/lib.rs b/crates/core/src/lib.rs
index 1234567..89abcdef 100644
--- a/crates/core/src/lib.rs
+++ b/crates/core/src/lib.rs
@@ -1,3 +1,4 @@
 pub mod existing;
+pub mod new_mod;
-pub mod old_mod;
";
        let lines = parse_diff_lines(diff);
        assert_eq!(
            lines,
            vec![
                DiffLine {
                    text: "pub mod new_mod;".to_string(),
                    added: true,
                },
                DiffLine {
                    text: "pub mod old_mod;".to_string(),
                    added: false,
                },
            ]
        );
    }

    // build_changes detects deletions when file does not exist on disk.
    #[test]
    fn build_changes_detects_deletions() {
        let tracked = "crates/fb/src/exists.rs\ncrates/fb/src/deleted.rs\n";
        let untracked = "crates/fb/src/untracked.rs\n";
        let changes = build_changes(tracked, untracked, |p| p != "crates/fb/src/deleted.rs");
        assert_eq!(
            changes,
            vec![
                Change {
                    path: "crates/fb/src/exists.rs".to_string(),
                    deleted: false,
                },
                Change {
                    path: "crates/fb/src/deleted.rs".to_string(),
                    deleted: true,
                },
                Change {
                    path: "crates/fb/src/untracked.rs".to_string(),
                    deleted: false,
                },
            ]
        );
    }
}


