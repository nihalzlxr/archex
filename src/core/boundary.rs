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
name = "engine"
layer = "engine"
path = "packages/engine/**"

[[module]]
name = "content"
layer = "content"
path = "packages/content/**"
"#;

    struct Repo {
        dir: tempfile::TempDir,
        db: crate::core::db::Db,
    }

    /// Writes the files first, THEN seeds from config and scans — a fixture
    /// that scans before its sources exist measures nothing.
    fn repo(files: &[(&str, &str)]) -> Repo {
        let dir = tempfile::tempdir().expect("tempdir");
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
        Repo { dir, db }
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
            "import { schedule } from 'some-unknown-package';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(
            report.unresolved_imports > 0,
            "and it must still be COUNTED, not hidden"
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
            "import { schedule } from '../engine/src/internal/scheduler';\n",
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
from_layer = "content"
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
layers = ["engine"]

[[module]]
name = "engine"
layer = "engine"
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
            "import 'some-unknown-package';\n",
        ));
        let repo = repo(&files);
        let report = run(&repo, &[entrypoint_rule()]);
        assert_eq!(report.unclassified_files, 1);
        assert!(report.unresolved_imports >= 1);
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