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
    /// The spec declares no deliverable this module can run against, so no critic can be
    /// shown the code. Carries what the spec DID declare, when it declared something --
    /// "declares nothing" and "declares a path I cannot find a crate in" need different
    /// fixes, and a variant carrying neither reports them identically.
    NoDeclaration {
        /// The declared path, when there was one.
        declared: Option<String>,
    },
}

/// Decide what to do with one task. Pure: the caller supplies what it found on disk.
///
/// THE DECISION LIVES IN `farmerbob_core::backfill`. This used to make it here, and got the
/// crate wrong: it read the second path segment and, when there wasn't one, substituted
///
///     .unwrap_or("farmerbob-core")
///
/// so a spec declaring a root-level file was silently assigned a crate nobody stated, and
/// the tier ran against it. A guessed crate is indistinguishable from a parsed one, which is
/// this project's recurring defect, and core already refused instead -- requiring
/// `crates/<name>/...` and answering `NoTarget { declared }` otherwise. It had no callers.
/// (bead farmerbob-oa5w)
pub fn plan_for(spec: Option<&str>, has_claims: bool) -> Plan {
    use farmerbob_core::backfill::{Candidate, Plan as CorePlan, plan};
    let target = spec
        .and_then(|s| farmerbob_core::target_decl::declared_all(s).ok())
        .and_then(|ds| {
            ds.first()
                .map(|d| farmerbob_core::target_decl::path(d).to_string())
        });
    let candidate = Candidate {
        name: String::new(),
        has_spec: spec.is_some(),
        has_claims,
        target: target.clone(),
    };
    match plan(&candidate) {
        CorePlan::NoSpec => Plan::NoSpec,
        CorePlan::AlreadyDone => Plan::AlreadyDone,
        CorePlan::NoTarget { declared } => Plan::NoDeclaration { declared },
        // `plan` returns Run only when the target parsed, so it is present here.
        CorePlan::Run { krate } => match target {
            Some(target) => Plan::Run { krate, target },
            None => Plan::NoDeclaration { declared: None },
        },
    }
}

/// Run the tier over every named task.
pub fn run(tasks: &[String]) -> i32 {
    let repo = crate::paths::repo();
    let logs = crate::paths::logs();
    // No tasks named means every task with a spec and no claims -- the script carried a
    // hardcoded list of fifteen, which went stale the moment a sixteenth task existed.
    let owned: Vec<String> = if tasks.is_empty() {
        outstanding(&repo.join(".fb/prompts"), &logs)
    } else {
        tasks.to_vec()
    };
    for task in &owned {
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
            Plan::NoDeclaration { declared } => match declared {
                Some(d) => println!(
                    "skip {task}: declared {d:?}, which is not crates/<name>/... so no crate \
                     can be read from it"
                ),
                None => println!("skip {task}: spec declares no deliverable"),
            },
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
        assert_eq!(
            plan_for(Some("# no markers\n"), false),
            Plan::NoDeclaration { declared: None }
        );
    }

    /// A GUESSED CRATE IS INDISTINGUISHABLE FROM A PARSED ONE. This read the second path
    /// segment and substituted `.unwrap_or("farmerbob-core")` when there wasn't one, so a
    /// spec declaring a root-level file was silently assigned a crate nobody stated and the
    /// subjective tier ran against it. `farmerbob_core::backfill` refuses instead, requiring
    /// `crates/<name>/...`, and had no callers.
    #[test]
    fn a_target_with_no_crate_in_it_is_refused_not_guessed() {
        let spec = "<!-- fb:creates main.rs -->\n";
        assert_eq!(
            plan_for(Some(spec), false),
            Plan::NoDeclaration {
                declared: Some("main.rs".to_string())
            },
            "a root-level path names no crate and must not be given one"
        );
    }

    /// The reason survives to the caller: "declares nothing" and "declares something I
    /// cannot read a crate from" need different fixes.
    #[test]
    fn the_two_ways_of_having_no_target_are_told_apart() {
        assert_eq!(
            plan_for(Some("# nothing\n"), false),
            Plan::NoDeclaration { declared: None }
        );
        assert!(matches!(
            plan_for(Some("<!-- fb:creates x/y.rs -->\n"), false),
            Plan::NoDeclaration { declared: Some(_) }
        ));
    }
}
