//! `fb followups <task>` — read every critic's FOLLOWUPs and say where each one goes.
//!
//! A critic's follow-up is the cheapest work this project can do, and until now it was
//! routed by hand or not at all. `farmerbob_core::ledger` has parsed `FOLLOWUP:` blocks
//! into [`Proposal`]s since it was merged and had ZERO CALLERS, so critics were writing
//! into a format the harness could read while nothing read it. This is the caller.
//!
//! What it gathers, for a judge to rule on:
//!
//! - which named paths the author already owns, and which belong to someone else
//! - whether the author.s worktree still exists, since an in-turn ruling needs one
//!
//! It does NOT decide. The question is how much work the follow-up is, and no file
//! comparison answers that.
//!
//!
//! This command decides and prints. It does not resume: launching an agent costs money and
//! belongs behind an explicit act, so the resume command is printed for the operator.

use farmerbob_core::ledger::{self, Evidence, Kind};
use farmerbob_core::measurement::Measurement;
use farmerbob_core::target_decl;
use std::fs;
use std::path::Path;

/// One follow-up, with who raised it and where it goes.
struct Routed {
    critic: String,
    subject: String,
    title: String,
    why: String,
    evidence: Evidence,
}

fn read_text(path: &Path) -> Measurement<String> {
    match fs::read_to_string(path) {
        Ok(text) => Measurement::observed(text),
        Err(error) => {
            Measurement::instrument_failed(&format!("cannot read {}: {error}", path.display()))
        }
    }
}

/// The files the task's spec declares, as `scope` compares them.
///
/// `Missing` when the spec cannot be read or declares nothing -- which must not collapse
/// into "declares nothing", because an unreadable spec would then route every follow-up to
/// the backlog while looking like a decision.
fn declared_for(task: &str) -> Measurement<Vec<String>> {
    let spec = crate::paths::repo()
        .join(".fb/prompts")
        .join(format!("{task}.md"));
    let text = match read_text(&spec) {
        Measurement::Observed(text) => text,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    // EVERY deliverable, not one. `declared` answers only for the single-deliverable case
    // and reports two markers as `Ambiguous`, which was correct while a task could declare
    // exactly one file and has not been since 2026-09-19. This caller was missed in that
    // migration, so scope-universe -- the first two-deliverable task -- could not have a
    // single follow-up routed: `fb followups scope-universe` refused outright with
    // "declares no single deliverable: Ambiguous([score.rs, scope_cmd.rs])".
    //
    // Two deliverables is not an unreadable spec. It is a spec with two deliverables, and
    // the follow-up router wants the SET, which is exactly what it compares a proposal's
    // touched paths against.
    match target_decl::declared_all(&text) {
        Ok(ds) if !ds.is_empty() => Measurement::observed(
            ds.iter()
                .map(|d| target_decl::path(d).to_string())
                .collect(),
        ),
        Ok(_) => Measurement::nothing_to_measure(&format!("{task}'s spec declares no deliverable")),
        Err(reason) => Measurement::nothing_to_measure(&format!(
            "{task}'s spec declares no readable deliverable: {reason:?}"
        )),
    }
}

/// `<critic>.on.<subject>.md` -> the two arm names.
fn arms_from(filename: &str) -> Option<(String, String)> {
    let stem = filename.strip_suffix(".md")?;
    let (critic, subject) = stem.split_once(".on.")?;
    if critic.is_empty() || subject.is_empty() {
        return None;
    }
    Some((critic.to_string(), subject.to_string()))
}

/// Read every critique for `task` and route its follow-ups.
pub fn run(task: &str, json: bool) -> i32 {
    let declared = match declared_for(task) {
        Measurement::Observed(paths) => paths,
        Measurement::Missing(reason) => {
            eprintln!("cannot read what {task} declared: {reason:?}");
            return 1;
        }
    };
    let dir = crate::paths::logs().join("critiques").join(task);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        // No critiques is not a failure: the stage may simply not have run.
        Err(_) => {
            println!("no critiques for {task} -- run the pipeline first");
            return 4;
        }
    };

    let mut routed: Vec<Routed> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((critic, subject)) = arms_from(&name) else {
            continue;
        };
        let Measurement::Observed(report) = read_text(&entry.path()) else {
            eprintln!("  {critic}: critique unreadable, skipped");
            continue;
        };
        for proposal in ledger::parse(&report) {
            if proposal.kind != Kind::FollowUp {
                continue;
            }
            let worktree = crate::paths::worktrees().join(format!("{task}--{subject}"));
            let why = proposal
                .body
                .lines()
                .find_map(|l| l.trim_start().strip_prefix("WHY:"))
                .unwrap_or("(no WHY given)")
                .trim()
                .to_string();
            routed.push(Routed {
                critic: critic.clone(),
                subject: subject.clone(),
                title: proposal.title.clone(),
                why,
                evidence: ledger::evidence(&proposal, &declared, worktree.is_dir()),
            });
        }
    }

    if routed.is_empty() {
        println!("{task}: no follow-ups were raised");
        return 0;
    }

    if json {
        let rows: Vec<String> = routed
            .iter()
            .map(|r| {
                format!(
                    r#"{{"critic":"{}","subject":"{}","scope":"{}","inside":"{}","outside":"{}","author_worktree":{},"title":"{}"}}"#,
                    r.critic,
                    r.subject,
                    r.evidence.scope.join(","),
                    r.evidence.inside.join(","),
                    r.evidence.outside.join(","),
                    r.evidence.author_worktree,
                    r.title.replace('"', "'")
                )
            })
            .collect();
        println!("[{}]", rows.join(","));
        return 0;
    }

    println!("{task}: {} follow-up(s) awaiting a ruling", routed.len());
    println!();
    println!("  This command GATHERS. It does not rule. The question is how much work each");
    println!("  follow-up is -- a turn, or its own spec and run -- and no file comparison");
    println!("  answers that. An earlier version of this decided by whether the scope fell");
    println!("  inside the task's declared set, and that rule was wrong in both directions:");
    println!("  it called a twenty-site migration in a foreign file a task (right) and an");
    println!("  API redesign in the author's own file a one-turn fix (wrong, and expensive).");

    for r in &routed {
        println!();
        println!("  {} on {}", r.critic, r.subject);
        println!("    ASKS : {}", r.title);
        println!("    WHY  : {}", r.why);
        if r.evidence.scope.is_empty() {
            println!("    SCOPE: (none named -- cannot be placed as written)");
        } else {
            if !r.evidence.inside.is_empty() {
                println!("    OWNED BY THE AUTHOR : {}", r.evidence.inside.join(", "));
            }
            if !r.evidence.outside.is_empty() {
                println!(
                    "    OWNED BY SOMEONE ELSE: {}",
                    r.evidence.outside.join(", ")
                );
            }
        }
        println!(
            "    AUTHOR'S WORKTREE: {}",
            if r.evidence.author_worktree {
                "present -- an in-turn ruling is available"
            } else {
                "GONE -- in-turn is not available whatever the work costs"
            }
        );
        println!("    RULE IT:");
        println!(
            "      in-turn:  fb dispatch {} {task} <a file holding the follow-up> --continue",
            r.subject
        );
        println!("      task:     write a spec into .fb/prompts/ and queue it");
        println!(
            "      then:     fb ledger --record --arm {} --task {task} --kind followup --ruling accepted|rejected|duplicate --title '...'",
            r.critic
        );
    }
    0
}
