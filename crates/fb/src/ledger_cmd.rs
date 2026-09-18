//! `fb ledger` — record and read proposal credits.
//!
//! Append-only JSONL at `<logs>/ledger.jsonl`, one ruled proposal per line. Append-only
//! because a credit record that can be rewritten is a credit record nobody can cite: the
//! adjudicator rules once, in the open, and a later change is a new line rather than an edit.
//!
//! The tally itself lives in `farmerbob_core::ledger` and is pure. This file does I/O and
//! formatting and nothing else.

use farmerbob_core::ledger::{Credit, Entry, Kind, Ruling, acceptance_rate, tally};
use farmerbob_core::measurement::Measurement;
use std::io::Write;

fn path() -> std::path::PathBuf {
    crate::paths::logs().join("ledger.jsonl")
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::SpecDefect => "spec",
        Kind::FollowUp => "followup",
    }
}

fn ruling_str(r: Ruling) -> &'static str {
    match r {
        Ruling::Accepted => "accepted",
        Ruling::Rejected => "rejected",
        Ruling::Duplicate => "duplicate",
    }
}

fn parse_kind(s: &str) -> Option<Kind> {
    match s {
        "spec" | "spec-defect" | "specdefect" => Some(Kind::SpecDefect),
        "followup" | "follow-up" => Some(Kind::FollowUp),
        _ => None,
    }
}

fn parse_ruling(s: &str) -> Option<Ruling> {
    match s {
        "accepted" | "accept" => Some(Ruling::Accepted),
        "rejected" | "reject" => Some(Ruling::Rejected),
        "duplicate" | "dup" => Some(Ruling::Duplicate),
        _ => None,
    }
}

/// One field out of a flat JSON object, without a JSON dependency. The writer
/// is `record` above and escapes exactly the characters read back here.
fn field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(
                        u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)?,
                    );
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn read_entries() -> Vec<Entry> {
    let text = match std::fs::read_to_string(path()) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (Some(arm), Some(task), Some(k), Some(r)) = (
            field(line, "arm"),
            field(line, "task"),
            field(line, "kind"),
            field(line, "ruling"),
        ) else {
            continue;
        };
        let (Some(kind), Some(ruling)) = (parse_kind(&k), parse_ruling(&r)) else {
            continue;
        };
        out.push(Entry {
            arm,
            task,
            kind,
            ruling,
            title: field(line, "title").unwrap_or_default(),
        });
    }
    out
}

/// Append one ruled proposal.
#[allow(clippy::too_many_arguments)]
pub fn record(arm: &str, task: &str, kind: &str, ruling: &str, title: &str) -> i32 {
    let (Some(k), Some(r)) = (parse_kind(kind), parse_ruling(ruling)) else {
        eprintln!(
            "fb ledger record: kind must be spec|followup, ruling must be accepted|rejected|duplicate"
        );
        return 2;
    };
    if arm.is_empty() || task.is_empty() || title.trim().is_empty() {
        eprintln!("fb ledger record: arm, task and title are all required and non-empty");
        return 2;
    }
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = format!(
        "{{\"arm\":\"{}\",\"task\":\"{}\",\"kind\":\"{}\",\"ruling\":\"{}\",\"title\":\"{}\"}}\n",
        esc(arm),
        esc(task),
        kind_str(k),
        ruling_str(r),
        esc(title.trim())
    );
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        Ok(mut f) => match f.write_all(line.as_bytes()) {
            Ok(()) => {
                println!(
                    "recorded: {} {} {} on {}",
                    arm,
                    ruling_str(r),
                    kind_str(k),
                    task
                );
                0
            }
            Err(e) => {
                eprintln!("fb ledger record: writing {}: {e}", p.display());
                1
            }
        },
        Err(e) => {
            eprintln!("fb ledger record: opening {}: {e}", p.display());
            1
        }
    }
}

fn rate_cell(c: &Credit, k: Kind) -> String {
    match acceptance_rate(c, k) {
        Measurement::Observed(r) => format!("{:.0}%", r * 100.0),
        Measurement::Missing(_) => "  --".to_string(),
    }
}

/// Print the tally, optionally filtered to one task.
pub fn show(task: Option<&str>) -> i32 {
    let all = read_entries();
    let filtered: Vec<Entry> = match task {
        Some(t) => all.into_iter().filter(|e| e.task == t).collect(),
        None => all,
    };
    if filtered.is_empty() {
        // Not an error and not a zero table: nothing has been ruled yet, and saying so
        // is different from saying every arm scored nothing.
        println!(
            "no proposals recorded{}",
            match task {
                Some(t) => format!(" for {t}"),
                None => String::new(),
            }
        );
        println!(
            "  (fb ledger record --arm X --task T --kind spec|followup --ruling ... --title \"...\")"
        );
        return 0;
    }
    let credits = tally(&filtered);
    println!(
        "{:<22} {:>9} {:>4} {:>5} {:>11} {:>4} {:>5}",
        "ARM", "SPEC-ACC", "DUP", "RATE", "FOLLOWUP-ACC", "DUP", "RATE"
    );
    println!("{}", "-".repeat(68));
    for c in &credits {
        println!(
            "{:<22} {:>9} {:>4} {:>5} {:>11} {:>4} {:>5}",
            c.arm,
            c.spec_accepted,
            c.spec_duplicate,
            rate_cell(c, Kind::SpecDefect),
            c.followups_accepted,
            c.followups_duplicate,
            rate_cell(c, Kind::FollowUp),
        );
    }
    println!();
    println!("  RATE counts Accepted + Duplicate over everything proposed of that kind.");
    println!("  Duplicate means correct but not first, which is not the same as wrong.");
    println!("  `--` is an arm that proposed nothing of that kind: no rate, not a rate of zero.");
    0
}
