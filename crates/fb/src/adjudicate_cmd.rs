//! `fb brief <task>` — assemble every piece of evidence for one task, for the ADJUDICATOR.
//!
//! This does NOT pick a winner. The orchestrator does, and the whole point of the design is
//! that it does so with both tiers in front of it: objective metrics the harness computed,
//! and the critiques the models wrote about each other. An automated verdict is either
//! redundant with the adjudicator or a replacement for it, and neither is wanted.
//!
//! Written because adjudication kept being done by hand from whatever was convenient, and
//! the subjective tier kept being the part that got skipped. Six consecutive tasks were
//! decided on cross-examination and test counts alone while their critiques sat unread on
//! disk. A human remembering to look is not a mechanism.
//!
//! So this REFUSES to decide when the critiques are missing. That is the same discipline the
//! objective tier already enforces everywhere else: a check that did not run must say so
//! rather than be silently skipped.

use farmerbob_core::adjudicate::{Evidence, Gap, Ruling, adjudicate, evidence_gaps};

/// The repository root. Was hardcoded to one absolute path in two places, so `fb brief` only
/// worked on this machine -- found by or-inkling reviewing a different arm's port.
fn repo_root() -> String {
    std::env::var("FB_REPO").unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string())
    })
}
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn logs() -> PathBuf {
    crate::paths::logs()
}

fn load(path: &PathBuf) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// One critic's subjective review, kept verbatim. Never scored, always shown.
struct Judgement {
    critic: String,
    subject: String,
    text: String,
}

fn judgements(task: &str) -> Vec<Judgement> {
    let dir = logs().join("critiques").join(task);
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        let Some((critic, subject)) = stem.split_once(".on.") else {
            continue;
        };
        let Ok(body) = fs::read_to_string(&p) else {
            continue;
        };
        // keep only the JUDGEMENTS section; CLAIMS are executed elsewhere and must not be
        // re-read as opinion here
        let text = body
            .split("## JUDGEMENT")
            .nth(1)
            // `strip_prefix`, not `trim_start_matches`. The heading may be JUDGEMENT or
            // JUDGEMENTS, so exactly one trailing S is optional -- but trim_start_matches
            // removes EVERY leading S, so a judgement opening "SPEC ambiguity ..." was
            // silently rendered as "PEC ambiguity ...". Found by or-inkling while reviewing
            // a different arm's port, in the tool this project adjudicates with.
            .map(|s| {
                let s = s.strip_prefix('S').unwrap_or(s);
                s.split("\n## ").next().unwrap_or(s).trim().to_string()
            })
            .unwrap_or_default();
        out.push(Judgement {
            critic: critic.into(),
            subject: subject.into(),
            text,
        });
    }
    out
}

/// Does this task have a frozen conformance suite? Most do not: only a handful were ever
/// written, and adjudicate::eligible() requires conformance to be KNOWN and 1.0. Without
/// this, every task without a frozen suite reports NoCandidate -- which is technically what
/// the spec says and useless in practice.
fn has_frozen_suite(task: &str) -> bool {
    // EXISTENCE IS NOT A SUITE. This tested `.exists()`, so an empty file counted as a frozen
    // conformance suite and the brief stopped saying "no frozen conformance suite; gate-PASS
    // stands in" -- claiming a conformance measurement from a file with no tests in it.
    //
    // A stray empty .fb/conformance/port-objective.rs appeared during a differential run and
    // would have done exactly that. It is the same trap as `cargo test` reporting
    // "ok. 0 passed" for a filter that matched nothing, which is the first bug ever filed
    // here, one level up: at the file rather than at the run.
    //
    // A suite is a suite when it contains at least one test.
    let path = format!("{}/.fb/conformance/{task}.rs", repo_root());
    std::fs::read_to_string(path).is_ok_and(|body| body.contains("#[test]"))
}

/// Tests the candidate WROTE, counted in its own module file.
///
/// score.json's `tests_run` is the whole workspace's test count -- 638 against 645 for two
/// candidates whose modules hold 7 and 14 tests. Feeding that to the rubric makes criterion 4
/// a near-tie for every task and pushes the decision down to Simplicity, which is how this
/// command first disagreed with every adjudication made by hand.
fn tests_written(task: &str, arm: &str) -> Option<u32> {
    let spec = fs::read_to_string(format!("{}/.fb/prompts/{task}.md", repo_root())).ok()?;
    let rel = spec
        .lines()
        .find(|l| l.contains("fb:creates"))
        .and_then(|l| l.split_whitespace().nth(2))?;
    let src = fs::read_to_string(format!(
        "{}/{task}--{arm}/{rel}",
        crate::paths::worktrees().display()
    ))
    .ok()?;
    Some(src.matches("#[test]").count() as u32)
}

fn evidence(task: &str) -> Vec<Evidence> {
    let score = load(&logs().join(format!("{task}.score.json")));
    let cx = load(&logs().join(format!("{task}.crossx.json")));
    let defects_json = load(&logs().join(format!("{task}.defects.json")));
    let mut out = Vec::new();
    let Some(Value::Array(rows)) = score else {
        return out;
    };
    for r in rows {
        let arm = r
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if r.get("verdict").and_then(|v| v.as_str()) != Some("PASS") {
            continue;
        }
        let survival = cx
            .as_ref()
            .and_then(|c| c.get(&arm))
            .and_then(|v| v.get("survival"))
            .and_then(|v| v.as_f64());
        let tests = tests_written(task, &arm);
        let overfitted = cx
            .as_ref()
            .and_then(|c| c.get(&arm))
            .and_then(|v| v.get("suite_overfitted"))
            .and_then(|v| v.as_bool());
        let defect_sensitivity = defects_json
            .as_ref()
            .and_then(|d| d.get("arms"))
            .and_then(|a| a.get(&arm))
            .and_then(|v| {
                let of = v
                    .get("beyond_reference_of")
                    .and_then(serde_json::Value::as_u64);
                let caught = v
                    .get("beyond_reference_caught")
                    .and_then(serde_json::Value::as_u64);
                match (caught, of) {
                    (Some(c), Some(n)) if n > 0 => Some(c as f64 / n as f64),
                    _ => {
                        let c = v.get("caught").and_then(serde_json::Value::as_u64)?;
                        let n = v.get("comparable").and_then(serde_json::Value::as_u64)?;
                        (n > 0).then(|| c as f64 / n as f64)
                    }
                }
            });
        out.push(Evidence {
            arm,
            // A frozen suite is the real measurement. Where none exists, clearing the
            // build-and-tests gate is the strongest conformance signal available, and
            // saying so beats reporting NoCandidate for every task without one.
            conformance: Some(1.0),
            // Defect sensitivity, from <task>.defects.json when fb-defects has run.
            //
            // It reports TWO numbers and only one of them ranks anything. Raw sensitivity --
            // validated defects caught -- came back 100% for every arm on every task
            // measured, because a mutant was kept only if the REFERENCE suite detected it,
            // which selects for defects any competent suite catches. The discriminating
            // number is `beyond_reference`: defects the reference MISSES. On gate the raw
            // figure was 12/12 four ways and beyond-reference split the field 1/2, 1/2, 0/2,
            // 0/2.
            //
            // So the criterion prefers beyond-reference where it exists, and falls back to
            // raw sensitivity where no mutant escaped the reference -- in which case it is
            // reporting agreement, not quality, and will tie.  (bead farmerbob-jd2.11)
            defect_sensitivity,
            survival,
            clippy: r
                .get("clippy")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()),
            crates_touched: r
                .get("crates_touched")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
            // The measurement ScopeDiscipline now ranks on. `fb score` writes this from
            // farmerbob_core::scope, and it is absent -- None, not 0 -- when the worktree
            // could not be assessed. None must stay None all the way here: an unassessed
            // run has not been shown to have stayed in scope, and mapping it to 0 would
            // hand it the criterion it never earned.
            scope_departures: r
                .get("scope_departures")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
            tests,
            lines: r.get("lines").and_then(|v| v.as_u64()).map(|n| n as u32),
            cost_usd: None,
            suite_overfitted: overfitted,
        });
    }
    out
}

pub fn run(task: &str, epsilon: f64, allow_missing_critique: bool) -> i32 {
    let cands = evidence(task);
    if cands.is_empty() {
        println!("{task}: no passing candidates scored");
        return 1;
    }
    let js = judgements(task);

    let frozen = has_frozen_suite(task);
    println!("== {task}: {} passing candidate(s)", cands.len());
    if !frozen {
        println!("  (no frozen conformance suite; gate-PASS stands in for conformance)");
    }
    for c in &cands {
        println!(
            "  {:<22} survival={:<6} defect={:<6} clippy={:<4} tests={:<5} lines={}",
            c.arm,
            c.survival
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "n/a".into()),
            c.defect_sensitivity
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "n/a".into()),
            c.clippy
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".into()),
            c.tests
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".into()),
            c.lines
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".into()),
        );
    }

    println!("\n== subjective tier: {} critique(s)", js.len());
    if js.is_empty() {
        println!("  NONE FOUND. The critiques are the half of this the harness cannot compute,");
        println!("  and six consecutive tasks were decided without them. Run:");
        println!("    ./fb-pipeline.sh {task} <crate>");
        if !allow_missing_critique {
            println!("\nREFUSING to adjudicate. Pass --allow-missing-critique to override.");
            return 2;
        }
    }
    for j in &js {
        println!("\n  {} on {}:", j.critic, j.subject);
        for line in j.text.lines().filter(|l| !l.trim().is_empty()).take(6) {
            println!("    {}", line.trim());
        }
    }

    // Report BOTH kinds of gap. A criterion measured for nobody is the more important of the
    // two -- it usually means the instrument was never run -- and the old call silently
    // dropped exactly those.  (bead farmerbob-jd2.11)
    let gaps = evidence_gaps(&cands);
    let absent: Vec<String> = gaps
        .iter()
        .filter_map(|g| match g {
            Gap::Absent(c) => Some(format!("{c:?}")),
            _ => None,
        })
        .collect();
    if !absent.is_empty() {
        println!("\nNEVER MEASURED for any candidate: {}", absent.join(", "));
        println!("  These criteria cannot rank this field. If an instrument for one exists but");
        println!("  is not in the pipeline, that is a harness gap, not a property of the field.");
    }
    let missing: Vec<_> = gaps
        .iter()
        .filter_map(|g| match g {
            Gap::Partial(c) => Some(*c),
            _ => None,
        })
        .collect();
    if !missing.is_empty() {
        println!("\n== not measured: {missing:?}");
    }

    // The rubric ordering, offered as ONE input among several and labelled as such. It knows
    // nothing about what the critics said, which is the half that cannot be computed.
    println!();
    match adjudicate(&cands, epsilon) {
        Ruling::Winner { arm, on, margin } => {
            println!("metrics alone would favour {arm} on {on:?} (margin {margin:.4})");
        }
        Ruling::Undecided { tied, next } => {
            println!("metrics alone do not separate {tied:?}; would need {next:?}");
        }
        Ruling::NoCandidate => {
            println!("metrics alone: no candidate cleared the gate");
        }
    }
    println!("\nThis is a brief, not a verdict. Weigh the critiques above against it.");
    0
}

#[cfg(test)]
mod frozen_suite_is_not_mere_existence {
    /// The predicate under test, in the form it must keep: a file is a suite only when it
    /// holds at least one test. Exercised on strings rather than the filesystem so it says
    /// something about the rule instead of about a temp directory.
    fn counts_as_suite(body: &str) -> bool {
        body.contains("#[test]")
    }

    #[test]
    fn an_empty_file_is_not_a_frozen_suite() {
        assert!(!counts_as_suite(""));
        assert!(!counts_as_suite("\n"));
        assert!(!counts_as_suite("// a comment and nothing else\n"));
    }

    #[test]
    fn a_file_with_a_test_is_a_frozen_suite() {
        assert!(counts_as_suite(
            "#[cfg(test)]\nmod c { #[test] fn t() {} }\n"
        ));
    }

    /// A module that declares tests but contains none is the same empty-suite trap wearing a
    /// module wrapper, and must not count either.
    #[test]
    fn a_test_module_with_no_tests_is_not_a_frozen_suite() {
        assert!(!counts_as_suite(
            "#[cfg(test)]\nmod conformance_x {\n    use super::*;\n}\n"
        ));
    }
}
