//! `fb repro <task> <impl-arm> <suite-arm>` — run one arm's suite against another's code.
//!
//! Ported from fb-repro.sh. One cross-examination cell, by hand, for when a matrix result
//! needs reproducing. The refusal reasons come from `farmerbob_core::graft::plan_cell`, so
//! this and `fb crossx` cannot disagree about whether a cell is buildable -- which is the
//! duplication that produced four separate grafting beads.

use farmerbob_core::graft::{self, Refusal, Source};
use farmerbob_core::target_decl;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Everything but the `#[cfg(test)]` block: an implementation with its own tests stripped,
/// so the only assertions running are the suite arm's.
pub fn without_tests(text: &str) -> String {
    match text.find("#[cfg(test)]") {
        Some(i) => text[..i].to_string(),
        None => text.to_string(),
    }
}

/// The `#[cfg(test)]` block onwards, with its module renamed so it cannot collide with the
/// implementation's own, and the suite file's `use` lines carried in.
///
/// A grafted suite that kept the name `tests` shadowed the module it was grafted beside and
/// the cell compiled against the wrong code.
pub fn suite_block(text: &str) -> String {
    let Some(i) = text.find("#[cfg(test)]") else {
        return String::new();
    };
    let uses: Vec<&str> = text[..i]
        .lines()
        .filter(|l| l.starts_with("use "))
        .collect();
    let mut out = String::new();
    let mut injected = false;
    for line in text[i..].lines() {
        let renamed = if line.trim_start().starts_with("mod tests") {
            line.replacen("mod tests", "mod repro", 1)
        } else {
            line.to_string()
        };
        out.push_str(&renamed);
        out.push('\n');
        if !injected && renamed.trim_start().starts_with("mod repro") {
            for u in &uses {
                out.push_str("    ");
                out.push_str(u);
                out.push('\n');
            }
            injected = true;
        }
    }
    out
}

fn source_of(wt: &Path, target: &str) -> Vec<Source> {
    match fs::read_to_string(wt.join(target)) {
        Ok(text) => vec![Source {
            path: target.to_string(),
            has_tests: text.contains("#[test]"),
        }],
        Err(_) => Vec::new(),
    }
}

/// Reproduce one cell.
pub fn run(task: &str, impl_arm: &str, suite_arm: &str) -> i32 {
    let repo = crate::paths::repo();
    let Ok(spec) = fs::read_to_string(repo.join(".fb/prompts").join(format!("{task}.md"))) else {
        eprintln!("no spec for {task}");
        return 2;
    };
    let target = match target_decl::declared_all(&spec) {
        Ok(ds) if !ds.is_empty() => target_decl::path(&ds[0]).to_string(),
        _ => {
            eprintln!("{task} declares no deliverable");
            return 2;
        }
    };
    let wt = crate::paths::worktrees();
    let iw = wt.join(format!("{task}--{impl_arm}"));
    let sw = wt.join(format!("{task}--{suite_arm}"));
    if let Err(refusal) = graft::plan_cell(
        impl_arm,
        suite_arm,
        &source_of(&iw, &target),
        &source_of(&sw, &target),
        &target,
    ) {
        println!("{}", describe(&refusal, impl_arm, suite_arm, &target));
        return 2;
    }
    let scratch = std::env::temp_dir().join(format!("fb-repro-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    if !Command::new("cp")
        .args(["-r", &iw.to_string_lossy(), &scratch.to_string_lossy()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("cannot copy {}", iw.display());
        return 2;
    }
    let impl_text = fs::read_to_string(iw.join(&target)).unwrap_or_default();
    let suite_text = fs::read_to_string(sw.join(&target)).unwrap_or_default();
    let grafted = format!(
        "{}\n{}",
        without_tests(&impl_text),
        suite_block(&suite_text)
    );
    let _ = fs::write(scratch.join(&target), grafted);
    let krate = target
        .split('/')
        .nth(1)
        .unwrap_or("farmerbob-core")
        .to_string();
    println!("== {suite_arm}'s suite against {impl_arm}'s {target}");
    let out = Command::new("cargo")
        .current_dir(&scratch)
        .args(["test", "-q", "-p", &krate])
        .output();
    match out {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            for line in text.lines().filter(|l| {
                l.starts_with("error")
                    || l.starts_with("---- ")
                    || l.contains("panicked at")
                    || l.contains("assertion")
                    || l.starts_with("test result:")
            }) {
                println!("{line}");
            }
            let _ = fs::remove_dir_all(&scratch);
            i32::from(!o.status.success())
        }
        Err(e) => {
            eprintln!("cannot run cargo: {e}");
            let _ = fs::remove_dir_all(&scratch);
            2
        }
    }
}

fn describe(r: &Refusal, impl_arm: &str, suite_arm: &str, target: &str) -> String {
    match r {
        Refusal::NoImpl => format!("{impl_arm} has no {target}"),
        Refusal::NoSuite => format!("{suite_arm} has no {target}"),
        Refusal::SuiteHasNoTests => {
            format!("{suite_arm}'s {target} carries no tests -- the cell would pass vacuously")
        }
        Refusal::SameArm => {
            "impl and suite are the same arm; that is the diagonal, not a graft".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The implementation's OWN tests are stripped, so the only assertions running belong
    /// to the suite arm. Leaving them in means a cell can pass on the author's own tests.
    #[test]
    fn the_implementations_own_tests_are_stripped() {
        let text = "pub fn f() {}\n#[cfg(test)]\nmod tests { #[test] fn mine() {} }\n";
        let out = without_tests(text);
        assert!(out.contains("pub fn f"));
        assert!(!out.contains("mine"), "{out}");
    }

    /// A file with no test block survives whole -- stripping must not eat an implementation
    /// that simply has no tests.
    #[test]
    fn a_file_without_tests_is_untouched() {
        assert_eq!(without_tests("pub fn f() {}\n"), "pub fn f() {}\n");
    }

    /// The grafted module is RENAMED. A suite that kept the name `tests` shadowed the
    /// module it was grafted beside, and the cell compiled against the wrong code.
    #[test]
    fn the_grafted_module_is_renamed() {
        let suite = "use std::fmt;\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let out = suite_block(suite);
        assert!(out.contains("mod repro"), "{out}");
        assert!(!out.contains("mod tests"), "{out}");
    }

    /// The suite file's `use` lines are carried INTO the renamed module. Dropping them is
    /// bead farmerbob-74l: a suite that failed against its own implementation.
    #[test]
    fn the_suites_imports_are_carried_in() {
        let suite = "use std::fmt;\nuse crate::x;\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let out = suite_block(suite);
        assert!(out.contains("use std::fmt;"), "{out}");
        assert!(out.contains("use crate::x;"), "{out}");
        // and exactly once -- the fix for the dropped imports then injected duplicates,
        // which are a hard error (bead farmerbob-4xv).
        assert_eq!(out.matches("use std::fmt;").count(), 1, "{out}");
    }

    /// A suite arm whose file carries no tests is refused rather than run: the cell would
    /// assert nothing and pass.
    #[test]
    fn a_suite_with_no_tests_is_refused_not_run() {
        assert_eq!(
            graft::plan_cell(
                "a",
                "b",
                &[Source {
                    path: "t.rs".into(),
                    has_tests: true
                }],
                &[Source {
                    path: "t.rs".into(),
                    has_tests: false
                }],
                "t.rs"
            ),
            Err(Refusal::SuiteHasNoTests)
        );
    }
}
