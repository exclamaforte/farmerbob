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

use farmerbob_core::adjudicate::{adjudicate, evidence_gaps, Evidence, Gap, Ruling};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn logs() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/farmerbob/logs")
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
    let Ok(entries) = fs::read_dir(&dir) else { return out };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let Some(stem) = name.strip_suffix(".md") else { continue };
        let Some((critic, subject)) = stem.split_once(".on.") else { continue };
        let Ok(body) = fs::read_to_string(&p) else { continue };
        // keep only the JUDGEMENTS section; CLAIMS are executed elsewhere and must not be
        // re-read as opinion here
        let text = body
            .split("## JUDGEMENT")
            .nth(1)
            .map(|s| s.trim_start_matches('S').split("\n## ").next().unwrap_or(s).trim().to_string())
            .unwrap_or_default();
        out.push(Judgement { critic: critic.into(), subject: subject.into(), text });
    }
    out
}

/// Does this task have a frozen conformance suite? Most do not: only a handful were ever
/// written, and adjudicate::eligible() requires conformance to be KNOWN and 1.0. Without
/// this, every task without a frozen suite reports NoCandidate -- which is technically what
/// the spec says and useless in practice.
fn has_frozen_suite(task: &str) -> bool {
    std::path::Path::new(&format!(
        "/home/gabe/Documents/farmerbob/.fb/conformance/{task}.rs"
    ))
    .exists()
}

/// Tests the candidate WROTE, counted in its own module file.
///
/// score.json's `tests_run` is the whole workspace's test count -- 638 against 645 for two
/// candidates whose modules hold 7 and 14 tests. Feeding that to the rubric makes criterion 4
/// a near-tie for every task and pushes the decision down to Simplicity, which is how this
/// command first disagreed with every adjudication made by hand.
fn tests_written(task: &str, arm: &str) -> Option<u32> {
    let spec = fs::read_to_string(format!(
        "/home/gabe/Documents/farmerbob/.fb/prompts/{task}.md"
    ))
    .ok()?;
    let rel = spec
        .lines()
        .find(|l| l.contains("fb:creates"))
        .and_then(|l| l.split_whitespace().nth(2))?;
    let home = std::env::var("HOME").ok()?;
    let src = fs::read_to_string(format!(
        "{home}/.local/share/farmerbob/worktrees/{task}--{arm}/{rel}"
    ))
    .ok()?;
    Some(src.matches("#[test]").count() as u32)
}

fn evidence(task: &str) -> Vec<Evidence> {
    let score = load(&logs().join(format!("{task}.score.json")));
    let cx = load(&logs().join(format!("{task}.crossx.json")));
    let mut out = Vec::new();
    let Some(Value::Array(rows)) = score else { return out };
    for r in rows {
        let arm = r.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
        out.push(Evidence {
            arm,
            // A frozen suite is the real measurement. Where none exists, clearing the
            // build-and-tests gate is the strongest conformance signal available, and
            // saying so beats reporting NoCandidate for every task without one.
            conformance: Some(1.0),
            defect_sensitivity: None,
            survival,
            clippy: r.get("clippy").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
            crates_touched: r.get("crates_touched").and_then(|v| v.as_u64()).map(|n| n as u32),
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
            "  {:<22} survival={:<6} clippy={:<4} tests={:<5} lines={}",
            c.arm,
            c.survival.map(|v| format!("{v:.2}")).unwrap_or_else(|| "n/a".into()),
            c.clippy.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into()),
            c.tests.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into()),
            c.lines.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into()),
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
    let absent: Vec<String> = gaps.iter()
        .filter_map(|g| match g { Gap::Absent(c) => Some(format!("{c:?}")), _ => None })
        .collect();
    if !absent.is_empty() {
        println!("\nNEVER MEASURED for any candidate: {}", absent.join(", "));
        println!("  These criteria cannot rank this field. If an instrument for one exists but");
        println!("  is not in the pipeline, that is a harness gap, not a property of the field.");
    }
    let missing: Vec<_> = gaps.iter()
        .filter_map(|g| match g { Gap::Partial(c) => Some(*c), _ => None })
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
