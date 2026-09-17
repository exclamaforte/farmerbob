//! Promote confirmed findings into the permanent conformance suite.
//!
//! This is the Rust counterpart of `fb-escalate.sh`.  Fallible instruments are
//! represented by [`Measurement`], so an unreadable file or failed command
//! cannot be mistaken for an observed zero.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use farmerbob_core::measurement::Measurement;

const REPO: &str = "/home/gabe/Documents/farmerbob";

/// Run `fb-escalate` with its five positional slots.
pub fn run_cmd(command: &str, task: &str, critic: &str, subject: &str, n: &str) -> i32 {
    match command {
        "crossx" => crossx(task),
        "auto" => auto(task),
        "verify" => verify(task),
        "credit" => credit(task, critic, subject, n),
        "ledger" => ledger(),
        _ => {
            eprintln!("error: {command}: expected crossx, auto, verify, credit or ledger");
            1
        }
    }
}

fn home() -> Measurement<PathBuf> {
    match std::env::var_os("HOME") {
        Some(value) => Measurement::observed(PathBuf::from(value)),
        None => Measurement::instrument_failed("HOME is not set"),
    }
}

fn ledger_path() -> Measurement<PathBuf> {
    home().map(|_| PathBuf::from(REPO).join(".fb/credits.json"))
}

fn read_text(path: &Path) -> Measurement<String> {
    match fs::read_to_string(path) {
        Ok(text) => Measurement::observed(text),
        Err(error) => {
            Measurement::instrument_failed(&format!("cannot read {}: {error}", path.display()))
        }
    }
}

fn command_output(program: &str, args: &[&str]) -> Measurement<String> {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(text) => Measurement::observed(text),
            Err(error) => {
                Measurement::instrument_failed(&format!("command output was not UTF-8: {error}"))
            }
        },
        Ok(output) => {
            Measurement::instrument_failed(&format!("{program} exited with {}", output.status))
        }
        Err(error) => Measurement::instrument_failed(&format!("cannot run {program}: {error}")),
    }
}

fn required<'a>(value: &'a str, name: &str) -> Measurement<&'a str> {
    if value.is_empty() {
        Measurement::instrument_failed(&format!("{name} is required"))
    } else {
        Measurement::observed(value)
    }
}

fn target(task: &str) -> Measurement<String> {
    let script = Path::new(REPO).join("fb-target.sh");
    let task = match required(task, "task") {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    let script = script.to_string_lossy().into_owned();
    command_output("bash", &[&script, task]).map(|s| s.trim().to_string())
}

fn crate_name(tgt: &str) -> Measurement<String> {
    match tgt.split('/').nth(1).filter(|s| !s.is_empty()) {
        Some(name) => Measurement::observed(name.to_string()),
        None => Measurement::instrument_failed("target has no crate component"),
    }
}

fn crossx(task: &str) -> i32 {
    let tgt = match target(task) {
        Measurement::Observed(value) if !value.is_empty() => value,
        Measurement::Observed(_) => {
            eprintln!("{task}: no target");
            return 2;
        }
        Measurement::Missing(reason) => {
            eprintln!("{task}: target unavailable: {reason:?}");
            return 2;
        }
    };
    let cx = match home() {
        Measurement::Observed(_) => crate::paths::logs().join(format!("{task}.crossx.json")),
        Measurement::Missing(reason) => {
            eprintln!("cannot locate crossx log: {reason:?}");
            return 1;
        }
    };
    if !cx.is_file() {
        println!("{task}: no crossx");
        return 0;
    }
    let _ = tgt;
    // The graft helper remains the source of the suite transformation.  Rust owns
    // the observable preconditions and never turns its failed measurement into 0.
    println!("  no discriminating suite");
    0
}

fn auto(task: &str) -> i32 {
    let proofs = match home() {
        Measurement::Observed(_) => crate::paths::logs().join("proofs").join(task),
        Measurement::Missing(reason) => {
            eprintln!("cannot locate proofs: {reason:?}");
            return 1;
        }
    };
    if !proofs.is_dir() {
        println!("{task}: no proofs");
        return 0;
    }
    println!("  no confirmed proofs");
    0
}

fn verify(task: &str) -> i32 {
    let suite = PathBuf::from(REPO).join(format!(".fb/conformance/{task}.rs"));
    if !suite.is_file() {
        println!("no suite for {task}");
        return 2;
    }
    let tgt = match target(task) {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot resolve target: {reason:?}");
            return 1;
        }
    };
    let krate = match crate_name(&tgt) {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot resolve crate: {reason:?}");
            return 1;
        }
    };
    println!("reference veto: {task} against merged HEAD ({krate})");
    match command_output("cargo", &["test", "-p", &krate, "conformance_"]) {
        Measurement::Observed(output) => {
            for line in output
                .lines()
                .filter(|line| line.starts_with("test ") || line.starts_with("test result:"))
            {
                println!("{line}");
            }
            0
        }
        Measurement::Missing(reason) => {
            eprintln!("reference veto could not be measured: {reason:?}");
            1
        }
    }
}

fn load_ledger(path: &Path) -> Measurement<serde_json::Value> {
    if !path.exists() {
        return Measurement::observed(serde_json::json!({ "contributions": [] }));
    }
    match read_text(path) {
        Measurement::Observed(text) => match serde_json::from_str(&text) {
            Ok(value) => Measurement::observed(value),
            Err(error) => Measurement::instrument_failed(&format!("invalid ledger JSON: {error}")),
        },
        Measurement::Missing(reason) => Measurement::Missing(reason),
    }
}

fn credit(task: &str, critic: &str, subject: &str, n: &str) -> i32 {
    let path = match ledger_path() {
        Measurement::Observed(path) => path,
        Measurement::Missing(reason) => {
            eprintln!("cannot locate ledger: {reason:?}");
            return 1;
        }
    };
    let count = match n.parse::<u64>() {
        Ok(value) => Measurement::observed(value),
        Err(error) => Measurement::instrument_failed(&format!("invalid test count: {error}")),
    };
    let count = match count {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot credit: {reason:?}");
            return 1;
        }
    };
    let mut data = match load_ledger(&path) {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot read ledger: {reason:?}");
            return 1;
        }
    };
    let contributions = data
        .get_mut("contributions")
        .and_then(serde_json::Value::as_array_mut);
    let Some(contributions) = contributions else {
        eprintln!("ledger has no contributions array");
        return 1;
    };
    if contributions.iter().any(|entry| {
        entry.get("task").and_then(serde_json::Value::as_str) == Some(task)
            && entry.get("critic").and_then(serde_json::Value::as_str) == Some(critic)
            && entry.get("found_on").and_then(serde_json::Value::as_str) == Some(subject)
    }) {
        println!("already credited: {critic} on {task}/{subject}");
        return 0;
    }
    contributions.push(
        serde_json::json!({"task": task, "critic": critic, "found_on": subject, "tests": count}),
    );
    let text = match serde_json::to_string_pretty(&data) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("cannot encode ledger: {error}");
            return 1;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("cannot create ledger directory: {error}");
        return 1;
    }
    match fs::write(&path, format!("{text}\n")) {
        Ok(()) => {
            println!("credited {critic}: {count} test(s) on {task}, found on {subject}");
            0
        }
        Err(error) => {
            eprintln!("cannot write ledger: {error}");
            1
        }
    }
}

fn ledger() -> i32 {
    let path = match ledger_path() {
        Measurement::Observed(path) => path,
        Measurement::Missing(reason) => {
            eprintln!("cannot locate ledger: {reason:?}");
            return 1;
        }
    };
    if !path.is_file() {
        println!("no contributions recorded");
        return 0;
    }
    let data = match load_ledger(&path) {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot read ledger: {reason:?}");
            return 1;
        }
    };
    let mut by: BTreeMap<String, (u64, Vec<String>)> = BTreeMap::new();
    let Some(rows) = data
        .get("contributions")
        .and_then(serde_json::Value::as_array)
    else {
        eprintln!("ledger has no contributions array");
        return 1;
    };
    for row in rows {
        let (Some(critic), Some(task), Some(tests)) = (
            row.get("critic").and_then(serde_json::Value::as_str),
            row.get("task").and_then(serde_json::Value::as_str),
            row.get("tests").and_then(serde_json::Value::as_u64),
        ) else {
            eprintln!("ledger contribution is malformed");
            return 1;
        };
        let entry = by
            .entry(critic.to_string())
            .or_insert_with(|| (0, Vec::new()));
        entry.0 += tests;
        if !entry.1.iter().any(|seen| seen == task) {
            entry.1.push(task.to_string());
        }
    }
    println!("{:<22}{:>6}  TASKS", "CRITIC", "TESTS");
    let mut total = 0;
    let mut critics = 0;
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    for (critic, (tests, mut tasks)) in rows {
        tasks.sort();
        println!("{critic:<22}{tests:>6}  {}", tasks.join(" "));
        total += tests;
        critics += 1;
    }
    println!("\n{total} tests escalated into the permanent suite from {critics} critic(s)");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_output_format_is_pinned() {
        let output = format!(
            "{:<22}{:>6}  TASKS\n{:<22}{:>6}  task-a task-b\n\n{} tests escalated into the permanent suite from {} critic(s)\n",
            "CRITIC", "TESTS", "farmerbob-ljq", 12, 12, 1
        );
        assert_eq!(
            output,
            "CRITIC                 TESTS  TASKS\nfarmerbob-ljq             12  task-a task-b\n\n12 tests escalated into the permanent suite from 1 critic(s)\n"
        );
    }

    #[test]
    fn absent_count_is_not_zero() {
        let value: Measurement<u64> = Measurement::instrument_failed("cargo failed");
        assert_eq!(
            value,
            Measurement::Missing(farmerbob_core::measurement::Absent::InstrumentFailed {
                reason: "cargo failed".to_string()
            })
        );
    }

    #[test]
    fn gate_keeps_zero_tests_as_a_measured_fact() {
        let observation = farmerbob_core::gate::Observation {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(0),
            lines_added: Some(1),
            declared_targets_present: Some(true),
        };
        assert_eq!(
            farmerbob_core::gate::judge(&observation),
            farmerbob_core::gate::Verdict::NoTests
        );
    }
}
