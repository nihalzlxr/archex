//! The boundary engine: an edge query over resolved imports.
//!
//! `from_layer`/`to_layer` name a layer OR a module. Layers are resolved
//! first, then module names; a name that is both is a config error, never a
//! coin flip.
//!
//! Three things this engine will not do:
//! - flag an unresolved import (unclassified is not a violation)
//! - flag a file matching the config's test_globs, unless the rule opts in
//! - flag a target matched by `allow_to`


use crate::core::config::{Config, ConfigError, Rule, Severity};
use crate::core::db::ImportKind;
use crate::core::db::Db;
use std::collections::{BTreeMap, BTreeSet};

/// Replace the index's modules with the config's.
///
/// A committed config is authoritative: it does not merge with what a scan
/// inferred, because merging makes a module impossible to delete.
pub fn install_modules(db: &Db, config: &Config) -> Result<usize, ConfigError> {
    db.clear_modules()?;
    for module in &config.modules {
        db.insert_module(&module.name, &module.layer, &module.path)
            .map_err(|e| ConfigError(format!("cannot install module `{}`: {}", module.name, e)))?;
    }
    Ok(config.modules.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule_id: String,
    pub severity: Severity,
    pub from_file: String,
    pub to_file: String,
    pub line_number: i64,
    /// The specifier as written. A finding has to show its evidence.
    pub imported_from: String,
    pub message: String,
}

/// What a rule actually saw. A rule matching nothing and a rule that does not
/// exist look identical from the outside; this is how they are told apart.
#[derive(Debug, Clone, Copy)]
pub struct RuleStatus {
    pub from_files: usize,
    pub to_files: usize,
    /// Edges between the two sides, before allow_to and test exemptions.
    pub edges: usize,
    /// Type-only edges this rule did not judge, because it did not ask to.
    pub type_only_skipped: usize,
    /// Edges that crossed these two modules and then left the graph —
    /// resolved to a file no module owns. Counted so "0 candidates" can be
    /// read for what it is.
    pub escaped: usize,
    /// One side has no files: almost certainly a typo or a dead layer.
    pub suspicious: bool,
}

#[derive(Debug, Default)]
pub struct Coverage {
    pub classified_files: usize,
    pub unclassified_files: usize,
    pub unresolved_imports: usize,
    /// Imports that resolved to a real file no module owns — typically a
    /// workspace package's built entry point (`packages/engine/dist/…`).
    /// Measured on project/fitness: 163 of them, and every cross-package
    /// boundary rule read clean because of them.
    pub outside_index: usize,
    /// Third-party packages. Not gaps: `react` is not in the repo and
    /// never will be. Counted apart so they cannot dilute the first-party
    /// number that actually means something.
    pub external_packages: usize,
    /// The first few offenders, so "12 imports left the graph" is
    /// actionable rather than a number to shrug at.
    pub outside_index_examples: Vec<String>,
    pub unresolved_examples: Vec<String>,
    /// The escaped edges themselves, kept so a rule can be told how many of
    /// them crossed ITS two sides. A count alone cannot say that.
    pub escaped: Vec<EscapedEdge>,
}

/// An import that resolved to a real file outside every module.
#[derive(Debug, Clone)]
pub struct EscapedEdge {
    pub from_module: String,
    pub to_file: String,
}

impl Coverage {
    fn example(list: &mut Vec<String>, value: String) {
        const MAX: usize = 5;
        if list.len() < MAX {
            list.push(value);
        }
    }
}

#[derive(Debug, Default)]
pub struct BoundaryReport {
    pub findings: Vec<Finding>,
    pub rule_status: BTreeMap<String, RuleStatus>,
    /// Files that belong to no module. Counted, never silently "clean".
    pub classified_files: usize,
    pub unclassified_files: usize,
    /// Imports whose specifier did not resolve. Unclassified, never a
    /// violation.
    pub unresolved_imports: usize,
    /// See `Coverage::outside_index`: edges that resolved to a file no
    /// module owns. They left the graph, so a rule cannot judge them.
    pub outside_index: usize,
    pub external_packages: usize,
    pub outside_index_examples: Vec<String>,
    pub unresolved_examples: Vec<String>,
    /// Rules whose only edges were the ones that left the graph.
    pub rules_that_could_not_judge: Vec<String>,
}

impl BoundaryReport {
    pub fn rule_status(&self, rule_id: &str) -> Option<RuleStatus> {
        self.rule_status.get(rule_id).copied()
    }
}

/// One resolved edge, with everything the rules need to judge it.
struct Edge {
    from_file: String,
    to_file: String,
    line_number: i64,
    imported_from: String,
    from_module: String,
    to_module: String,
    /// What kind of edge this is. A type-only edge does not create runtime
    /// coupling, so a rule skips it unless it opts in.
    kind: ImportKind,
}

pub fn check(db: &Db, config: &Config, rules: &[Rule]) -> Result<BoundaryReport, ConfigError> {
    let layer_of: BTreeMap<String, String> = config
        .modules
        .iter()
        .map(|m| (m.name.clone(), m.layer.clone()))
        .collect();

    let (edges, coverage) = collect_edges(db)?;
    let test_globs = config.test_globs().iter().map(|g| g.to_string()).collect::<Vec<_>>();

    let mut report = BoundaryReport {
        classified_files: coverage.classified_files,
        unclassified_files: coverage.unclassified_files,
        unresolved_imports: coverage.unresolved_imports,
        outside_index: coverage.outside_index,
        external_packages: coverage.external_packages,
        outside_index_examples: coverage.outside_index_examples.clone(),
        unresolved_examples: coverage.unresolved_examples.clone(),
        ..Default::default()
    };

    for rule in rules {
        if !rule.is_active() {
            continue;
        }
        let Some(boundary) = &rule.boundary else {
            continue;
        };

        let from_modules = resolve_side(&layer_of, &config.modules, &boundary.from_layer)
            .map_err(|e| ConfigError(format!("rule `{}`: {}", rule.id, e)))?;
        let to_modules = resolve_side(&layer_of, &config.modules, &boundary.to_layer)
            .map_err(|e| ConfigError(format!("rule `{}`: {}", rule.id, e)))?;

        let from_files: usize = from_modules
            .iter()
            .map(|module| db.files_in_module(module).unwrap_or(0))
            .sum();
        let to_files: usize = to_modules
            .iter()
            .map(|module| db.files_in_module(module).unwrap_or(0))
            .sum();

        let between_sides = edges
            .iter()
            .filter(|e| from_modules.contains(&e.from_module) && to_modules.contains(&e.to_module));

        // Edges that crossed these two modules but left the graph before
        // they got here. A rule cannot judge what it cannot see, and a
        // rule that examined nothing must not read as a clean result.
        let escaped = escaped_edges(&coverage, &from_modules, &to_modules, &config.modules);

        // A type-only edge compiles to nothing. Skipping it is the default,
        // and the count of what was skipped is reported rather than swallowed:
        // a rule that quietly judged nothing must never look like a clean run.
        let type_only_skipped = between_sides
            .clone()
            .filter(|e| e.kind == ImportKind::TypeOnly && !boundary.include_type_only)
            .count();

        let candidates: Vec<&Edge> = between_sides
            .filter(|e| boundary.include_type_only || e.kind != ImportKind::TypeOnly)
            .collect();

        report.rule_status.insert(
            rule.id.clone(),
            RuleStatus {
                from_files,
                to_files,
                edges: candidates.len(),
                type_only_skipped,
                suspicious: from_files == 0 || to_files == 0,
                escaped,
            },
        );

        if escaped > 0 && candidates.is_empty() && !boundary.include_tests {
            report.rules_that_could_not_judge.push(rule.id.clone());
        }

        for edge in candidates {
            if !boundary.include_tests && is_test_file(&edge.from_file, &test_globs) {
                continue;
            }
            if matches_any(&edge.from_file, &boundary.exclude)
                || matches_any(&edge.to_file, &boundary.exclude)
            {
                continue;
            }
            if matches_any(&edge.to_file, &boundary.allow_to) {
                continue;
            }
            report.findings.push(Finding {
                rule_id: rule.id.clone(),
                severity: rule.severity,
                from_file: edge.from_file.clone(),
                to_file: edge.to_file.clone(),
                line_number: edge.line_number,
                imported_from: edge.imported_from.clone(),
                message: boundary.message.clone(),
            });
        }
    }

    report.findings.sort_by(|a, b| {
        a.rule_id
            .cmp(&b.rule_id)
            .then(a.from_file.cmp(&b.from_file))
            .then(a.line_number.cmp(&b.line_number))
    });
    Ok(report)
}

/// Pull every resolved edge out of the index, plus the coverage counters.
///
/// Unresolved imports and unmapped files are counted here rather than
/// discovered later: "nobody complained" must not be mistaken for "clean".
fn collect_edges(db: &Db) -> Result<(Vec<Edge>, Coverage), ConfigError> {
    let indexed = db
        .indexed_files()
        .map_err(|e| ConfigError(format!("cannot read index: {}", e)))?;

    let mut edges = Vec::new();
    let mut coverage = Coverage::default();

    // A file in no module never reaches file_map, so coverage comes from the
    // scan's own counters. Deriving it from the index would report zero
    // uncovered files in exactly the repos with the most of them.
    let (scanned, mapped) = db.scan_stats();
    coverage.classified_files = mapped;
    coverage.unclassified_files = scanned.saturating_sub(mapped);

    for file in &indexed {
        if db.module_of_file(file).is_none() {
            continue;
        }

        for import in db
            .imports_of_file(file)
            .map_err(|e| ConfigError(format!("cannot read imports: {}", e)))?
        {
            let Some(target) = import.resolved_to else {
                // `react` is not in this repository and never will be.
                // Calling that an unclassified import would bury the
                // first-party gaps under a hundred lines of npm.
                if import.kind == ImportKind::Package {
                    coverage.external_packages += 1;
                } else {
                    coverage.unresolved_imports += 1;
                    Coverage::example(
                        &mut coverage.unresolved_examples,
                        format!("{}: {}", file, import.imported_from),
                    );
                }
                continue;
            };

            let Some(from_module) = db.module_of_file(file) else {
                continue;
            };
            let Some(to_module) = db.module_of_file(&target) else {
                // It resolved to a real file that no module owns — a
                // workspace package's built entry point, usually. The edge
                // is real and it is NOT in the graph, so it is counted and
                // named. Dropping it here is how a monorepo's boundary
                // rules report "0 candidate edges" on code that has plenty.
                coverage.outside_index += 1;
                Coverage::example(
                    &mut coverage.outside_index_examples,
                    format!("{} -> {}", file, target),
                );
                coverage.escaped.push(EscapedEdge {
                    from_module: from_module.clone(),
                    to_file: target,
                });
                continue;
            };

            edges.push(Edge {
                from_file: file.clone(),
                to_file: target,
                line_number: import.line_number,
                imported_from: import.imported_from,
                from_module,
                to_module,
                kind: import.kind,
            });
        }
    }

    Ok((edges, coverage))
}

/// A name is a layer or a module. If it is both, that is a config error: a
/// coin flip here silently changes what the rule means.
fn resolve_side(
    layer_of: &BTreeMap<String, String>,
    modules: &[crate::core::config::ModuleConfig],
    name: &str,
) -> Result<BTreeSet<String>, String> {
    let is_layer = layer_of.values().any(|l| l == name);
    let is_module = modules.iter().any(|m| m.name == name);

    let layer_modules: BTreeSet<String> = layer_of
        .iter()
        .filter(|(_, layer)| *layer == name)
        .map(|(module, _)| module.clone())
        .collect();

    match (is_layer, is_module) {
        // Both readings are available. Reject only when they disagree:
        // accepting a name whose two readings name the same set of modules
        // cannot change what the rule means, and in a monorepo
        // `name = "engine", layer = "engine"` is the obvious thing to write.
        (true, true) if layer_modules.len() == 1 && layer_modules.contains(name) => Ok(layer_modules),
        (true, true) => Err(format!(
            "`{}` is both a layer and a module name, and they do not mean the \
             same thing: as a layer it covers {:?}, as a module it covers only \
             itself. Rename one of them, or use a name that is unambiguous",
            name,
            layer_modules.iter().collect::<Vec<_>>()
        )),
        (true, false) => Ok(layer_modules),
        (false, true) => Ok([name.to_string()].into_iter().collect()),
        (false, false) => Err(format!("`{}` is neither a declared layer nor a module", name)),
    }
}

fn compile(globs: &[String]) -> Vec<glob::Pattern> {
    globs
        .iter()
        .filter_map(|g| glob::Pattern::new(g).ok())
        .collect()
}

/// `*` must not cross a `/`: `packages/content/src/*.ts` means files in that
/// directory, not the whole tree. `**` still spans directories.
fn matches_pattern(pattern: &glob::Pattern, path: &str) -> bool {
    pattern.matches_path_with(
        std::path::Path::new(path),
        glob::MatchOptions {
            require_literal_separator: true,
            ..Default::default()
        },
    )
}

pub fn is_test_file(path: &str, globs: &[String]) -> bool {
    matches_any(path, globs)
}

pub fn matches_any(path: &str, globs: &[String]) -> bool {
    let compiled = compile(globs);
    compiled.iter().any(|p| matches_pattern(p, path))
}

/// How many edges crossed this rule's two sides and then left the graph.
///
/// The escaped edge has no module on its target side — that is why it is
/// escaped — so the target is matched against the `to` modules' path
/// patterns instead. That is the same question the boundary engine asks
/// everywhere else, not a guess about what the file is.
fn escaped_edges(
    coverage: &Coverage,
    from_modules: &std::collections::BTreeSet<String>,
    to_modules: &std::collections::BTreeSet<String>,
    modules: &[crate::core::config::ModuleConfig],
) -> usize {
    let patterns: Vec<String> = to_modules
        .iter()
        .filter_map(|name| modules.iter().find(|m| &m.name == name))
        .map(|m| m.path.clone())
        .collect();

    coverage
        .escaped
        .iter()
        .filter(|escaped| {
            from_modules.iter().any(|name| *name == escaped.from_module)
                && matches_any(&escaped.to_file, &patterns)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{Config, Rule, Severity};

    const CONFIG: &str = r#"
schema_version = 1
name = "t"
layers = ["ui", "engine", "content"]

[[module]]
name = "web"
layer = "ui"
path = "apps/web/**"

[[module]]
name = "engine-pkg"
layer = "engine"
path = "packages/engine/**"

[[module]]
name = "content-pkg"
layer = "content"
path = "packages/content/**"
"#;

    struct Repo {
        db: crate::core::db::Db,
    }

    /// Writes the files first, THEN seeds from config and scans — a fixture
    /// that scans before its sources exist measures nothing.
    fn repo(files: &[(&str, &str)]) -> Repo {
        let dir = tempfile::tempdir().expect("tempdir");

        // A workspace, so `@fitness/engine` is a resolvable specifier rather
        // than a third-party package that stays unclassified forever.
        for sub in ["packages", "packages/engine", "packages/content"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        std::fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("packages/engine/package.json"),
            "{\"name\":\"@fitness/engine\",\"exports\":{\".\":\"./src/index.ts\"}}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("packages/content/package.json"),
            "{\"name\":\"@fitness/content\",\"main\":\"src/index.ts\"}",
        )
        .unwrap();

        for (rel, body) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        let db = crate::core::db::Db::open_in_memory().expect("db");
        db.init_schema().expect("schema");
        let config = Config::parse_str(CONFIG).expect("config");
        install_modules(&db, &config).expect("install modules");
        crate::core::parser::Parser::new(std::sync::Arc::new(db.clone()))
            .scan(dir.path())
            .expect("scan");
        Repo { db }
    }

    fn engine_files() -> Vec<(&'static str, &'static str)> {
        vec![
            ("packages/engine/src/index.ts", "export const engine = 1;\n"),
            (
                "packages/engine/src/internal/scheduler.ts",
                "export const schedule = 1;\n",
            ),
        ]
    }

    fn rule(body: &str) -> Rule {
        Rule::parse_str(body).expect("valid rule")
    }

    fn entrypoint_rule() -> Rule {
        rule(
            r#"
schema_version = 1
id = "web-uses-engine-entry"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "engine"
allow_to = ["packages/engine/src/index.ts"]
message = "Import the engine through its public entrypoint."
"#,
        )
    }

    fn run(repo: &Repo, rules: &[Rule]) -> BoundaryReport {
        let config = Config::parse_str(CONFIG).expect("config");
        check(&repo.db, &config, rules).expect("valid config and rules")
    }

    // --- the core: caught, and located ---

    #[test]
    fn a_forbidden_edge_is_caught_with_both_files_and_a_line() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "import { schedule } from '@fitness/engine/src/internal/scheduler';\n",
        ));
        let repo = repo(&files);

        let report = run(&repo, &[entrypoint_rule()]);
        assert_eq!(report.findings.len(), 1, "{:?}", report.findings);

        let finding = &report.findings[0];
        assert_eq!(finding.rule_id, "web-uses-engine-entry");
        assert_eq!(finding.from_file, "apps/web/src/feature.ts");
        assert_eq!(finding.to_file, "packages/engine/src/internal/scheduler.ts");
        assert_eq!(finding.line_number, 1);
        assert_eq!(finding.severity, Severity::Error);
        assert!(finding.message.contains("public entrypoint"));
    }

    #[test]
    fn the_line_number_is_the_import_not_the_file_start() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "// header\n// header\nimport { schedule } from '@fitness/engine/src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        assert_eq!(run(&repo, &[entrypoint_rule()]).findings[0].line_number, 3);
    }

    // --- the exceptions, which matter more ---

    #[test]
    fn the_public_entrypoint_is_allowed() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "import { engine } from '@fitness/engine';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    /// 34 test-file-to-internals imports on fitness versus 3 real ones. If
    /// this is not exempt, every rule flags every test file on day one.
    #[test]
    fn a_test_file_importing_internals_produces_no_finding() {
        let mut files = engine_files();
        files.push((
            "packages/engine/test/scheduler.test.ts",
            "import { schedule } from '../src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn include_tests_opts_back_in() {
        let mut files = engine_files();
        files.push((
            "packages/engine/test/scheduler.test.ts",
            "import { schedule } from '../src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "engine"
to_layer = "engine"
include_tests = true
message = "m"
"#,
        );
        assert_eq!(run(&repo, &[rule]).findings.len(), 1);
    }

    #[test]
    fn per_rule_exclude_still_works() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/legacy.ts",
            "import { schedule } from '@fitness/engine/src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "engine"
exclude = ["apps/web/src/legacy.ts"]
message = "m"
"#,
        );
        assert!(run(&repo, &[rule]).findings.is_empty());
    }

    // --- what must never be a violation ---

    /// An unresolved import is unclassified, not an accusation. Guessing here
    /// is how a gate cries wolf.
    #[test]
    fn an_unresolved_import_is_never_a_violation() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "import { gone } from '../lib/gone';\nimport { schedule } from 'some-unknown-package';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(
            report.unresolved_imports > 0,
            "and the first-party gap must still be COUNTED, not hidden"
        );
        assert_eq!(
            report.external_packages, 1,
            "a bare package name is not a gap: it is not in this repo and \
             never will be. Counting it as one buried the real number."
        );
    }

    #[test]
    fn a_file_in_no_module_is_unclassified_and_silent() {
        let mut files = engine_files();
        files.push((
            "scripts/build.ts",
            "import { schedule } from '../packages/engine/src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.unclassified_files, 1);
    }

    #[test]
    fn a_clean_tree_produces_zero_findings() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "import { engine } from '@fitness/engine';\n",
        ));
        files.push((
            "packages/content/src/repo.ts",
            "export const repo = 1;\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.unclassified_files, 0);
    }

    // --- naming: layer or module, never a coin flip ---

    #[test]
    fn from_and_to_may_name_modules_instead_of_layers() {
        let mut files = engine_files();
        files.push((
            "packages/content/src/repo.ts",
            "import { schedule } from '../../engine/src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "content-pkg"
to_layer = "engine"
message = "m"
"#,
        );
        assert_eq!(run(&repo, &[rule]).findings.len(), 1);
    }

    #[test]
    fn a_name_that_is_both_a_layer_and_a_module_is_rejected() {
        let mut files = engine_files();
        files.push(("apps/web/src/feature.ts", "export const x = 1;\n"));
        let repo = repo(&files);
        let config = Config::parse_str(
            r#"
schema_version = 1
name = "t"
layers = ["ui", "engine"]

[[module]]
name = "web"
layer = "ui"
path = "apps/web/**"

[[module]]
name = "admin"
layer = "engine"
path = "apps/admin/**"

[[module]]
name = "engine"
layer = "ui"
path = "packages/engine/**"
"#,
        )
        .expect("config");
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "web"
to_layer = "engine"
message = "m"
"#,
        );
        let err = check(&repo.db, &config, &[rule]).expect_err("ambiguous name must not resolve");
        assert!(err.to_string().contains("engine"), "{}", err);
        assert!(err.to_string().contains("both"), "{}", err);
    }

    #[test]
    fn a_rule_naming_an_unknown_layer_or_module_is_an_error() {
        let repo = repo(&engine_files());
        let config = Config::parse_str(CONFIG).expect("config");
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "marketing"
to_layer = "engine"
message = "m"
"#,
        );
        let err = check(&repo.db, &config, &[rule]).expect_err("unknown reference");
        assert!(err.to_string().contains("marketing"), "{}", err);
    }

    // --- dormancy: a rule that matches nothing must say so ---

    #[test]
    fn a_rule_whose_side_has_no_files_is_reported_as_suspicious() {
        let repo = repo(&engine_files());
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "engine"
message = "m"
"#,
        );
        // No ui files exist in this repo at all.
        let report = run(&repo, &[rule]);
        let status = report.rule_status("r").expect("rule status");
        assert_eq!(status.from_files, 0);
        assert!(status.suspicious, "a rule with an empty side is misconfigured");
    }

    #[test]
    fn a_rule_with_files_on_both_sides_but_no_edges_is_informational() {
        let mut files = engine_files();
        files.push(("apps/web/src/feature.ts", "export const x = 1;\n"));
        let repo = repo(&files);
        let rule = rule(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "engine"
message = "m"
"#,
        );
        let report = run(&repo, &[rule]);
        let status = report.rule_status("r").expect("rule status");
        assert!(status.from_files > 0 && status.to_files > 0);
        assert_eq!(status.edges, 0);
        assert!(!status.suspicious, "no edges between two populated layers is a clean result");
    }

    #[test]
    fn candidate_edge_count_is_reported_even_when_exempt() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "import { engine } from '@fitness/engine';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        let status = report.rule_status("web-uses-engine-entry").expect("status");
        assert_eq!(status.edges, 1, "the edge exists, allow_to just permits it");
    }

    #[test]
    fn retired_rules_are_not_enforced() {
        let mut files = engine_files();
        files.push((
            "apps/web/src/feature.ts",
            "import { schedule } from '@fitness/engine/src/internal/scheduler';\n",
        ));
        let repo = repo(&files);
        let mut rule = entrypoint_rule();
        rule.status = crate::core::config::Status::Retired;
        assert!(run(&repo, &[rule]).findings.is_empty());
    }

    #[test]
    fn coverage_counters_are_reported() {
        let mut files = engine_files();
        files.push(("scripts/build.ts", "export const x = 1;\n"));
        files.push((
            "apps/web/src/feature.ts",
            "import { gone } from '../lib/gone';\nimport 'some-unknown-package';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert_eq!(report.unclassified_files, 1);
        assert!(
            report.unresolved_imports >= 1,
            "the first-party gap is the number that matters"
        );
        assert_eq!(report.external_packages, 1, "and npm packages are not gaps");
        assert_eq!(report.classified_files, 3);
    }

    // --- helpers ---

    #[test]
    fn is_test_file_matches_the_defaults() {
        let globs: Vec<String> = crate::core::config::DEFAULT_TEST_GLOBS
            .iter()
            .map(|g| g.to_string())
            .collect();
        for path in ["src/a.test.ts", "packages/engine/test/foo.ts", "src/__tests__/bar.tsx"] {
            assert!(is_test_file(path, &globs), "{} should be a test file", path);
        }
        for path in ["src/a.ts", "apps/web/app/page.tsx", "src/testing/helpers.ts"] {
            assert!(!is_test_file(path, &globs), "{} should not be a test file", path);
        }
    }

    #[test]
    fn allow_to_matches_targets() {
        let globs = vec!["packages/engine/src/index.ts".to_string()];
        assert!(matches_any("packages/engine/src/index.ts", &globs));
        assert!(!matches_any("packages/engine/src/internal/scheduler.ts", &globs));
    }

    #[test]
    fn allow_to_supports_globs() {
        let globs = vec!["packages/engine/src/index.ts".to_string(), "packages/content/src/*.ts".to_string()];
        assert!(matches_any("packages/content/src/repo.ts", &globs));
        assert!(!matches_any("packages/content/src/deep/repo.ts", &globs));
    }
}
#[cfg(test)]
mod naming_tests {
    use super::*;
    fn layer_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(m, l)| (m.to_string(), l.to_string()))
            .collect()
    }

    fn modules(pairs: &[(&str, &str)]) -> Vec<crate::core::config::ModuleConfig> {
        pairs
            .iter()
            .map(|(name, layer)| crate::core::config::ModuleConfig {
                name: name.to_string(),
                layer: layer.to_string(),
                path: format!("{}/**", name),
            })
            .collect()
    }

    /// A module named after its own layer is the obvious thing to write in a
    /// monorepo, and both readings name the same modules, so it is not
    /// ambiguous and must not be rejected.
    #[test]
    fn a_module_named_after_its_own_layer_is_accepted() {
        let resolved = resolve_side(
            &layer_map(&[("engine", "engine")]),
            &modules(&[("engine", "engine")]),
            "engine",
        )
        .expect("unambiguous");
        assert_eq!(resolved.into_iter().collect::<Vec<_>>(), vec!["engine"]);
    }

    /// Here the two readings differ: as a layer it covers every ui module, as
    /// a module only itself. Reject it rather than guess.
    #[test]
    fn a_name_whose_two_readings_differ_is_rejected() {
        // A layer named `engine` covering web + admin, alongside a MODULE
        // named `engine`: the two readings name different module sets.
        let err = resolve_side(
            &layer_map(&[("web", "engine"), ("admin", "engine")]),
            &modules(&[("web", "engine"), ("admin", "engine"), ("engine", "ui")]),
            "engine",
        )
        .expect_err("must be ambiguous");
        assert!(err.contains("both a layer and a module"), "{}", err);
    }

    #[test]
    fn a_layer_covering_several_modules_resolves_to_all_of_them() {
        let resolved = resolve_side(
            &layer_map(&[("web", "ui"), ("admin", "ui")]),
            &modules(&[("web", "ui"), ("admin", "ui")]),
            "ui",
        )
        .expect("valid");
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn config_loading_reports_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".archex/rules")).unwrap();
        std::fs::write(
            dir.path().join(".archex/config.toml"),
            "schema_version = 1\nname = \"x\"\nlayers = []\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".archex/rules/bad.toml"),
            "schema_version = 1\nid = \"bad\"\nstatus = \"accepted\"\nkind = \"boundary\"\nseverity = \"error\"\n",
        )
        .unwrap();
        let err = crate::core::config::load_rules(dir.path()).expect_err("bad rule");
        assert!(err.to_string().contains("bad.toml"), "{}", err);
    }
}
