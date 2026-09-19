//! `fb backfill [task...]` — run the subjective tier over tasks that never got one.
//!
//! Ported from fb-backfill.sh. Fifteen tasks were adjudicated on objective metrics alone
//! when waves replaced `fb trial` and the critique/promote/prove stages silently stopped
//! running (bead farmerbob-k9f). This catches up.

use std::fs;
use std::path::Path;

/// Why a task is skipped, or that it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Run the tier for this task against this crate and target.
    Run { krate: String, target: String },
    /// No spec: nothing to critique against.
    NoSpec,
    /// It already has claims, so the tier has run. Re-running would re-spend arms on a
    /// question already answered.
    AlreadyDone,
    /// The spec declares no deliverable, so no critic can be shown the code.
    NoDeclaration,
}

/// Decide what to do with one task. Pure: the caller supplies what it found on disk.
pub fn plan_for(spec: Option<&str>, has_claims: bool) -> Plan {
    let Some(spec) = spec else {
        return Plan::NoSpec;
    };
    if has_claims {
        return Plan::AlreadyDone;
    }
    match farmerbob_core::target_decl::declared_all(spec) {
        Ok(ds) if !ds.is_empty() => {
            let target = farmerbob_core::target_decl::path(&ds[0]).to_string();
            // crates/<name>/src/... -- the crate is the second segment.
            let krate = target
                .split('/')
                .nth(1)
                .unwrap_or("farmerbob-core")
                .to_string();
            Plan::Run { krate, target }
        }
        _ => Plan::NoDeclaration,
    }
}

/// Run the tier over every named task.
pub fn run(tasks: &[String]) -> i32 {
    let repo = crate::paths::repo();
    let logs = crate::paths::logs();
    for task in tasks {
        let spec_path = repo.join(".fb/prompts").join(format!("{task}.md"));
        let spec = fs::read_to_string(&spec_path).ok();
        let has_claims = logs
            .join(format!("{task}.claims.json"))
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        match plan_for(spec.as_deref(), has_claims) {
            Plan::NoSpec => println!("skip {task}: no spec"),
            Plan::AlreadyDone => println!("skip {task}: already has claims"),
            Plan::NoDeclaration => println!("skip {task}: spec declares no deliverable"),
            Plan::Run { krate, target } => {
                println!("======== {task} ({krate}, {target})");
                crate::critique::run_cmd(task, &krate, &target);
                crate::promote::run_cmd(task);
                let prover = crate::prove::default_prover(None);
                crate::prove::run_cmd(task, &krate, &target, &prover);
            }
        }
    }
    println!("======== backfill complete");
    0
}

/// Every task with a spec but no claims, in name order.
pub fn outstanding(prompts: &Path, logs: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(prompts) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(task) = name.strip_suffix(".md") else {
            continue;
        };
        if task.starts_with('_') {
            continue;
        }
        let claims = logs.join(format!("{task}.claims.json"));
        if claims.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            continue;
        }
        out.push(task.to_string());
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task that already has claims is SKIPPED. Re-running the tier re-spends arms on a
    /// question already answered, and this command exists to be run repeatedly.
    #[test]
    fn a_task_with_claims_is_skipped() {
        let spec = "<!-- fb:creates crates/farmerbob-core/src/x.rs -->\n";
        assert_eq!(plan_for(Some(spec), true), Plan::AlreadyDone);
    }

    /// The same spec without claims runs. Pinned as a pair with the test above: one flag
    /// different, and the difference is whether an arm is spent.
    #[test]
    fn the_same_task_without_claims_runs() {
        let spec = "<!-- fb:creates crates/farmerbob-core/src/x.rs -->\n";
        assert_eq!(
            plan_for(Some(spec), false),
            Plan::Run {
                krate: "farmerbob-core".into(),
                target: "crates/farmerbob-core/src/x.rs".into()
            }
        );
    }

    /// The crate is DERIVED from the declared path, not defaulted. `fb score --crate`
    /// defaulting to farmerbob-core measured a crate the candidates never touched
    /// (bead farmerbob-jd2.12).
    #[test]
    fn the_crate_comes_from_the_declared_path() {
        let spec = "<!-- fb:modifies crates/fb/src/score.rs -->\n";
        match plan_for(Some(spec), false) {
            Plan::Run { krate, .. } => assert_eq!(krate, "fb"),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    /// No spec and no declaration are different skips. Both are reasons not to run and they
    /// need different fixes -- one is a missing file, the other a malformed one.
    #[test]
    fn a_missing_spec_and_a_missing_declaration_are_different() {
        assert_eq!(plan_for(None, false), Plan::NoSpec);
        assert_eq!(plan_for(Some("# no markers\n"), false), Plan::NoDeclaration);
    }
}
