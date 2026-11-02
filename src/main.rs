mod cli;
mod core;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "archex")]
#[command(about = "Architecture explorer", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    Serve,
    Setup {
        #[arg(long)]
        agent: Option<String>,
    },
    Check {
        /// Check one file instead of the whole tree.
        #[arg(long)]
        file_path: Option<String>,
        /// Fail on rules that match nothing, not just on violations.
        #[arg(long)]
        strict: bool,
        /// Record current violations as the baseline instead of failing.
        #[arg(long)]
        update_baseline: bool,
    },
    Rule {
        #[command(subcommand)]
        command: cli::rule::RuleCommands,
    },
    /// Import committed ledger records into the local index
    Sync,
    Decide {
        #[command(subcommand)]
        command: cli::decide::DecideCommands,
    },
    Hook {
        #[command(subcommand)]
        command: cli::hook::HookCommands,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cli::init::run(),
        Commands::Serve => {
            if let Err(e) = cli::serve::run().await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Setup { agent } => {
            if let Err(e) = cli::setup::run(agent) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Check { file_path, strict, update_baseline } => match file_path {
            Some(path) => {
                if let Err(e) = cli::check::run(path) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
            None => match cli::check::run_gate(strict, update_baseline) {
                Ok(true) => {}
                Ok(false) => std::process::exit(1),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            },
        },
        Commands::Rule { command } => {
            if let Err(e) = cli::rule::run(command) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Sync => {
            let root = std::env::current_dir().unwrap_or_else(|_| ".".into());
            if let Err(e) = cli::sync::run(&root) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Decide { command } => {
            if let Err(e) = cli::decide::run(command) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Hook { command } => {
            if let Err(e) = cli::hook::run(command) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}