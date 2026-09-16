mod doctor;
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

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Doctor) => doctor::run(cli.json),
        Some(Command::Agents { all }) => agents(all, cli.json),
        None => {
            println!("fb — farmerbob. Try `fb --help`.");
            exit::OK
        }
    };
    std::process::exit(code);
}