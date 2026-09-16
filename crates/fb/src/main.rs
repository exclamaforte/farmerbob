mod doctor;
mod import;
mod sources;

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
    /// Run environment preflight checks and print an actionable report.
    Doctor,
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
        Some(Command::Doctor) => doctor::run(cli.json),
        Some(Command::Agents { all }) => agents(all, cli.json),
        Some(Command::Leaderboard { from, excluded }) => leaderboard(&from, excluded, cli.json),
        None => {
            println!("fb — farmerbob. Try `fb --help`.");
            exit::OK
        }
    };
    std::process::exit(code);
}