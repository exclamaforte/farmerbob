mod adjudicate_cmd;
mod cmd;
mod critique;
mod doctor;
mod import;
mod pareto;
mod score;
mod select;
mod sources;
mod trial;

use clap::{Parser, Subcommand};

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
        #[arg(long)] built: Option<bool>,
        #[arg(long)] tests_passed: Option<bool>,
        #[arg(long)] tests_run: Option<u32>,
        #[arg(long)] lines_added: Option<u32>,
        #[arg(long)] target_present: Option<bool>,
    },
    /// Assemble all evidence for a task -- objective metrics AND the critiques -- for the
    /// adjudicator to weigh. Does not pick a winner.
    Brief {
        task: String,
        #[arg(long, default_value_t = 0.001)] epsilon: f64,
        /// Decide even when no critique exists. Off by default, deliberately.
        #[arg(long)] allow_missing_critique: bool,
    },
    /// Run environment preflight checks and print an actionable report.
    Doctor,
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
        /// Resume from a stage: dispatch|score|critique|promote|prove|report
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

    println!("{:<24} {:<10} {:>8} {:>9}  {}", "ARM", "BUCKET", "IN", "OUT", "MODEL");
    for (arm, s) in reg.dispatchable() {
        let (i, o) = (s.price_in.unwrap_or(0.0), s.price_out.unwrap_or(0.0));
        let price = if s.is_free() { "free".to_string() } else { format!("{i:.3}") };
        let out = if s.is_free() { String::new() } else { format!("{o:.3}") };
        println!("{arm:<24} {:<10} {price:>8} {out:>9}  {}", s.bucket(), s.model);
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
            Err(e) => { eprintln!("error: {e}"); return exit::ERROR; }
        }
        return exit::OK;
    }

    println!("{:<24}{:>4}{:>4}{:>5}{:>8}{:>10}{:>9}", "ARM", "OK", "NO", "n", "RATE", "LINES", "EXCL");
    let mut rows: Vec<(&String, &Tally)> = per_arm.iter().collect();
    rows.sort_by(|a, b| {
        let r = |t: &Tally| {
            let n = t.accepted + t.rejected;
            if n > 0 { t.accepted as f64 / n as f64 } else { -1.0 }
        };
        r(b.1).partial_cmp(&r(a.1)).unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.accepted.cmp(&a.1.accepted))
    });
    for (arm, t) in rows {
        let n = t.accepted + t.rejected;
        let rate = if n > 0 {
            format!("{:.0}%", 100.0 * t.accepted as f64 / n as f64)
        } else {
            "n/a".into()
        };
        println!("{arm:<24}{:>4}{:>4}{:>5}{rate:>8}{:>10}{:>9}", t.accepted, t.rejected, n, t.lines, t.excluded);
    }
    println!("\n{} runs; {} excluded as not-arm-results", records.len(), excluded_rows.len());
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
        Some(Command::Gate { built, tests_passed, tests_run, lines_added, target_present }) => {
            use farmerbob_core::gate::{judge, unmeasured, explain, Observation};
            let o = Observation {
                built,
                tests_passed,
                tests_run,
                lines_added,
                declared_targets_present: target_present,
            };
            let v = judge(&o);
            if cli.json {
                println!(
                    "{{\"verdict\":\"{v:?}\",\"is_pass\":{},\"blames_arm\":{},\"unmeasured\":{:?},\"explain\":{:?}}}",
                    v.is_pass(), v.blames_arm(), unmeasured(&o), explain(&o, v)
                );
            } else {
                println!("{v:?}  {}", explain(&o, v));
            }
            if v.is_pass() { exit::OK } else { exit::ERROR }
        }
        Some(Command::Critique { task, krate, target }) =>
            critique::run_cmd(&task, &krate, &target),
        Some(Command::Select { n, seed, needs }) => select::run_cmd(
            n,
            seed,
            &needs.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect::<Vec<_>>(),
            cli.json,
        ),
        Some(Command::Pareto { epsilon }) => pareto::run_cmd(epsilon, cli.json),
        Some(Command::Score { task, krate }) => score::run_cmd(&task, &krate, cli.json),
        Some(Command::Brief { task, epsilon, allow_missing_critique }) =>
            adjudicate_cmd::run(&task, epsilon, allow_missing_critique),
        Some(Command::Doctor) => doctor::run(cli.json),
        Some(Command::Agents { all }) => agents(all, cli.json),
        Some(Command::Leaderboard { from, excluded }) => leaderboard(&from, excluded, cli.json),
        Some(Command::Trial { task, krate, target, agents, prover, from }) => {
            match trial::Stage::parse(&from) {
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
            }
        }
        None => {
            println!("fb — farmerbob. Try `fb --help`.");
            exit::OK
        }
    };
    std::process::exit(code);
}