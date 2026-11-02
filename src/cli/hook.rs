use clap::Subcommand;
use crate::core::db::{Db, DB_PATH};
use crate::core::config;
use std::path::Path;

#[derive(Subcommand)]
pub enum HookCommands {
    /// Pre-edit hook: runs architecture check on a file before editing
    PreEdit {
        /// File path to check
        file_path: String,
    },
    /// Post-edit hook: runs architecture check on a file after editing
    PostEdit {
        /// File path to check
        file_path: String,
    },
}

pub fn run(command: HookCommands) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        HookCommands::PreEdit { file_path } => run_hook(file_path, "pre-edit"),
        HookCommands::PostEdit { file_path } => run_hook(file_path, "post-edit"),
    }
}

fn run_hook(file_path: String, _hook_type: &str) -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::current_dir()?;
    let db = Db::shared(Path::new(DB_PATH))?;

    let rel = crate::core::paths::normalize_in(&file_path, &root);
    let absolute_outside = crate::core::paths::looks_absolute(&rel);
    if rel.is_empty() || absolute_outside {
        println!(
            "✓ {}: outside the project — no architectural context",
            file_path
        );
        return Ok(());
    }

    db.init_schema()?;
    if db.get_all_modules()?.is_empty() {
        return Err("no modules in the index. Run `archex init` first.".into());
    }
    crate::core::rule_engine::warn_legacy_db_rules(&db);

    let cfg = config::Config::load(&root).map_err(|e| format!(
        "{}\n\nThe hook needs a committed architecture. Run `archex init` to draft one.",
        e
    ))?;
    let rules = config::load_rules(&root)?;

    let report = crate::core::boundary::check(&db, &cfg, &rules)?;

    let findings: Vec<&crate::core::boundary::Finding> = report
        .findings
        .iter()
        .filter(|f| f.from_file == rel)
        .collect();

    if findings.is_empty() {
        println!("✓ {}: Clean - no architecture violations", rel);
        return Ok(());
    }

    println!(
        "✗ {}: {} architecture violation(s)",
        rel,
        findings.len()
    );
    println!();

    for finding in &findings {
        println!(
            "  {}:{}  [{}] {}",
            finding.from_file, finding.line_number, finding.rule_id, finding.message
        );
        println!(
            "        imports {} -> {}",
            finding.imported_from, finding.to_file
        );
    }

    Ok(())
}