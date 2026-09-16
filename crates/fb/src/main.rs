mod doctor;

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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Doctor) => std::process::exit(doctor::run(cli.json)),
        None => println!("fb"),
    }
}