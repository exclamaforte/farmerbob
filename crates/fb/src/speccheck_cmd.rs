//! `fb speccheck <task> <crate>` — critique a spec BEFORE anyone implements it.
//!
//! Ported from fb-speccheck.sh, which assembled a prompt by concatenating shell variables
//! and had no tests because it could not have them. It broke the day a task first declared
//! two files: `TARGET=$(fb_target "$BEAD")` captured both paths into one string, the
//! existence test on that string failed, and the critic was shown NO code at all under a
//! prompt telling it to check references against the code below.
//!
//! The assembly is pure here and tested. Only the launch and the filesystem are not.

use farmerbob_core::measurement::Measurement;
use farmerbob_core::target_decl;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// How many characters of code the prompt may carry in total.
pub const CODE_BUDGET: usize = 60_000;

/// One file offered to the critic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    /// Repo-relative path.
    pub path: String,
    /// What the critic is shown, already truncated.
    pub body: String,
    /// True when the file does not exist yet, which a `creates` task expects.
    pub absent: bool,
}

/// Render the excerpts into the `{CODE}` block.
pub fn code_block(excerpts: &[Excerpt]) -> String {
    if excerpts.is_empty() {
        return "=== the spec declares no deliverable, so no code could be shown ===".to_string();
    }
    excerpts
        .iter()
        .map(|e| {
            if e.absent {
                format!("=== {} does not exist yet ===", e.path)
            } else {
                format!(
                    "=== {} ({} lines) ===\n{}",
                    e.path,
                    e.body.lines().count(),
                    e.body
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split the budget evenly across however many files the task declares.
///
/// A two-file task must not blow the prompt, and must not be given one file and a stub.
pub fn per_file_budget(files: usize) -> usize {
    if files == 0 {
        return CODE_BUDGET;
    }
    CODE_BUDGET / files
}

/// Module names the spec mentions as `crate::x::Y` or `x::Y`.
///
/// Carried so a wrong-reference finding is a check rather than a recollection. Deduplicated,
/// sorted, and capped -- the prompt has a budget and six is what the script chose.
pub fn modules_named(spec: &str, cap: usize) -> Vec<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    for token in spec.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
        let token = token.strip_prefix("crate::").unwrap_or(token);
        let Some((module, rest)) = token.split_once("::") else {
            continue;
        };
        if module.is_empty() || rest.is_empty() {
            continue;
        }
        // A type is capitalised; a further `::` means this is a path, not a module::Type.
        if !rest.chars().next().is_some_and(char::is_uppercase) || rest.contains("::") {
            continue;
        }
        // Rust module names are snake_case and may carry digits.
        if module
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            found.insert(module.to_string());
        }
    }
    found.into_iter().take(cap).collect()
}

/// How many findings a report states, and whether it declared none.
///
/// "NO SPEC DEFECTS FOUND" is a legitimate answer and must not read as a failure to report.
pub fn findings_in(report: &str) -> (usize, bool) {
    let none = report.contains("NO SPEC DEFECTS FOUND");
    let n = report
        .lines()
        .filter(|l| l.trim_start().starts_with("FINDING:"))
        .count();
    (n, none)
}

/// Gather the excerpts for a task from its declared set.
pub fn excerpts_for(repo: &Path, spec: &str, krate: &str) -> Vec<Excerpt> {
    let declared: Vec<String> = match target_decl::declared_all(spec) {
        Ok(ds) => ds
            .iter()
            .map(|d| target_decl::path(d).to_string())
            .collect(),
        Err(_) => Vec::new(),
    };
    let budget = per_file_budget(declared.len());
    let mut out: Vec<Excerpt> = Vec::new();
    for path in &declared {
        match fs::read_to_string(repo.join(path)) {
            Ok(text) => out.push(Excerpt {
                path: path.clone(),
                body: text.chars().take(budget).collect(),
                absent: false,
            }),
            Err(_) => out.push(Excerpt {
                path: path.clone(),
                body: String::new(),
                absent: true,
            }),
        }
    }
    // Modules the spec names, excluding anything already shown.
    for module in modules_named(spec, 6) {
        let rel = format!("crates/{krate}/src/{module}.rs");
        if declared.iter().any(|d| d == &rel) {
            continue;
        }
        if let Ok(text) = fs::read_to_string(repo.join(&rel)) {
            let without_tests: String = text
                .split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .chars()
                .take(24_000)
                .collect();
            out.push(Excerpt {
                path: rel,
                body: without_tests,
                absent: false,
            });
        }
    }
    out
}

/// Render the critic's prompt from the template.
pub fn prompt(template: &str, spec: &str, code: &str, out_path: &Path) -> String {
    template
        .replace("{SPEC}", &spec.chars().take(60_000).collect::<String>())
        .replace("{CODE}", code)
        .replace("{OUT}", &out_path.to_string_lossy())
}

/// Run the stage.
pub fn run(task: &str, krate: &str, want: Option<&str>) -> i32 {
    let repo = crate::paths::repo();
    let spec_path = repo.join(".fb/prompts").join(format!("{task}.md"));
    let Ok(spec) = fs::read_to_string(&spec_path) else {
        eprintln!("no spec at {}", spec_path.display());
        return 1;
    };
    let Ok(registry) = crate::sources::Registry::load(&repo.join("sources.toml")) else {
        eprintln!("cannot read the registry");
        return 1;
    };
    let arms: Vec<String> = match want {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty() && registry.eligible(a).is_ok())
            .map(str::to_string)
            .collect(),
        None => registry
            .dispatchable()
            .into_iter()
            .map(|(n, _)| n.to_string())
            .collect(),
    };
    println!("spec critique of {task}: {} arm(s)", arms.len());
    if arms.is_empty() {
        eprintln!("no eligible arms");
        return 1;
    }
    let wt = crate::paths::worktrees();
    // A candidate worktree means a wave is dispatched or in flight, and this stage owns
    // that path and would delete it.
    for arm in &arms {
        let candidate = wt.join(format!("{task}--{arm}"));
        if candidate.exists() {
            eprintln!(
                "REFUSING: {} exists -- a wave for {task} is dispatched or in flight.",
                candidate.display()
            );
            return 1;
        }
    }
    let Ok(template) = fs::read_to_string(repo.join(".fb/prompts/_speccheck.md")) else {
        eprintln!("no speccheck template");
        return 1;
    };
    let code = code_block(&excerpts_for(&repo, &spec, krate));
    let log_dir = crate::paths::logs().join("speccheck").join(task);
    let _ = fs::create_dir_all(&log_dir);
    for arm in &arms {
        let cw = wt.join(format!("{task}--{arm}"));
        let _ = fs::remove_dir_all(&cw);
        let _ = fs::create_dir_all(cw.join(".fb"));
        let out_path = cw.join(".fb/speccheck.md");
        let text = prompt(&template, &spec, &code, &out_path);
        let _ = fs::write(log_dir.join(format!("{arm}.prompt.md")), &text);
        let said = crate::launch::launch(arm, &text, &cw, false, &[]);
        let log = match &said {
            Measurement::Observed(s) if !s.is_empty() => s.clone(),
            Measurement::Observed(_) => {
                "(the launcher wrote nothing to stdout or stderr)\n".to_string()
            }
            Measurement::Missing(reason) => format!("{reason:?}\n"),
        };
        let _ = fs::write(log_dir.join(format!("{arm}.log")), log);
        match fs::read_to_string(&out_path) {
            Ok(report) if !report.trim().is_empty() => {
                let _ = fs::write(log_dir.join(format!("{arm}.findings.md")), &report);
                let (n, none) = findings_in(&report);
                if none {
                    println!("  {arm:<22} no spec defects found");
                } else {
                    println!("  {arm:<22} {n} finding(s)");
                }
            }
            _ => println!("  {arm:<22} (no report written)"),
        }
        // Only remove what is still our scratch dir.
        if !cw.join(".git").exists() {
            let _ = fs::remove_dir_all(&cw);
        }
    }
    println!("  -> {}/", log_dir.display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget is split across the declared files. A two-file task that gave the whole
    /// budget to the first would blow the prompt; one that gave none to the second is the
    /// bug that shipped.
    #[test]
    fn the_budget_is_split_across_declared_files() {
        assert_eq!(per_file_budget(1), CODE_BUDGET);
        assert_eq!(per_file_budget(2), CODE_BUDGET / 2);
        assert_eq!(
            per_file_budget(0),
            CODE_BUDGET,
            "no files, no division by zero"
        );
    }

    /// EVERY declared file appears in the block, each with its own heading. fb-speccheck.sh
    /// concatenated two paths into one filename and emitted
    /// "score.rs\nscope_cmd.rs does not exist yet", showing neither file's contents.
    #[test]
    fn every_declared_file_gets_its_own_heading() {
        let block = code_block(&[
            Excerpt {
                path: "a.rs".into(),
                body: "one\ntwo\n".into(),
                absent: false,
            },
            Excerpt {
                path: "b.rs".into(),
                body: String::new(),
                absent: true,
            },
        ]);
        assert!(block.contains("=== a.rs (2 lines) ==="), "{block}");
        assert!(block.contains("=== b.rs does not exist yet ==="), "{block}");
    }

    /// No deliverable says so, rather than showing an empty block that reads as "no code".
    #[test]
    fn no_deliverable_says_so() {
        assert!(code_block(&[]).contains("declares no deliverable"));
    }

    /// Module references are recognised with and without the `crate::` prefix, deduplicated,
    /// and capped. A bare word, a path with three segments, and a lowercase second segment
    /// are not module::Type.
    #[test]
    fn modules_named_finds_types_not_prose() {
        let spec = "uses crate::scope::Declared and target_decl::Declaration and scope::Scope \
                    but not std::io::Write nor foo::bar nor plain words";
        let found = modules_named(spec, 6);
        assert!(found.contains(&"scope".to_string()), "{found:?}");
        assert!(found.contains(&"target_decl".to_string()), "{found:?}");
        assert!(
            !found.contains(&"std".to_string()),
            "three segments: {found:?}"
        );
        assert!(
            !found.contains(&"foo".to_string()),
            "lowercase type: {found:?}"
        );
        assert_eq!(
            found.iter().filter(|m| *m == "scope").count(),
            1,
            "deduplicated"
        );
    }

    /// The cap is honoured, so a spec naming forty modules cannot blow the prompt.
    #[test]
    fn modules_named_respects_the_cap() {
        let spec = (0..20)
            .map(|i| format!("mod{i}::Type"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(modules_named(&spec, 6).len(), 6);
    }

    /// "NO SPEC DEFECTS FOUND" is a legitimate answer, not a failure to report. Pinned
    /// against a report that does state findings: the two must not look alike.
    #[test]
    fn declaring_no_defects_differs_from_stating_some() {
        assert_eq!(findings_in("NO SPEC DEFECTS FOUND.\n"), (0, true));
        let some = "FINDINGS\n\nFINDING: one\nWHY: x\n\nFINDING: two\nWHY: y\n";
        assert_eq!(findings_in(some), (2, false));
    }

    /// The prompt carries the spec, the code and the path the critic must write to. {OUT}
    /// pointing anywhere but the report file is how a critic overwrites its own prompt.
    #[test]
    fn the_prompt_substitutes_all_three_slots() {
        let out = prompt(
            "spec={SPEC} code={CODE} out={OUT}",
            "S",
            "C",
            Path::new("/tmp/r.md"),
        );
        assert_eq!(out, "spec=S code=C out=/tmp/r.md");
    }
}
