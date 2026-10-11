use crate::core::db::{Db, DB_PATH};
use std::path::Path;

/// The per-file advisory: an agent mid-edit asks "is this file clean?"
///
/// This runs the SAME loader and the SAME engine as the gate, then filters the
/// report to one file. Not "similar to" the gate — the same findings, so an
/// agent can never be told a file is clean while CI blocks it. Advisory and
/// gate disagreement is worse than either being wrong alone: agents learn to
/// distrust the advisory and stop asking.
pub fn run(file_path: String) -> Result<(), Box<dyn std::error::Error>> {
    use crate::core::config;

    let root = std::env::current_dir()?;
    let db = Db::shared(Path::new(DB_PATH))?;

    // Normalize the way the index stores paths, so an agent passing an
    // absolute path, a backslash path or `./`-prefixed path matches the same
    // file the gate reports. A file the repo does not own is not something an
    // architectural rule can have an opinion about.
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

    let config = config::Config::load(&root).map_err(|e| format!(
        "{}\n\nThe advisory needs a committed architecture. Run `archex init` to draft one.",
        e
    ))?;
    let rules = config::load_rules(&root)?;

    let report = crate::core::boundary::check(&db, &config, &rules)?;

    // Baseline absorbs debt for the gate. The advisory answers "is this file
    // violating?" — recorded debt is still a violation, it just does not fail
    // the build. Hiding it here would make the two paths disagree for a
    // different reason.
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

    let mut errors = 0usize;
    for finding in &findings {
        println!(
            "  {}:{}  [{}] {}",
            finding.from_file, finding.line_number, finding.rule_id, finding.message
        );
        println!(
            "        imports {} -> {}",
            finding.imported_from, finding.to_file
        );
        match finding.severity {
            crate::core::config::Severity::Error => errors += 1,
            crate::core::config::Severity::Warning => {}
        }
    }

    if errors > 0 {
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
    crate::core::rule_engine::warn_legacy_db_rules(&db);

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
        "  {} unresolved first-party import(s) (never a violation, never silently clean)",
        report.unresolved_imports
    );
    for example in &report.unresolved_examples {
        println!("      {}", example);
    }
    println!(
        "  {} external package import(s) — not in this repo, not counted as gaps",
        report.external_packages
    );
    if report.ambiguous > 0 {
        println!(
            "  {} ambiguous import(s) — `@scope/name` that resolved to nothing, so \
             package-or-alias cannot be told apart:",
            report.ambiguous
        );
        for example in &report.ambiguous_examples {
            println!("      {}", example);
        }
    }
    if report.outside_index > 0 {
        // The most dangerous line in the output. These edges exist and left
        // the graph: a workspace package resolving to its built entry point
        // is the common cause. Every rule below is blind to them, so a
        // clean run here is not a clean run.
        println!(
            "  {} import(s) resolved OUTSIDE the index — no module owns the target, \
             so no rule could judge them:",
            report.outside_index
        );
        for example in &report.outside_index_examples {
            println!("      {}", example);
        }
    }
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
            "    {}: {} from-file(s), {} to-file(s), {} candidate edge(s){}{}{}",
            rule.id,
            status.from_files,
            status.to_files,
            status.edges,
            if status.type_only_skipped > 0 {
                format!(
                    ", {} type-only edge(s) not judged",
                    status.type_only_skipped
                )
            } else {
                String::new()
            },
            if status.escaped > 0 {
                format!(
                    ", {} edge(s) left the graph — this rule could not judge them",
                    status.escaped
                )
            } else {
                String::new()
            },
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
