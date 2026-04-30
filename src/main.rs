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
    }
}