//! `fb followups <task>` — read every critic's FOLLOWUPs and say where each one goes.
//!
//! A critic's follow-up is the cheapest work this project can do, and until now it was
//! routed by hand or not at all. `farmerbob_core::ledger` has parsed `FOLLOWUP:` blocks
//! into [`Proposal`]s since it was merged and had ZERO CALLERS, so critics were writing
//! into a format the harness could read while nothing read it. This is the caller.
//!
//! The routing rule, which lives in `ledger::route` rather than here:
//!
//! - every path in `SCOPE:` was declared by the reviewed task -> resume that arm in the
//!   worktree it still owns. The default, and the cheap path: the agent has the file, the
//!   worktree and its own reasoning already loaded.
//! - any path outside -> the backlog group that owns that code. No arm confined to this
//!   task can complete it.
//! - no `SCOPE:` at all -> unroutable, which is not a rejection.
//!
//! This command decides and prints. It does not resume: launching an agent costs money and
//! belongs behind an explicit act, so the resume command is printed for the operator.

use farmerbob_core::ledger::{self, Kind, Route};
use farmerbob_core::measurement::Measurement;
use farmerbob_core::target_decl;
use std::fs;
use std::path::Path;

/// One follow-up, with who raised it and where it goes.
struct Routed {
    critic: String,
    subject: String,
    title: String,
    route: Route,
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
    match target_decl::declared(&text) {
        Ok(declaration) => Measurement::observed(vec![target_decl::path(&declaration).to_string()]),
        Err(reason) => Measurement::nothing_to_measure(&format!(
            "{task}'s spec declares no single deliverable: {reason:?}"
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
            routed.push(Routed {
                critic: critic.clone(),
                subject: subject.clone(),
                title: proposal.title.clone(),
                route: ledger::route(&proposal, &declared),
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
                let (kind, detail) = match &r.route {
                    Route::Resume { scope } => ("resume", scope.join(",")),
                    Route::Backlog { outside, .. } => ("backlog", outside.join(",")),
                    Route::Unroutable => ("unroutable", String::new()),
                };
                format!(
                    r#"{{"critic":"{}","subject":"{}","route":"{}","detail":"{}","title":"{}"}}"#,
                    r.critic,
                    r.subject,
                    kind,
                    detail,
                    r.title.replace('"', "'")
                )
            })
            .collect();
        println!("[{}]", rows.join(","));
        return 0;
    }

    println!("{task}: {} follow-up(s)", routed.len());
    for r in &routed {
        println!();
        println!("  {} on {}", r.critic, r.subject);
        println!("  {}", r.title);
        match &r.route {
            Route::Resume { scope } => {
                println!("  -> RESUME {} (scope: {})", r.subject, scope.join(", "));
                println!(
                    "     . fb-launch.sh; fb_launch {} \"<the follow-up>\" {}/{}--{} continue",
                    r.subject,
                    crate::paths::worktrees().display(),
                    task,
                    r.subject
                );
            }
            Route::Backlog { outside, .. } => {
                println!("  -> BACKLOG (outside the task: {})", outside.join(", "));
                println!("     file it into the group in .fb/BACKLOG.md that owns those files");
            }
            Route::Unroutable => {
                println!("  -> UNROUTABLE: no SCOPE line. Not a rejection; ask the critic.");
            }
        }
    }
    0
}
