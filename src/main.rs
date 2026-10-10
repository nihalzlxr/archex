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
    },
    Rule {
        #[command(subcommand)]
        command: cli::rule::RuleCommands,
    },
    Decision {
        #[command(subcommand)]
        command: cli::decision::DecisionCommands,
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
        Commands::Check { file_path, strict } => match file_path {
            Some(path) => {
                if let Err(e) = cli::check::run(path) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
            None => match cli::check::run_gate(strict) {
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
        Commands::Decision { command } => {
            if let Err(e) = cli::decision::run(command) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}