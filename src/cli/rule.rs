use crate::core::db::{Db, RuleType};
use clap::{Parser, Subcommand};
use std::path::Path;

const DB_PATH: &str = ".archex/db.sqlite";

#[derive(Parser)]
pub struct RuleCli {
    #[command(subcommand)]
    command: RuleCommands,
}

#[derive(Subcommand)]
pub enum RuleCommands {
    List {
        #[arg(long, short)]
        module: Option<String>,
    },
    Add {
        #[arg(long, short)]
        module: String,
        #[arg(long, short)]
        rule_type: String,
        #[arg(long, short)]
        description: String,
        #[arg(long, short)]
        pattern: Option<String>,
    },
    Remove {
        #[arg(long)]
        rule_id: i64,
    },
    Test {
        #[arg(long, short)]
        module: String,
        #[arg(long, short)]
        file: String,
    },
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = RuleCli::parse();
    let db = Db::open(Path::new(DB_PATH))?;

    match cli.command {
        RuleCommands::List { module } => {
            if let Some(name) = module {
                let module_id = db.get_module_id_by_name(&name)?
                    .ok_or_else(|| format!("Module '{}' not found", name))?;
                
                let rules = db.get_rules_for_module(module_id)?;
                
                if rules.is_empty() {
                    println!("No rules found for module '{}'", name);
                } else {
                    println!("Rules for module '{}':", name);
                    println!("{:<8} | {:<12} | {:<40} | {}", "ID", "TYPE", "DESCRIPTION", "PATTERN");
                    println!("{}", "-".repeat(80));
                    for r in rules {
                        let pattern_str = r.pattern.as_deref().unwrap_or("-");
                        println!("{:<8} | {:<12} | {:<40} | {}", 
                            r.id, 
                            format!("{:?}", r.rule_type).to_lowercase(),
                            truncate(&r.description, 40),
                            pattern_str
                        );
                    }
                }
            } else {
                let all_modules = db.get_all_modules()?;
                if all_modules.is_empty() {
                    println!("No modules found. Run \"archex init\" first.");
                    return Ok(());
                }
                
                println!("{:<8} | {:<12} | {:<12} | {:<40} | {}", "RULE_ID", "TYPE", "MODULE", "DESCRIPTION", "PATTERN");
                println!("{}", "-".repeat(90));
                
                for m in &all_modules {
                    let module_id = m.id;
                    let rules = db.get_rules_for_module(module_id)?;
                    
                    if rules.is_empty() {
                        println!("{:<8} | {:<12} | {:<12} | {:<40} | {}", 
                            "-", 
                            "-",
                            truncate(&m.name, 12),
                            "(no rules)",
                            "-"
                        );
                    } else {
                        for r in rules {
                            let pattern_str = r.pattern.as_deref().unwrap_or("-");
                            println!("{:<8} | {:<12} | {:<12} | {:<40} | {}", 
                                r.id, 
                                format!("{:?}", r.rule_type).to_lowercase(),
                                truncate(&m.name, 12),
                                truncate(&r.description, 40),
                                pattern_str
                            );
                        }
                    }
                }
            }
        }

        RuleCommands::Add { module, rule_type, description, pattern } => {
            let module_id = db.get_module_id_by_name(&module)?
                .ok_or_else(|| format!("Module '{}' not found", module))?;
            
            let valid_type = match rule_type.to_lowercase().as_str() {
                "forbidden" => RuleType::Forbidden,
                "required" => RuleType::Required,
                "warning" => RuleType::Warning,
                t => return Err(format!("Invalid rule type '{}'. Use: forbidden, required, warning", t).into()),
            };
            
            db.insert_rule(module_id, &rule_type, &description, pattern.as_deref())?;
            
            let count = db.get_rule_count()?;
            println!("[OK] Rule added successfully (ID: {})", count);
            println!("  Module: {}", module);
            println!("  Type: {}", rule_type);
            println!("  Description: {}", description);
        }

        RuleCommands::Remove { rule_id } => {
            print!("Delete rule {}? (y/n): ", rule_id);
            std::io::Write::flush(&mut std::io::stdout())?;
            
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            
            if answer.trim().eq_ignore_ascii_case("y") {
                db.delete_rule(rule_id)?;
                println!("[OK] Rule {} deleted", rule_id);
            } else {
                println!("Cancelled.");
            }
        }

        RuleCommands::Test { module, file } => {
            let module_id = db.get_module_id_by_name(&module)?
                .ok_or_else(|| format!("Module '{}' not found", module))?;
            
            let rules = db.get_rules_for_module(module_id)?;
            
            if rules.is_empty() {
                println!("No rules found for module '{}'", module);
                return Ok(());
            }
            
            let full_path = std::env::current_dir()?.join(&file);
            let content = std::fs::read_to_string(&full_path)?;
            
            println!("Testing {} rule(s) from module '{}' against {}:", rules.len(), module, file);
            println!();
            
            use crate::core::parser::{Parser as ArchexParser, DriftResult, DriftViolation};
            
            let mut drift_result = ArchexParser::check_drift(&file, &content);
            drift_result.module = module.clone();
            
            for rule in &rules {
                let rule_type_str = match rule.rule_type {
                    RuleType::Forbidden => "forbidden",
                    RuleType::Required => "required",
                    RuleType::Warning => "warning",
                };

                if let Some(pattern) = &rule.pattern {
                    let pattern_lower = pattern.to_lowercase();
                    let content_lower = content.to_lowercase();
                    
                    match rule.rule_type {
                        RuleType::Forbidden => {
                            let matches = content_lower.contains(&pattern_lower);
                            if matches {
                                drift_result.violations.push(
                                    DriftViolation {
                                        rule_type: rule_type_str.to_string(),
                                        rule_description: rule.description.clone(),
                                        pattern: Some(pattern.clone()),
                                        line_number: None,
                                        suggestion: "Remove or refactor matching pattern".to_string(),
                                    }
                                );
                            }
                        }
                        RuleType::Required => {
                            let has_pattern = content_lower.contains(&pattern_lower);
                            if !has_pattern {
                                drift_result.violations.push(
                                    DriftViolation {
                                        rule_type: rule_type_str.to_string(),
                                        rule_description: rule.description.clone(),
                                        pattern: Some(pattern.clone()),
                                        line_number: None,
                                        suggestion: format!("Add required pattern: {}", pattern),
                                    }
                                );
                            }
                        }
                        RuleType::Warning => {
                            let matches = content_lower.contains(&pattern_lower);
                            if matches {
                                drift_result.violations.push(
                                    DriftViolation {
                                        rule_type: rule_type_str.to_string(),
                                        rule_description: rule.description.clone(),
                                        pattern: Some(pattern.clone()),
                                        line_number: None,
                                        suggestion: "Consider refactoring".to_string(),
                                    }
                                );
                            }
                        }
                    }
                }
            }
            
            if drift_result.clean {
                println!("[OK] All rules passed - no violations found");
            } else {
                for v in &drift_result.violations {
                    println!("  {}: {}", v.rule_type.to_uppercase(), v.rule_description);
                    if let Some(p) = &v.pattern {
                        println!("    Pattern: {}", p);
                    }
                    println!("    Suggestion: {}", v.suggestion);
                    println!();
                }
            }
        }
    }

    Ok(())
}

fn truncate(s: &str, len: usize) -> String {
    if s.len() > len {
        format!("{}...", &s[..len-3])
    } else {
        s.to_string()
    }
}