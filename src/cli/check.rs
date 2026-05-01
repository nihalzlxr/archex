use crate::core::db::{Db, DB_PATH};
use crate::core::parser::{Parser, DriftResult, DriftViolation};
use regex::Regex;
use std::path::Path;
use crate::core::db::RuleType;

pub fn run(file_path: String) -> Result<(), Box<dyn std::error::Error>> {
    let db = Db::open(Path::new(DB_PATH))?;

    let full_path = if file_path.starts_with('/') || file_path.contains(':') {
        Path::new(&file_path).to_path_buf()
    } else {
        std::env::current_dir()?.join(&file_path)
    };

    let content = std::fs::read_to_string(&full_path)?;
    
    let module_info = db.get_module_for_file(&file_path).ok().and_then(|m| m);
    let module_name = module_info.as_ref().map(|(n, _)| n.clone()).unwrap_or_else(|| "unknown".to_string());

    let rules = if let Some((_, module_id)) = module_info {
        db.get_rules_for_module(module_id).unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut drift_result = Parser::check_drift(&file_path, &content);
    drift_result.module = module_name.clone();

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
                    let regex_result = Regex::new(pattern);
                    let matches = if let Ok(re) = regex_result {
                        re.is_match(&content_lower)
                    } else {
                        content_lower.contains(&pattern_lower)
                    };

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
                    let regex_result = Regex::new(pattern);
                    let has_pattern = if let Ok(re) = regex_result {
                        re.is_match(&content_lower)
                    } else {
                        content_lower.contains(&pattern_lower)
                    };

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
                    let regex_result = Regex::new(pattern);
                    let matches = if let Ok(re) = regex_result {
                        re.is_match(&content_lower)
                    } else {
                        content_lower.contains(&pattern_lower)
                    };

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
        println!("✓ {}: Clean - no forbidden violations found", file_path);
        for v in &drift_result.violations {
            if v.rule_type == "warning" {
                println!("  ⚠ {}: {}", v.rule_type, v.rule_description);
            } else if v.rule_type == "required" {
                println!("  ⚠ {}: {}", v.rule_type, v.rule_description);
            }
        }
        return Ok(());
    }

    println!("✗ {}: Violations found in module '{}'", file_path, module_name);
    println!();

    for v in &drift_result.violations {
        match v.rule_type.as_str() {
            "forbidden" => {
                println!("  ✗ FORBIDDEN: {}", v.rule_description);
            }
            "required" => {
                println!("  ⚠ REQUIRED: {}", v.rule_description);
            }
            "warning" => {
                println!("  ⚠ WARNING: {}", v.rule_description);
            }
            _ => {
                println!("  - {}: {}", v.rule_type, v.rule_description);
            }
        }

        if let Some(pattern) = &v.pattern {
            if !pattern.is_empty() {
                println!("    Pattern: {}", pattern);
            }
        }
        if !v.suggestion.is_empty() {
            println!("    Suggestion: {}", v.suggestion);
        }
        println!();
    }

    let has_forbidden = drift_result.violations.iter()
        .any(|v| v.rule_type == "forbidden");

    if has_forbidden {
        std::process::exit(1);
    }

    Ok(())
}