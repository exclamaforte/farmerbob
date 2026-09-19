//! `fb verifier <task> <crate> <target> <arm>` — have an arm write a suite from the SPEC
//! alone, then run it against every candidate.
//!
//! Ported from fb-verify.sh. The verifier never sees candidate code, which is the whole
//! point: a suite written from the specification cannot be shaped by any implementation,
//! so a candidate that fails it deviated from the spec rather than from a rival's taste.

use std::fs;
use std::path::Path;

/// The spec as a verifier should see it: harness directives removed, and everything from
/// the scoring rubric onwards dropped.
///
/// The rubric describes how CANDIDATES are scored and would tell the verifier to write an
/// implementation. The `fb:` markers would make the dispatcher treat the verifier's prompt
/// as a task declaration.
pub fn spec_for_verifier(spec: &str) -> String {
    let body = spec
        .split("## How this will be scored")
        .next()
        .unwrap_or(spec);
    body.lines()
        .filter(|l| {
            let t = l.trim();
            !(t.starts_with("<!-- fb:creates") || t.starts_with("<!-- fb:modifies"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Render the verifier's prompt.
pub fn prompt(template: &str, spec: &str, target: &str, krate: &str) -> String {
    template
        .replace("{SPEC}", &spec_for_verifier(spec))
        .replace("{TARGET_FILE}", target)
        .replace("{CRATE}", krate)
}

/// Pull the `#[cfg(test)]` block out of what the verifier wrote, renamed and re-imported.
///
/// `use super::*` cannot work: the suite is appended to `lib.rs`, not to the module it was
/// written beside, so `super` is the crate root and the names it wanted are not there.
pub fn extract_suite(written: &str) -> String {
    let Some(i) = written.find("#[cfg(test)]") else {
        return String::new();
    };
    written[i..]
        .lines()
        .map(|l| {
            if l.contains("use super::*;") {
                l.replace("use super::*;", "use crate::*; use crate::vprelude::*;")
            } else if l.trim_start().starts_with("mod ") && l.contains('{') {
                // One name for every verifier suite, so two of them cannot collide when a
                // caller appends more than one.
                let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                format!("{indent}mod verifier {{")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The prelude a verifier's suite may rely on without naming a dependency.
pub const VPRELUDE: &str = "pub mod vprelude { pub use std::path::PathBuf; \
pub use std::time::Duration; pub use chrono::{DateTime, Utc}; pub use uuid::Uuid; }\n";

/// How many tests a suite declares.
pub fn tests_in(suite: &str) -> usize {
    suite
        .lines()
        .filter(|l| l.trim().starts_with("#[test]"))
        .count()
}

/// Run the stage.
pub fn run(task: &str, krate: &str, target: &str, arm: &str) -> i32 {
    let repo = crate::paths::repo();
    let logs = crate::paths::logs();
    let suite_path = logs.join(format!("{task}.verifier-suite.rs"));
    let regen = std::env::var("FB_REGEN").ok().as_deref() == Some("1");

    if regen
        || !suite_path.is_file()
        || fs::metadata(&suite_path).map(|m| m.len()).unwrap_or(0) == 0
    {
        let Ok(spec) = fs::read_to_string(repo.join(".fb/prompts").join(format!("{task}.md")))
        else {
            eprintln!("no spec for {task}");
            return 1;
        };
        let Ok(template) = fs::read_to_string(repo.join(".fb/prompts/_verifier.md")) else {
            eprintln!("no verifier template");
            return 1;
        };
        let text = prompt(&template, &spec, target, krate);
        let prompt_file = logs.join(format!("{task}.verifier-prompt.md"));
        let _ = fs::write(&prompt_file, &text);
        println!("dispatching verifier {arm} on the spec (no candidate code in context)");
        let code = crate::dispatch_cmd::run(arm, &format!("verify-{task}"), &prompt_file, krate);
        if code != 0 {
            eprintln!("verifier dispatch returned {code}; continuing to read what it wrote");
        }
        let wt = crate::paths::worktrees().join(format!("verify-{task}--{arm}"));
        let written = fs::read_to_string(wt.join(target)).unwrap_or_default();
        let suite = extract_suite(&written);
        let _ = fs::write(&suite_path, &suite);
    }

    let suite = fs::read_to_string(&suite_path).unwrap_or_default();
    let n = tests_in(&suite);
    println!(
        "verifier suite: {n} tests, {} lines  -> {}",
        suite.lines().count(),
        suite_path.display()
    );
    if n == 0 {
        println!("verifier produced no tests");
        return 1;
    }
    // Running it against the field is exactly what `fb conform` does.
    let with_prelude = logs.join(format!("{task}.verifier-suite-full.rs"));
    let _ = fs::write(&with_prelude, format!("{VPRELUDE}{suite}"));
    crate::conform_cmd::run(task, krate, &with_prelude)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verifier must not be shown the scoring rubric: it describes how CANDIDATES are
    /// measured and would have the verifier write an implementation instead of a suite.
    #[test]
    fn the_rubric_is_cut_from_the_verifiers_spec() {
        let spec = "# Task\nDo the thing.\n## How this will be scored\nWrite code.\n";
        let out = spec_for_verifier(spec);
        assert!(out.contains("Do the thing"));
        assert!(!out.contains("Write code"), "{out}");
    }

    /// `fb:` markers are stripped, or the dispatcher reads the verifier's own prompt as a
    /// task declaration and checks preconditions against it.
    #[test]
    fn harness_markers_are_stripped() {
        let out = spec_for_verifier("<!-- fb:creates a.rs -->\n# Task\nbody\n");
        assert!(!out.contains("fb:creates"), "{out}");
        assert!(out.contains("# Task"));
    }

    /// `use super::*` cannot work: the suite is appended to lib.rs, not to the module it
    /// was written beside, so `super` is the crate root and the names it wanted are absent.
    #[test]
    fn super_imports_are_rewritten_to_the_crate_root() {
        let written = "fn f() {}\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn t() {}\n}\n";
        let out = extract_suite(written);
        assert!(!out.contains("use super::*;"), "{out}");
        assert!(out.contains("use crate::*;"), "{out}");
        assert!(out.contains("use crate::vprelude::*;"), "{out}");
    }

    /// Every verifier suite gets the SAME module name, so appending two cannot collide on
    /// one identifier.
    #[test]
    fn the_suite_module_is_renamed_to_verifier() {
        let out =
            extract_suite("#[cfg(test)]\nmod whatever_they_called_it {\n#[test]\nfn t(){}\n}\n");
        assert!(out.contains("mod verifier {"), "{out}");
        assert!(!out.contains("whatever_they_called_it"), "{out}");
    }

    /// A file with no test block yields an EMPTY suite, which the caller reports as "the
    /// verifier produced no tests" rather than running zero assertions and calling it a
    /// pass.
    #[test]
    fn no_test_block_yields_no_suite() {
        assert_eq!(extract_suite("fn f() {}\n"), "");
        assert_eq!(tests_in(""), 0);
    }

    /// The prompt carries the spec, the target and the crate.
    #[test]
    fn the_prompt_substitutes_every_slot() {
        let out = prompt("s={SPEC} t={TARGET_FILE} c={CRATE}", "# S\n", "a.rs", "k");
        assert_eq!(out, "s=# S t=a.rs c=k");
    }
}
