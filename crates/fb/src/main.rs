mod adjudicate_cmd;
mod cmd;
mod compare_gather;
mod compare_cmd;
mod prices_cmd;
mod timing_cmd;
mod verify_cmd;
mod critique;
mod crossx;
mod decl_cmd;
mod defects;
mod differential;
mod doctor;
mod eligible;
mod escalate;
mod fate_cmd;
mod import;
mod ledger_cmd;
mod live_cmd;
mod mutants;
mod next_cmd;
mod objective;
mod pareto;
mod park_cmd;
mod paths;
mod promote;
mod prove;
mod reap_cmd;
mod scope_cmd;
mod score;
mod select;
mod sem_cmd;
mod slots_cmd;
mod sources;
mod stage_cmd;
mod status;
mod trial;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// farmerbob: orchestrate AI coding agents at scale.
#[derive(Parser)]
#[command(name = "fb", version, about)]
struct Cli {
    /// Print the doctor report as JSON instead of a table.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Decide a run's verdict from its observations, via farmerbob_core::gate.
    ///
    /// The shell harness reimplemented this rule four times independently and got it wrong
    /// each time -- an empty test suite exits 0, so "nothing broke" read as "it worked".
    /// This is the one implementation. Omit a flag to say NOT MEASURED, which is a
    /// different fact from zero and yields Indeterminate rather than blaming the arm.
    Gate {
        #[arg(long)]
        built: Option<bool>,
        #[arg(long)]
        tests_passed: Option<bool>,
        #[arg(long)]
        tests_run: Option<u32>,
        #[arg(long)]
        lines_added: Option<u32>,
        #[arg(long)]
        target_present: Option<bool>,
        /// Files changed outside the declared deliverable. Omit to say NOT ASSESSED,
        /// which yields Indeterminate -- scope unchecked is not scope found clean.
        #[arg(long)]
        scope_departures: Option<u32>,
    },
    /// Assemble all evidence for a task -- objective metrics AND the critiques -- for the
    /// adjudicator to weigh. Does not pick a winner.
    Brief {
        task: String,
        #[arg(long, default_value_t = 0.001)]
        epsilon: f64,
        /// Decide even when no critique exists. Off by default, deliberately.
        #[arg(long)]
        allow_missing_critique: bool,
    },
    /// Run environment preflight checks and print an actionable report.
    Doctor,
    /// Turn critics' CLAIMs into executed evidence: classify, then test each allegation.
    ///
    /// Ported from fb-promote.sh and its embedded Python. Keeps the three classifications --
    /// CONTRADICTED routes to the task author because the SPEC is underdetermined, and
    /// outranks a demonstrated defect; CONFIRMATORY is a claim that documented correct
    /// behaviour; TESTABLE is a genuine single-sided allegation. Refuses to write an empty
    /// claims file when no critique exists, because "no defect found" and "no review written"
    /// are different facts.
    Promote {
        /// The bead / task whose claims to promote. The shell script takes only this, and
        /// the port matched it rather than inventing parameters it does not use.
        task: String,
    },
    /// Rebuild logs/objective.json.
    Objective {
        #[arg(default_value = "")]
        only: String,
    },
    /// Measure each candidate suite's sensitivity against a set of injected defects.
    ///
    /// Ported from fb-defects.sh. A defect is valid when the reference conformance suite
    /// detects it, BEYOND THE REFERENCE when the reference misses it -- which is the only
    /// figure that separates a field where everyone catches the obvious defects -- and not a
    /// defect at all when it fails to compile.
    Defects {
        task: String,
        #[arg(long = "crate", default_value = score::DEFAULT_CRATE)]
        krate: String,
        target: String,
    },
    /// How many concurrent runs fit, and why -- the caller SlotTable::admit never had.
    ///
    /// fb-admit.sh counts slots against MEASURED usage and then grants every run a larger
    /// hard cap, so the sum of what the machine may be asked for was never bounded
    /// (farmerbob-89j). Here the hard cap IS the slot budget.
    Slots {
        #[arg(long)]
        available_mb: u64,
        #[arg(long)]
        headroom_mb: u64,
        #[arg(long)]
        memory_mb: u64,
        #[arg(long)]
        plan: bool,
    },
    /// Everything the orchestrator needs to decide what to do next, in one call.
    ///
    /// Ported from fb-status.sh. Its stdout is a wire format: the autopilot greps it and a
    /// human reads it, so ordering and wording are the contract, not a display choice.
    Status,
    /// What to look at first, and why.
    Next {
        /// Print at most this many items. 0 prints all.
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// Refuse to dispatch an arm that is absent, disabled, parked, or redundant.
    ///
    /// Ported from fb-eligible.sh, which the dispatcher sources as a predicate. Exit code and
    /// stderr are BOTH the contract: the dispatcher branches on the first, a human reads the
    /// second. Verified against the script on five arms covering every branch.
    Eligible { arm: String },
    /// Run a ported candidate against the script it replaces, and diff.
    ///
    /// The only gate in this harness that is not a gate on form. A candidate that computes
    /// nothing -- and one such was submitted, passing build, scope, lint and its own tests --
    /// satisfies every other check we own. This one it cannot satisfy.
    Differential { task: String },
    /// Decide what should happen to each of a task's confirmed findings.
    ///
    /// The bridge fb-escalate.sh needs: shell asks, farmerbob_core::finding_fate decides.
    Fate {
        /// The adjudicated task.
        task: String,
        /// What the escalated test did against the merged reference.
        #[arg(long)]
        veto: String,
    },
    /// Decide whether a finished run should park its arm, and how wide.
    ///
    /// The first caller of the quota chain. Every rule lives in farmerbob-core; this
    /// reads the run's log and outcome class and refuses to act without --apply.
    Park {
        /// The task the run belongs to.
        task: String,
        /// The arm that ran.
        arm: String,
        /// Write parked_until into sources.toml instead of printing the decision.
        #[arg(long)]
        apply: bool,
        /// Backoff used when the provider states no reset instant.
        #[arg(long, default_value_t = 3600)]
        backoff_secs: u64,
    },
    /// Assess whether a run stayed inside its declared deliverable.
    ///
    /// Joins scope::assess (paths) with lib_diff::classify (what the lib.rs diff did),
    /// two merged gates that had no caller.
    Scope {
        /// The task to assess.
        task: String,
        /// One arm, or every arm of the task when omitted.
        #[arg(long)]
        arm: Option<String>,
    },

    /// Show which worktree directories may be removed, and with --execute, remove them.
    ///
    /// The first caller of the wtreap chain. Every rule lives in farmerbob-core and is
    /// tested there; this reads the real world and refuses to act without --execute.
    Reap {
        /// Perform the admitted steps instead of printing them.
        #[arg(long)]
        execute: bool,
        /// Permit `git worktree remove` on registered directories.
        #[arg(long)]
        unregister: bool,
    },
    /// Record and read credit for spec defects and follow-ups proposed by arms.
    ///
    /// Two stages feed it: the spec critique that runs BEFORE implementation, and the
    /// FOLLOWUPS section of cross-critique. An accepted proposal is a positive signal
    /// about an arm that nothing else here measures. See docs/PROPOSALS.md.
    Ledger {
        /// Append a ruling instead of printing the tally.
        #[arg(long)]
        record: bool,
        /// The arm that made the proposal.
        #[arg(long, default_value = "")]
        arm: String,
        /// The task it was made during. Without --record, filters the tally.
        #[arg(long, default_value = "")]
        task: String,
        /// `spec` or `followup`.
        #[arg(long, default_value = "")]
        kind: String,
        /// `accepted`, `rejected` or `duplicate`.
        #[arg(long, default_value = "")]
        ruling: String,
        /// One line, as the arm wrote it.
        #[arg(long, default_value = "")]
        title: String,
    },
    /// Strip every `#[cfg(test)]` item from every `.rs` file under a directory, in place.
    ///
    /// Exists so `fb-crossx.sh` stops carrying its own awk version of this, which truncated
    /// a file on a quoted `"#[cfg(test)]"` and VOIDed every farmerbob-core matrix.
    StripTests { dir: std::path::PathBuf },
    /// Generate a defect set mechanically, so suite sensitivity can be measured.
    ///
    /// DefectSensitivity is the adjudicator's only direct measure of suite quality and has
    /// been available for one task in forty-eight, because the defect sets were hand-written.
    Mutants {
        /// The source file to mutate, repo-relative.
        file: String,
        /// The task whose defect set this becomes.
        task: String,
        /// How many defects to emit. Each costs a suite run per candidate.
        #[arg(long, default_value_t = 0)]
        max: usize,
    },
    /// Execute each promoted CLAIM as a test, and veto the ones the reference also fails.
    ///
    /// Ported from fb-prove.sh. A test that also fails the merged reference is testing a
    /// misreading of the spec, not a defect, so it is vetoed rather than counted.
    Prove {
        /// The bead / task whose claims to prove.
        task: String,
        /// The crate under test.
        #[arg(long = "crate", default_value = score::DEFAULT_CRATE)]
        krate: String,
        /// The declared deliverable, repo-relative.
        target: String,
        /// The arm that writes and runs the proofs, matching the script's fourth argument.
        #[arg(default_value = "")]
        prover: String,
    },
    /// Promote a confirmed finding into the permanent suite, and credit the critic.
    ///
    /// Ported from fb-escalate.sh. Subcommands: crossx, auto, verify, credit, ledger.
    Escalate {
        /// One of: crossx, auto, verify, credit, ledger.
        command: String,
        /// The task, where the subcommand takes one.
        #[arg(default_value = "")]
        task: String,
        #[arg(default_value = "")]
        critic: String,
        #[arg(default_value = "")]
        subject: String,
        #[arg(default_value = "")]
        n: String,
    },
    /// Cross-examine: run every candidate's suite against every candidate's implementation.
    ///
    /// Ported from fb-crossx.sh. Keeps the diagonal invariant -- a suite that cannot run
    /// against the code it shipped with VOIDs the whole matrix rather than reporting three
    /// usable rows -- and the spec-ambiguous partition, which outranks a demonstrated defect.
    Crossx {
        /// The bead / task whose candidates cross-examine each other.
        task: String,
        /// The crate under examination.
        #[arg(long = "crate", default_value = score::DEFAULT_CRATE)]
        krate: String,
        /// The declared deliverable, repo-relative.
        target: String,
    },
    /// Cross-review: each implementer critiques ANOTHER implementer's patch.
    ///
    /// Ported from fb-critique.sh, which the shell now delegates to. The derangement, the
    /// patch assembly, the scope report and the "(no critique written)" distinction all
    /// reproduce the script's behaviour, including its wire format, because other scripts
    /// grep that output.
    Critique {
        /// The bead / task whose candidates review each other.
        task: String,
        /// The crate under review.
        #[arg(long = "crate", default_value = score::DEFAULT_CRATE)]
        krate: String,
        /// The declared deliverable, repo-relative.
        target: String,
    },
    /// Choose which arms attempt a task, by Thompson sampling over their posteriors.
    ///
    /// Gives farmerbob_core::router its first caller. Arms were hand-picked before this, and
    /// the same four rotated through seven consecutive waves while three free routes went
    /// untried. Eligibility is a filter and never evidence: a parked or redundant arm is
    /// skipped without its posterior being touched.
    Select {
        /// How many arms to field.
        #[arg(long, default_value_t = 4)]
        n: usize,
        /// Seed the draws, so a selection can be replayed. Defaults to the clock.
        #[arg(long)]
        seed: Option<u64>,
        /// Capabilities the task requires, comma separated, e.g. "multiturn". Empty by
        /// default: the registry records capabilities for 2 of 24 arms, so requiring one
        /// benches every arm nobody has got round to describing.
        #[arg(long, default_value = "")]
        needs: String,
    },
    /// Cost against ability to complete, via farmerbob_core::cost and ::pricing.
    ///
    /// An arm whose launcher writes no cost store is UNMEASURED, not free. The shell
    /// aggregated a missing store as 0.0 and crowned one such arm on the frontier.
    Pareto {
        /// Costs within this many dollars per success are treated as indistinguishable.
        #[arg(long, default_value_t = 0.0)]
        epsilon: f64,
    },
    /// Measure every candidate worktree for a task, via farmerbob_core.
    ///
    /// Replaces fb-score.sh. Liveness needs TWO signals -- the task marker AND a launcher
    /// process actually sitting in the worktree -- because an orphaned run leaves the
    /// marker behind forever and then looks live for good.
    Score {
        /// The bead / task name whose worktrees to measure.
        task: String,
        /// The crate to build, test and lint.
        #[arg(long = "crate", default_value = score::DEFAULT_CRATE)]
        krate: String,
    },
    /// Run a full bakeoff: dispatch, score, cross-review, prove claims, report.
    Trial {
        /// Task name; the spec is .fb/prompts/<task>.md
        task: String,
        /// Crate the task targets.
        #[arg(long = "crate")]
        krate: String,
        /// File the task creates or modifies, relative to the repo root.
        #[arg(long)]
        target: String,
        /// Implementers, comma-separated.
        #[arg(long)]
        agents: String,
        /// Arm that turns critics' claims into executed tests.
        #[arg(long, default_value = "glm-53-flash")]
        prover: String,
        /// Resume from a stage: dispatch|score|differential|crossx|critique|promote|prove|report
        #[arg(long, default_value = "dispatch")]
        from: String,
    },
    /// Summarise run history per arm, excluding outcomes that say nothing about the arm.
    Leaderboard {
        /// Directory of harness run records.
        #[arg(long, default_value = "~/.local/share/farmerbob/logs")]
        from: String,
        /// Show the excluded outcomes and why they were excluded.
        #[arg(long)]
        excluded: bool,
    },
    /// List the agent backends farmerbob can dispatch to, and why any are refused.
    Agents {
        /// Include arms that are refused, with the reason.
        #[arg(long)]
        all: bool,
    },
    /// Print what a spec declares: its verb and the file it names.
    Decl {
        /// Path to the spec file.
        spec: PathBuf,
        /// Print only the path.
        #[arg(long, conflicts_with = "verb_only")]
        path_only: bool,
        /// Print only the verb.
        #[arg(long, conflicts_with = "path_only")]
        verb_only: bool,
    },
}

/// Exit codes are part of the contract: the planning agent branches on these without
/// parsing text. 0 ok, 1 error, 2 usage, 3 would-block, 4 quota-limited.
mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
}

fn registry_path() -> std::path::PathBuf {
    std::env::var_os("FB_SOURCES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("sources.toml"))
}

fn agents(all: bool, json: bool) -> i32 {
    let path = registry_path();
    let reg = match sources::Registry::load(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };

    if json {
        let rows: Vec<serde_json::Value> = reg
            .sources
            .iter()
            .filter(|(k, _)| all || reg.eligible(k).is_ok())
            .map(|(k, s)| {
                let el = reg.eligible(k);
                serde_json::json!({
                    "arm": k,
                    "model": s.model,
                    "kind": s.kind,
                    "bucket": s.bucket(),
                    "free": s.is_free(),
                    "price_in": s.price_in,
                    "price_out": s.price_out,
                    "eligible": el.is_ok(),
                    "refused_because": el.reason(),
                })
            })
            .collect();
        let env = serde_json::json!({"schema": "agents", "version": 1, "data": rows});
        match serde_json::to_string_pretty(&env) {
            Ok(t) => println!("{t}"),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
        return exit::OK;
    }

    println!(
        "{:<24} {:<10} {:>8} {:>9}  MODEL",
        "ARM", "BUCKET", "IN", "OUT"
    );
    for (arm, s) in reg.dispatchable() {
        let (i, o) = (s.price_in.unwrap_or(0.0), s.price_out.unwrap_or(0.0));
        let price = if s.is_free() {
            "free".to_string()
        } else {
            format!("{i:.3}")
        };
        let out = if s.is_free() {
            String::new()
        } else {
            format!("{o:.3}")
        };
        println!(
            "{arm:<24} {:<10} {price:>8} {out:>9}  {}",
            s.bucket(),
            s.model
        );
    }
    if all {
        let refused: Vec<(&String, String)> = reg
            .sources
            .keys()
            .filter_map(|k| reg.eligible(k).reason().map(|r| (k, r)))
            .collect();
        if !refused.is_empty() {
            println!("\nrefused:");
            for (arm, why) in refused {
                println!("  {arm:<24} {why}");
            }
        }
    }
    exit::OK
}

fn leaderboard(from: &str, show_excluded: bool, json: bool) -> i32 {
    let dir = if let Some(rest) = from.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h).join(rest),
            None => {
                eprintln!("error: HOME is not set, cannot expand {from}");
                return exit::ERROR;
            }
        }
    } else {
        std::path::PathBuf::from(from)
    };

    let records = match import::load_dir(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };

    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Tally {
        accepted: u32,
        rejected: u32,
        excluded: u32,
        lines: u64,
        seconds: i64,
    }
    let mut per_arm: BTreeMap<String, Tally> = BTreeMap::new();
    let mut excluded_rows: Vec<(String, String, &'static str)> = Vec::new();

    for (_, r) in &records {
        let t = per_arm.entry(r.source.clone()).or_default();
        match r.accepted() {
            Some(true) => {
                t.accepted += 1;
                t.lines += r.lines_added.unwrap_or(0);
                t.seconds += r.duration_s.unwrap_or(0);
            }
            Some(false) => t.rejected += 1,
            None => {
                t.excluded += 1;
                excluded_rows.push((r.source.clone(), r.bead.clone(), r.classify().as_str()));
            }
        }
    }

    if json {
        let rows: Vec<serde_json::Value> = per_arm
            .iter()
            .map(|(arm, t)| {
                let n = t.accepted + t.rejected;
                serde_json::json!({
                    "arm": arm, "accepted": t.accepted, "rejected": t.rejected,
                    "n": n, "excluded": t.excluded,
                    "rate": if n > 0 { Some(t.accepted as f64 / n as f64) } else { None },
                    "lines": t.lines, "seconds": t.seconds,
                })
            })
            .collect();
        let env = serde_json::json!({"schema":"leaderboard","version":1,"data":rows});
        match serde_json::to_string_pretty(&env) {
            Ok(t) => println!("{t}"),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
        return exit::OK;
    }

    println!(
        "{:<24}{:>4}{:>4}{:>5}{:>8}{:>10}{:>9}",
        "ARM", "OK", "NO", "n", "RATE", "LINES", "EXCL"
    );
    let mut rows: Vec<(&String, &Tally)> = per_arm.iter().collect();
    rows.sort_by(|a, b| {
        let r = |t: &Tally| {
            let n = t.accepted + t.rejected;
            if n > 0 {
                t.accepted as f64 / n as f64
            } else {
                -1.0
            }
        };
        r(b.1)
            .partial_cmp(&r(a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.accepted.cmp(&a.1.accepted))
    });
    for (arm, t) in rows {
        let n = t.accepted + t.rejected;
        let rate = if n > 0 {
            format!("{:.0}%", 100.0 * t.accepted as f64 / n as f64)
        } else {
            "n/a".into()
        };
        println!(
            "{arm:<24}{:>4}{:>4}{:>5}{rate:>8}{:>10}{:>9}",
            t.accepted, t.rejected, n, t.lines, t.excluded
        );
    }
    println!(
        "\n{} runs; {} excluded as not-arm-results",
        records.len(),
        excluded_rows.len()
    );
    if show_excluded {
        println!("\nexcluded:");
        for (arm, bead, why) in &excluded_rows {
            println!("  {arm:<24}{bead:<16}{why}");
        }
    }
    exit::OK
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Gate {
            built,
            tests_passed,
            tests_run,
            lines_added,
            target_present,
            scope_departures,
        }) => {
            use farmerbob_core::gate::{Observation, explain, judge, unmeasured};
            let o = Observation {
                built,
                tests_passed,
                tests_run,
                lines_added,
                declared_targets_present: target_present,
                scope_departures,
            };
            let v = judge(&o);
            if cli.json {
                println!(
                    "{{\"verdict\":\"{v:?}\",\"is_pass\":{},\"blames_arm\":{},\"unmeasured\":{:?},\"explain\":{:?}}}",
                    v.is_pass(),
                    v.blames_arm(),
                    unmeasured(&o),
                    explain(&o, v)
                );
            } else {
                println!("{v:?}  {}", explain(&o, v));
            }
            if v.is_pass() { exit::OK } else { exit::ERROR }
        }
        Some(Command::Promote { task }) => promote::run_cmd(&task),
        Some(Command::Objective { only }) => {
            objective::run_cmd(if only.is_empty() { None } else { Some(only) })
        }
        Some(Command::Defects {
            task,
            krate,
            target,
        }) => defects::run_cmd(&task, &krate, &target),
        Some(Command::Slots {
            available_mb,
            headroom_mb,
            memory_mb,
            plan,
        }) => slots_cmd::run_cmd(available_mb, headroom_mb, memory_mb, plan),
        Some(Command::Status) => status::run_cmd(),
        Some(Command::Next { limit }) => {
            // status::observe_live_agents is private to `status` and yields a list of
            // scope names, not a count; within this task's one-file rule the count
            // cannot be re-obtained here, so it is passed as Missing -- never as 0.
            next_cmd::run(
                &next_cmd::Paths {
                    repo: paths::repo(),
                    logs: paths::logs(),
                },
                farmerbob_core::measurement::Measurement::not_attempted(),
                limit,
                &mut std::io::stdout(),
            )
        }
        Some(Command::Eligible { arm }) => eligible::run_cmd(&arm),
        Some(Command::Differential { task }) => differential::run_cmd(&task),
        Some(Command::Fate { task, veto }) => fate_cmd::run_cmd(&task, &veto),
        Some(Command::Park {
            task,
            arm,
            apply,
            backoff_secs,
        }) => park_cmd::run_cmd(&task, &arm, apply, backoff_secs),
        Some(Command::Reap {
            execute,
            unregister,
        }) => reap_cmd::run_cmd(execute, unregister),
        Some(Command::Scope { task, arm }) => scope_cmd::run_cmd(&task, arm.as_deref()),
        Some(Command::Ledger {
            record,
            arm,
            task,
            kind,
            ruling,
            title,
        }) => {
            if record {
                ledger_cmd::record(&arm, &task, &kind, &ruling, &title)
            } else {
                ledger_cmd::show(if task.is_empty() {
                    None
                } else {
                    Some(task.as_str())
                })
            }
        }
        Some(Command::StripTests { dir }) => match crossx::strip_test_modules_in(&dir) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("fb strip-tests: {}: {e}", dir.display());
                1
            }
        },
        Some(Command::Mutants { file, task, max }) => mutants::run_cmd(&file, &task, max),
        Some(Command::Prove {
            task,
            krate,
            target,
            prover,
        }) => prove::run_cmd(&task, &krate, &target, &prover),
        Some(Command::Escalate {
            command,
            task,
            critic,
            subject,
            n,
        }) => escalate::run_cmd(&command, &task, &critic, &subject, &n),
        Some(Command::Crossx {
            task,
            krate,
            target,
        }) => crossx::run_cmd(&task, &krate, &target),
        Some(Command::Critique {
            task,
            krate,
            target,
        }) => critique::run_cmd(&task, &krate, &target),
        Some(Command::Select { n, seed, needs }) => select::run_cmd(
            n,
            seed,
            &needs
                .split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>(),
            cli.json,
        ),
        Some(Command::Pareto { epsilon }) => pareto::run_cmd(epsilon, cli.json),
        Some(Command::Score { task, krate }) => score::run_cmd(&task, &krate, cli.json),
        Some(Command::Brief {
            task,
            epsilon,
            allow_missing_critique,
        }) => adjudicate_cmd::run(&task, epsilon, allow_missing_critique),
        Some(Command::Doctor) => doctor::run(cli.json),
        Some(Command::Agents { all }) => agents(all, cli.json),
        Some(Command::Leaderboard { from, excluded }) => leaderboard(&from, excluded, cli.json),
        Some(Command::Trial {
            task,
            krate,
            target,
            agents,
            prover,
            from,
        }) => match trial::Stage::parse(&from) {
            None => {
                eprintln!("error: unknown stage `{from}`");
                2
            }
            Some(stage) => trial::Trial {
                task,
                krate,
                target,
                arms: agents.split(',').map(str::to_string).collect(),
                prover,
            }
            .run(stage),
        },
        Some(Command::Decl {
            spec,
            path_only,
            verb_only,
        }) => {
            let format = if path_only {
                decl_cmd::Format::PathOnly
            } else if verb_only {
                decl_cmd::Format::VerbOnly
            } else {
                decl_cmd::Format::Line
            };
            decl_cmd::run(&spec, format, &mut std::io::stdout())
        }
        None => {
            println!("fb — farmerbob. Try `fb --help`.");
            exit::OK
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempSpec {
        path: PathBuf,
    }

    impl TempSpec {
        fn new(name: &str, contents: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("fb_decl_wire_test_{}_{}", std::process::id(), name));
            std::fs::write(&path, contents).expect("write temp spec");
            TempSpec { path }
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempSpec {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn run_decl_from_args(args: &[&str]) -> Result<(i32, Vec<u8>), clap::Error> {
        let mut full_args = vec!["fb", "decl"];
        full_args.extend_from_slice(args);
        let cli = Cli::try_parse_from(full_args)?;
        match cli.command {
            Some(Command::Decl {
                spec,
                path_only,
                verb_only,
            }) => {
                let format = if path_only {
                    decl_cmd::Format::PathOnly
                } else if verb_only {
                    decl_cmd::Format::VerbOnly
                } else {
                    decl_cmd::Format::Line
                };
                let mut out = Vec::new();
                let code = decl_cmd::run(&spec, format, &mut out);
                Ok((code, out))
            }
            _ => panic!("expected Decl command"),
        }
    }

    #[test]
    fn clause_1_decl_one_marker_prints_line_and_exits_0() {
        let spec = TempSpec::new("c1", "<!-- fb:modifies crates/fb/src/main.rs -->\n");
        let (rc, stdout) =
            run_decl_from_args(&[spec.path().to_str().unwrap()]).expect("run decl");
        assert_eq!(rc, 0);
        assert_eq!(stdout, b"modifies crates/fb/src/main.rs\n");
    }

    #[test]
    fn clause_2_path_only_and_verb_only_flags() {
        let spec = TempSpec::new("c2", "<!-- fb:creates crates/fb/src/new.rs -->\n");

        let (rc_path, stdout_path) =
            run_decl_from_args(&[spec.path().to_str().unwrap(), "--path-only"])
                .expect("run decl path-only");
        assert_eq!(rc_path, 0);
        assert_eq!(stdout_path, b"crates/fb/src/new.rs\n");

        let (rc_verb, stdout_verb) =
            run_decl_from_args(&[spec.path().to_str().unwrap(), "--verb-only"])
                .expect("run decl verb-only");
        assert_eq!(rc_verb, 0);
        assert_eq!(stdout_verb, b"creates\n");
    }

    #[test]
    fn clause_3_conflicting_flags_exit_2_nothing_on_stdout() {
        let spec = TempSpec::new("c3", "<!-- fb:modifies crates/fb/src/main.rs -->\n");

        let err1 = match run_decl_from_args(&[
            spec.path().to_str().unwrap(),
            "--path-only",
            "--verb-only",
        ]) {
            Err(e) => e,
            Ok(_) => panic!("expected argument conflict error"),
        };
        assert_eq!(err1.exit_code(), 2);
        assert_eq!(err1.kind(), clap::error::ErrorKind::ArgumentConflict);

        let err2 = match run_decl_from_args(&[
            spec.path().to_str().unwrap(),
            "--verb-only",
            "--path-only",
        ]) {
            Err(e) => e,
            Ok(_) => panic!("expected argument conflict error"),
        };
        assert_eq!(err2.exit_code(), 2);
        assert_eq!(err2.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn clause_4_and_5_exit_codes_and_empty_stdout() {
        let markerless = TempSpec::new("c4_none", "no markers in this file\n");
        let (rc_none, stdout_none) =
            run_decl_from_args(&[markerless.path().to_str().unwrap()]).expect("run markerless");
        assert_eq!(rc_none, 2);
        assert!(stdout_none.is_empty(), "exit 2 must print nothing on stdout");

        let ambiguous = TempSpec::new(
            "c4_ambig",
            "<!-- fb:creates a.rs -->\n<!-- fb:modifies b.rs -->\n",
        );
        let (rc_ambig, stdout_ambig) =
            run_decl_from_args(&[ambiguous.path().to_str().unwrap()]).expect("run ambiguous");
        assert_eq!(rc_ambig, 3);
        assert!(stdout_ambig.is_empty(), "exit 3 must print nothing on stdout");

        let nonexistent = std::env::temp_dir()
            .join(format!("fb_decl_nonexistent_{}.md", std::process::id()));
        let (rc_missing, stdout_missing) =
            run_decl_from_args(&[nonexistent.to_str().unwrap()]).expect("run nonexistent");
        assert_eq!(rc_missing, 4);
        assert!(stdout_missing.is_empty(), "exit 4 must print nothing on stdout");
    }

    #[test]
    fn clause_6_slots_subcommand_unchanged() {
        let parsed = match Cli::try_parse_from([
            "fb",
            "slots",
            "--available-mb",
            "1000",
            "--headroom-mb",
            "200",
            "--memory-mb",
            "300",
            "--plan",
        ]) {
            Ok(c) => c,
            Err(e) => panic!("failed to parse slots: {e}"),
        };
        match parsed.command {
            Some(Command::Slots {
                available_mb,
                headroom_mb,
                memory_mb,
                plan,
            }) => {
                assert_eq!(available_mb, 1000);
                assert_eq!(headroom_mb, 200);
                assert_eq!(memory_mb, 300);
                assert!(plan);
                let rc = slots_cmd::run_cmd(available_mb, headroom_mb, memory_mb, plan);
                assert_eq!(rc, 0);
            }
            _ => panic!("expected Slots command"),
        }
    }

    #[test]
    fn boundary_missing_spec_argument_exits_2() {
        let err = match run_decl_from_args(&[]) {
            Err(e) => e,
            Ok(_) => panic!("expected missing argument error"),
        };
        assert_eq!(err.exit_code(), 2);
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn boundary_directory_spec_exits_4() {
        let dir = std::env::temp_dir();
        let (rc, stdout) =
            run_decl_from_args(&[dir.to_str().unwrap()]).expect("run directory spec");
        assert_eq!(rc, 4);
        assert!(stdout.is_empty(), "directory spec must produce empty stdout");
    }

    #[test]
    fn boundary_empty_file_declares_nothing_exits_2() {
        let empty = TempSpec::new("empty", "");
        let (rc, stdout) =
            run_decl_from_args(&[empty.path().to_str().unwrap()]).expect("run empty spec");
        assert_eq!(rc, 2);
        assert!(stdout.is_empty(), "empty spec must produce empty stdout");
    }

    #[test]
    fn boundary_neither_flag_defaults_to_line_format() {
        let spec = TempSpec::new("default_format", "<!-- fb:creates crates/x.rs -->\n");
        let (rc, stdout) =
            run_decl_from_args(&[spec.path().to_str().unwrap()]).expect("run default format");
        assert_eq!(rc, 0);
        assert_eq!(stdout, b"creates crates/x.rs\n");
    }

    // -- fb next -------------------------------------------------------------------------

    struct TempHarness {
        repo: PathBuf,
        logs: PathBuf,
    }

    impl TempHarness {
        /// A readable but idle harness: every directory `next_cmd` reads exists and
        /// holds no prompts, so there is nothing to act on.
        fn idle(label: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let uniq = NEXT.fetch_add(1, Ordering::Relaxed);
            let repo =
                std::env::temp_dir().join(format!("fb_next_wire_{label}_{}", std::process::id()));
            let logs = std::env::temp_dir()
                .join(format!("fb_next_wire_{label}_logs_{}_{}", std::process::id(), uniq));
            std::fs::create_dir_all(repo.join(".fb/prompts")).expect("create prompts dir");
            std::fs::create_dir_all(repo.join(".fb/queue")).expect("create queue dir");
            std::fs::create_dir_all(&logs).expect("create logs dir");
            TempHarness { repo, logs }
        }

        /// A harness with `tasks` prompted tasks, each holding a readable score file,
        /// so there is something to act on.
        fn actionable(label: &str, tasks: usize) -> Self {
            let harness = Self::idle(label);
            for n in 0..tasks {
                let name = format!("task{n:02}");
                std::fs::write(
                    harness.repo.join(".fb/prompts").join(format!("{name}.md")),
                    "spec\n",
                )
                .expect("write prompt");
                std::fs::write(harness.logs.join(format!("{name}.score.json")), "[]")
                    .expect("write score");
            }
            harness
        }
    }

    impl Drop for TempHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.repo);
            let _ = std::fs::remove_dir_all(&self.logs);
        }
    }

    fn parse_next(args: &[&str]) -> Result<usize, clap::Error> {
        let mut full_args = vec!["fb", "next"];
        full_args.extend_from_slice(args);
        let cli = Cli::try_parse_from(full_args)?;
        match cli.command {
            Some(Command::Next { limit }) => Ok(limit),
            _ => panic!("expected Next command"),
        }
    }

    fn run_next(harness: &TempHarness, limit: usize) -> (i32, String) {
        let mut out = Vec::new();
        let code = next_cmd::run(
            &next_cmd::Paths {
                repo: harness.repo.clone(),
                logs: harness.logs.clone(),
            },
            farmerbob_core::measurement::Measurement::not_attempted(),
            limit,
            &mut out,
        );
        (code, String::from_utf8_lossy(&out).into_owned())
    }

    #[test]
    fn clause_1_next_prints_ranked_items_and_exits_0_when_there_is_work() {
        let harness = TempHarness::actionable("clause1", 2);
        let limit = parse_next(&[]).expect("parse bare next");
        assert_eq!(limit, 0, "no --limit flag must default to 0, which prints all");
        let (code, all) = run_next(&harness, limit);
        assert_eq!(code, 0);
        assert!(all.contains("task00"), "{all}");
        assert!(all.contains("task01"), "{all}");
        let (code_capped, capped) = run_next(&harness, 99);
        assert_eq!(code_capped, 0);
        assert_eq!(
            capped, all,
            "a limit larger than the item count prints all of them, no padding"
        );
    }

    #[test]
    fn clause_3_limit_one_prints_one_item_and_limit_zero_prints_all() {
        let harness = TempHarness::actionable("clause3", 2);
        let limit = parse_next(&["--limit", "1"]).expect("parse --limit 1");
        assert_eq!(limit, 1);
        let (code, one) = run_next(&harness, limit);
        assert_eq!(code, 0);
        assert_eq!(one.lines().count(), 1, "{one}");

        let limit_all = parse_next(&["--limit", "0"]).expect("parse --limit 0");
        assert_eq!(limit_all, 0);
        let (code_zero, zero) = run_next(&harness, limit_all);
        assert_eq!(code_zero, 0);
        assert!(zero.lines().count() > 1, "{zero}");
    }

    #[test]
    fn clause_2_idle_exits_1_and_still_prints_a_readable_line() {
        let harness = TempHarness::idle("clause2");
        let (code, out) = run_next(&harness, 0);
        assert_eq!(code, 1);
        assert!(
            !out.trim().is_empty(),
            "nothing to do must still print, not print nothing: {out:?}"
        );
    }

    #[test]
    fn clause_4_unreadable_harness_exits_4_which_differs_from_idle_1() {
        let harness = TempHarness::idle("clause4");
        let (idle_code, _) = run_next(&harness, 0);
        assert_eq!(idle_code, 1);
        let gone = next_cmd::Paths {
            repo: harness.repo.join("removed"),
            logs: harness.logs.join("removed"),
        };
        let mut out = Vec::new();
        let code = next_cmd::run(
            &gone,
            farmerbob_core::measurement::Measurement::not_attempted(),
            0,
            &mut out,
        );
        assert_eq!(code, 4);
        assert_ne!(code, idle_code, "an idle harness and an unreadable one differ");
    }

    #[test]
    fn clause_7_non_numeric_limit_is_claps_own_usage_error_exit_2() {
        let err = match parse_next(&["--limit", "soon"]) {
            Err(e) => e,
            Ok(_) => panic!("expected a clap usage error for a non-numeric --limit"),
        };
        assert_eq!(err.exit_code(), 2);
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);

        let missing_value = match parse_next(&["--limit"]) {
            Err(e) => e,
            Ok(_) => panic!("expected a clap usage error for --limit with no value"),
        };
        assert_eq!(missing_value.exit_code(), 2);
    }
}
