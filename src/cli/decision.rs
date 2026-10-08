use crate::core::db::{Db, DB_PATH};
use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand)]
pub enum DecisionCommands {
    Add {
        #[arg(long, short)]
        title: String,
        #[arg(long, short)]
        context: Option<String>,
        #[arg(long, short)]
        decision: String,
    },
    List {
        #[arg(long, short)]
        limit: Option<usize>,
    },
    Search {
        #[arg(long, short)]
        query: String,
    },
}

pub fn run(command: DecisionCommands) -> Result<(), Box<dyn std::error::Error>> {
    let db = Db::open(Path::new(DB_PATH))?;

    match command {
        DecisionCommands::Add { title, context, decision } => {
            db.insert_decision(&title, context.as_deref(), &decision)?;
            
            println!("[OK] Decision added successfully");
            println!("  Title: {}", title);
            if let Some(c) = &context {
                println!("  Context: {}", c);
            }
            println!("  Decision: {}", decision);
        }

        DecisionCommands::List { limit } => {
            let limit = limit.unwrap_or(10);
            
            let rows = db.list_decisions(limit)?;
            
            if rows.is_empty() {
                println!("No decisions found.");
                return Ok(());
            }
            
            println!("Recent {} decision(s):", rows.len());
            println!();
            
            for (id, title, context, decision, _) in rows {
                println!("[{}] {}", id, title);
                if let Some(c) = context {
                    if !c.is_empty() {
                        println!("  Context: {}", c);
                    }
                }
                println!("  Decision: {}", decision);
                println!();
            }
        }

        DecisionCommands::Search { query } => {
            let rows = db.search_decisions_db(&query)?;
            
            if rows.is_empty() {
                println!("No decisions found matching '{}'", query);
                return Ok(());
            }
            
            println!("{} result(s) for '{}':", rows.len(), query);
            println!();
            
            for (id, title, context, decision, _) in rows {
                println!("[{}] {}", id, title);
                if let Some(c) = context {
                    if !c.is_empty() {
                        println!("  Context: {}", c);
                    }
                }
                println!("  Decision: {}", decision);
                println!();
            }
        }
    }

    Ok(())
}