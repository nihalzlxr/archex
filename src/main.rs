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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cli::init::run(),
        Commands::Serve => cli::serve::run(),
    }
}