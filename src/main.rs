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
        #[arg(long)]
        file_path: String,
    },
    Rule,
    Decision,
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
        Commands::Check { file_path } => {
            if let Err(e) = cli::check::run(file_path) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Rule => {
            if let Err(e) = cli::rule::run() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Decision => {
            if let Err(e) = cli::decision::run() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}