use crate::core::db::{Db, DB_PATH};
use crate::core::parser::{Parser, DriftViolation};
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
            let matches = crate::core::rule_engine::pattern_matches(&content, pattern);

            match rule.rule_type {
                RuleType::Forbidden if matches => {
                    drift_result.violations.push(DriftViolation {
                        rule_type: rule_type_str.to_string(),
                        rule_description: rule.description.clone(),
                        pattern: Some(pattern.clone()),
                        line_number: None,
                        suggestion: "Remove or refactor matching pattern".to_string(),
                    });
                }
                RuleType::Required if !matches => {
                    drift_result.violations.push(DriftViolation {
                        rule_type: rule_type_str.to_string(),
                        rule_description: rule.description.clone(),
                        pattern: Some(pattern.clone()),
                        line_number: None,
                        suggestion: format!("Add required pattern: {}", pattern),
                    });
                }
                RuleType::Warning if matches => {
                    drift_result.violations.push(DriftViolation {
                        rule_type: rule_type_str.to_string(),
                        rule_description: rule.description.clone(),
                        pattern: Some(pattern.clone()),
                        line_number: None,
                        suggestion: "Consider refactoring".to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    // `clean` is derived — recompute after the rules loop pushed violations.
    drift_result.recompute_clean();

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
/// The gate: whole-repo, config-driven, exit code 1 on an error-severity
/// finding.
///
/// `archex check --file-path` is a per-file convenience for an agent mid-edit.
/// The gate is the promise, and it runs over the whole tree.
pub fn run_gate(strict: bool, update_baseline: bool) -> Result<bool, Box<dyn std::error::Error>> {
    use crate::core::baseline::Baseline;
    use crate::core::boundary;
    use crate::core::config::{self, Config};

    let root = std::env::current_dir()?;
    let db = Db::shared(Path::new(DB_PATH))?;

    db.init_schema()?;
    if db.get_all_modules()?.is_empty() {
        return Err("no modules in the index. Run `archex init` first.".into());
    }

    let config = Config::load(&root).map_err(|e| format!(
        "{}\n\nThe gate needs a committed architecture. Run `archex init` to draft one.",
        e
    ))?;
    let rules = config::load_rules(&root)?;

    let report = boundary::check(&db, &config, &rules)?;

    println!("Architecture gate: {}", config.name);
    println!(
        "  {} files in modules, {} unclassified",
        report.classified_files, report.unclassified_files
    );
    println!(
        "  {} imports unclassified (never a violation, never silently clean)",
        report.unresolved_imports
    );
    println!("  {} rule(s) loaded", rules.len());

    // Per-rule visibility. A rule that matches nothing is indistinguishable
    // from a rule that does not exist, so say which one this is: an empty
    // side is almost certainly a typo, while two populated sides with no
    // edges is simply a clean result.
    let mut suspicious: Vec<String> = Vec::new();
    for rule in &rules {
        if !rule.is_active() || rule.boundary.is_none() {
            continue;
        }
        let Some(status) = report.rule_status(&rule.id) else {
            continue;
        };
        println!(
            "    {}: {} from-file(s), {} to-file(s), {} candidate edge(s){}",
            rule.id, status.from_files, status.to_files, status.edges,
            if rule.status == crate::core::config::Status::Retired {
                " [retired]"
            } else {
                ""
            }
        );
        if status.suspicious {
            suspicious.push(format!(
                "  rule `{}`: {} file(s) on the from side, {} on the to side -- \
                 one side is empty, so this rule matches nothing",
                rule.id, status.from_files, status.to_files
            ));
        }
    }
    for line in &suspicious {
        eprintln!("{}", line);
    }

    if update_baseline {
        let baseline = Baseline::from_findings(&report.findings);
        let path = Baseline::path(&root);
        std::fs::write(&path, baseline.to_toml())?;
        println!(
            "\n  Recorded {} existing violation(s) in {}. Commit it.",
            baseline.violations.len(),
            path.display()
        );
        return Ok(true);
    }

    // Recorded debt is reported, not hidden: it is still a violation, it just
    // does not fail the build. Silently dropping it would make the baseline a
    // place violations go to die.
    let baseline = Baseline::load(&root)?;
    let mut fresh = Vec::new();
    let mut recorded = 0usize;
    for finding in &report.findings {
        if baseline.absorbs(finding) {
            recorded += 1;
        } else {
            fresh.push(finding);
        }
    }

    if fresh.is_empty() {
        if recorded > 0 {
            println!("\n  No NEW violations ({} recorded in baseline).", recorded);
        } else {
            println!("\n  No violations.");
        }
    } else {
        if recorded > 0 {
            println!("\n  {} new violation(s):", fresh.len());
        } else {
            println!("\n  {} violation(s):", fresh.len());
        }
        for finding in &fresh {
            println!(
                "    {}:{}  [{}] {}",
                finding.from_file, finding.line_number, finding.rule_id, finding.message
            );
            println!(
                "        imports {} -> {}",
                finding.imported_from, finding.to_file
            );
        }
    }

    let errors = fresh
        .iter()
        .filter(|f| f.severity == crate::core::config::Severity::Error)
        .count();

    if strict && !suspicious.is_empty() {
        return Ok(false);
    }
    Ok(errors == 0)
}
