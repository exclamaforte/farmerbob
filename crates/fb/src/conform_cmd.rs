//! `fb conform <task> <suite.rs>` — run a frozen suite against every candidate.
//!
//! Ported from fb-conform.sh. The classification is the interesting part and is pure:
//! cargo's output decides whether a candidate conformed, deviated from the interface, or
//! never ran the suite at all -- and that last one is a harness fault, not a result.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What running the frozen suite against one candidate showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conformance {
    /// The suite ran and this many tests passed.
    Conformed(u32),
    /// The suite ran and some tests failed.
    Failed(u32, u32),
    /// The suite did not COMPILE against this candidate: it does not have the interface
    /// the specification fixed. Carries what the compiler could not find.
    ApiMismatch(String),
    /// The suite compiled and executed nothing. A harness fault, NOT a result about the
    /// candidate, and reporting it as a zero score would blame the arm for our bug.
    NoTestsRan,
    /// The suite compiled and executed a DIFFERENT number of tests than it declares.
    ///
    /// This was reported as `Failed(passed, expected - passed)` -- tests that never ran,
    /// counted as tests that failed. Those are not the same thing and the difference decides
    /// whose fault it is: a failing test is the candidate's, a test that did not execute is
    /// ours. `farmerbob_core::conform` has drawn that distinction since it merged, as
    /// `Short { expected, executed }`, and this module reimplemented the classification
    /// without it -- one of the 43 unreached core modules (bead farmerbob-oa5w), and this
    /// caller was written in the same session that counted them.
    Short {
        /// Tests the suite declares.
        expected: u32,
        /// Tests that actually executed.
        executed: u32,
    },
    /// The run is still in flight; measuring it would race the arm.
    Live,
    /// The candidate has no such crate to test.
    NoCrate,
}

/// Classify one cargo run.
///
/// `ok` is cargo's exit status, `output` its combined stdout and stderr.
pub fn classify(ok: bool, output: &str, expected: u32) -> Conformance {
    if !ok {
        if output.contains("error[E") || output.contains("cannot find") {
            let mut found: Vec<String> = Vec::new();
            for line in output.lines() {
                for marker in [
                    "cannot find ",
                    "no method named ",
                    "no function or associated item named ",
                ] {
                    if let Some(i) = line.find(marker) {
                        let rest = line[i..].trim().to_string();
                        if !found.contains(&rest) {
                            found.push(rest);
                        }
                    }
                }
            }
            found.truncate(2);
            return Conformance::ApiMismatch(if found.is_empty() {
                "does not match the specified interface".to_string()
            } else {
                found.join("; ")
            });
        }
        let passed = passed_count(output);
        let failed = failed_count(output);
        return Conformance::Failed(passed, failed.max(1));
    }
    // THE DECISION LIVES IN CORE. This module parses cargo's output; what the numbers MEAN
    // is `farmerbob_core::conform::read`, which is where it already was.
    let passed = passed_count(output);
    let failed = failed_count(output);
    let ran = farmerbob_core::conform::Ran {
        expected,
        executed: passed + failed,
        failed,
    };
    match farmerbob_core::conform::read(true, &ran) {
        farmerbob_core::conform::Conformance::Full => Conformance::Conformed(passed),
        farmerbob_core::conform::Conformance::Broke { failed } => {
            Conformance::Failed(passed, failed)
        }
        // Zero executed keeps its own name: "the suite ran nothing" is a different thing to
        // report than "it ran the wrong number", even though core rightly treats both as
        // saying nothing about the arm.
        farmerbob_core::conform::Conformance::Short { executed: 0, .. } => Conformance::NoTestsRan,
        farmerbob_core::conform::Conformance::Short { expected, executed } => {
            Conformance::Short { expected, executed }
        }
        // `read` returns NotGrafted only for `compiled = false`, and the non-compiling paths
        // were handled above.
        farmerbob_core::conform::Conformance::NotGrafted => Conformance::NoTestsRan,
    }
}

fn count_before(output: &str, word: &str) -> u32 {
    for line in output.lines() {
        if let Some(i) = line.find(word) {
            let head = &line[..i];
            if let Some(Ok(n)) = head.split_whitespace().last().map(str::parse::<u32>) {
                return n;
            }
        }
    }
    0
}

fn passed_count(output: &str) -> u32 {
    count_before(output, "passed")
}
fn failed_count(output: &str) -> u32 {
    count_before(output, "failed")
}

/// How many `#[test]` functions a suite declares.
pub fn expected_in(suite: &str) -> u32 {
    suite
        .lines()
        .filter(|l| l.trim().starts_with("#[test]"))
        .count() as u32
}

/// One line of the report.
pub fn render(arm: &str, c: &Conformance, expected: u32) -> String {
    let (result, detail) = match c {
        Conformance::Conformed(n) => ("CONFORM".to_string(), format!("{n}/{expected} passed")),
        Conformance::Failed(p, f) => ("FAIL".to_string(), format!("{p} passed, {f} failed")),
        Conformance::ApiMismatch(w) => ("FAIL".to_string(), format!("API-MISMATCH {w}")),
        Conformance::NoTestsRan => (
            "NO-TESTS".to_string(),
            "suite did not execute -- harness fault, not a result".to_string(),
        ),
        Conformance::Short { expected, executed } => (
            "SHORT".to_string(),
            format!(
                "{executed} of {expected} tests executed -- the suite did not run as \
                 written, so this says nothing about the candidate"
            ),
        ),
        Conformance::Live => ("LIVE".to_string(), String::new()),
        Conformance::NoCrate => ("NO-CRATE".to_string(), String::new()),
    };
    format!("{arm:<24} {result:<9} {detail}")
}

/// Run the suite against every candidate of `task`.
pub fn run(task: &str, krate: &str, suite_path: &Path) -> i32 {
    let Ok(suite) = fs::read_to_string(suite_path) else {
        eprintln!("no suite at {}", suite_path.display());
        return 1;
    };
    let expected = expected_in(&suite);
    let wt_root = crate::paths::worktrees();
    let prefix = format!("{task}--");
    let Ok(entries) = fs::read_dir(&wt_root) else {
        eprintln!("cannot read {}", wt_root.display());
        return 1;
    };
    let scratch = std::env::temp_dir().join(format!("fb-conform-{}", std::process::id()));
    let _ = fs::create_dir_all(&scratch);
    println!("{:<24} {:<9} FAILURES", "ARM", "RESULT");
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for wt in dirs {
        let Some(arm) = wt
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(&prefix))
        else {
            continue;
        };
        if arm.starts_with("critic--") || !wt.is_dir() {
            continue;
        }
        if wt.join(".fb-task.md").is_file() {
            println!("{}", render(arm, &Conformance::Live, expected));
            continue;
        }
        let copy = scratch.join(arm);
        let _ = fs::remove_dir_all(&copy);
        let ok = Command::new("cp")
            .args(["-r", &wt.to_string_lossy(), &copy.to_string_lossy()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let src = copy.join("crates").join(krate).join("src");
        if !ok || !src.is_dir() {
            println!("{}", render(arm, &Conformance::NoCrate, expected));
            continue;
        }
        let lib = src.join("lib.rs");
        let mut text = fs::read_to_string(&lib).unwrap_or_default();
        text.push('\n');
        text.push_str(&suite);
        let _ = fs::write(&lib, text);
        let out = Command::new("cargo")
            .current_dir(&copy)
            .args(["test", "-q", "-p", krate, "fb_conformance"])
            .output();
        let c = match out {
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                classify(o.status.success(), &text, expected)
            }
            Err(e) => Conformance::ApiMismatch(format!("cannot run cargo: {e}")),
        };
        println!("{}", render(arm, &c, expected));
    }
    let _ = fs::remove_dir_all(&scratch);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The suite compiling but executing nothing is a HARNESS fault, not a score of zero.
    /// Reporting it as a result blames the arm for our bug, which is the failure this whole
    /// project exists to remove.
    #[test]
    fn a_suite_that_ran_nothing_is_not_a_zero_score() {
        assert_eq!(
            classify(true, "test result: ok. 0 passed; 0 failed", 5),
            Conformance::NoTestsRan
        );
    }

    /// A suite that does not COMPILE against a candidate means the candidate lacks the
    /// interface the spec fixed, and the compiler's own words say which part.
    #[test]
    fn a_suite_that_will_not_compile_names_what_is_missing() {
        let out = "error[E0425]: cannot find value `Registry` in this scope";
        match classify(false, out, 5) {
            Conformance::ApiMismatch(w) => assert!(w.contains("cannot find value"), "{w}"),
            other => panic!("expected ApiMismatch, got {other:?}"),
        }
    }

    /// Full conformance and partial conformance are different answers over the same
    /// expectation. Pinned as a pair: a run that executes fewer than expected has NOT
    /// conformed, however many it passed.
    ///
    /// CORRECTED 2026-09-19. The second assertion read `Failed(3, 2)` -- two tests that
    /// never executed, recorded as two failures. I wrote that in the same session that
    /// counted `farmerbob_core::conform` among 43 unreached core modules, and core has
    /// drawn the distinction since it merged: a failing test is the candidate's fault, a
    /// test that did not run is ours, and the second "says nothing about the arm".
    #[test]
    fn passing_fewer_than_expected_is_not_conformance() {
        assert_eq!(
            classify(true, "5 passed; 0 failed", 5),
            Conformance::Conformed(5)
        );
        assert_eq!(
            classify(true, "3 passed; 0 failed", 5),
            Conformance::Short {
                expected: 5,
                executed: 3
            }
        );
    }

    /// The expected count comes from the suite's own `#[test]` markers, and a marker must
    /// start the line -- one quoted in a comment is prose.
    #[test]
    fn expected_counts_test_markers_only() {
        let suite = "#[test]\nfn a() {}\n// #[test] in a comment\n  #[test]\nfn b() {}\n";
        assert_eq!(
            expected_in(suite),
            2,
            "leading whitespace counts, prose does not"
        );
    }

    /// Every outcome renders a distinct RESULT word, so no two facts read alike in the
    /// table a human scans.
    #[test]
    fn every_outcome_has_its_own_word() {
        let words: Vec<String> = [
            Conformance::Conformed(1),
            Conformance::Failed(1, 1),
            Conformance::ApiMismatch("x".into()),
            Conformance::NoTestsRan,
            Conformance::Live,
            Conformance::NoCrate,
        ]
        .iter()
        .map(|c| {
            render("a", c, 1)
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_string()
        })
        .collect();
        let mut uniq = words.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(
            uniq.len(),
            5,
            "FAIL is shared by two, the rest distinct: {words:?}"
        );
    }

    /// TESTS THAT NEVER RAN ARE NOT TESTS THAT FAILED. A suite declaring 7 tests of which 5
    /// executed and all passed was reported `Failed(5, 2)` -- two failures that did not
    /// happen. The difference decides whose fault it is: a failing test is the candidate's,
    /// a test that did not execute is ours, and `farmerbob_core::conform` has said so since
    /// it merged.
    #[test]
    fn a_suite_that_ran_short_is_not_reported_as_failures() {
        let out = "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(
            classify(true, out, 7),
            Conformance::Short {
                expected: 7,
                executed: 5
            }
        );
        // And it says so in words, rather than printing a score.
        let line = render(
            "some-arm",
            &Conformance::Short {
                expected: 7,
                executed: 5,
            },
            7,
        );
        assert!(line.contains("SHORT"), "{line}");
        assert!(line.contains("says nothing about the candidate"), "{line}");
    }

    /// A genuine failure is still a genuine failure, and still the candidate's.
    #[test]
    fn real_failures_are_still_attributed_to_the_candidate() {
        let out = "test result: FAILED. 5 passed; 2 failed; 0 ignored; 0 measured";
        assert_eq!(classify(true, out, 7), Conformance::Failed(5, 2));
    }

    /// Every declared test ran and passed.
    #[test]
    fn a_full_suite_conforms() {
        let out = "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured";
        assert_eq!(classify(true, out, 7), Conformance::Conformed(7));
    }

    /// Zero executed keeps its own name: "ran nothing" is worth reporting differently from
    /// "ran the wrong number", though core rightly treats both as saying nothing about the
    /// arm.
    #[test]
    fn a_suite_that_executed_nothing_is_still_no_tests_ran() {
        let out = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out";
        assert_eq!(classify(true, out, 7), Conformance::NoTestsRan);
    }
}
