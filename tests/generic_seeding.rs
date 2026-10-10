use assert_cmd::Command;
use rusqlite::Connection;
use tempfile::TempDir;

mod common;

use common::fixture_project;

fn init(root: &std::path::Path) {
    Command::cargo_bin("archex")
        .unwrap()
        .arg("init")
        .current_dir(root)
        .write_stdin("")
        .assert()
        .success();
}

/// The curated Next.js list, kept here so the test asserts against the same
/// table the seeder uses rather than a number copied at some past moment.
const CURATED: &[(&str, &str, &str)] = &[
    ("app", "ui", "src/app/**"),
    ("components", "ui", "src/components/**"),
    ("api", "api", "src/app/api/**"),
    ("lib", "service", "src/lib/**"),
    ("db", "db", "src/db/**"),
    ("actions", "service", "src/actions/**"),
    ("services", "service", "src/services/**"),
    ("hooks", "ui", "src/hooks/**"),
    ("utils", "service", "src/utils/**"),
    ("types", "types", "src/types/**"),
    ("middleware", "api", "src/middleware/**"),
    ("jobs", "service", "src/jobs/**"),
    ("validations", "service", "src/validations/**"),
    ("models", "db", "src/models/**"),
    ("server", "api", "src/server/**"),
    ("libs", "service", "src/libs/**"),
];

/// A plain repo — no `next` in package.json — with three code directories and
/// one documentation-only directory.
fn plain_repo(root: &std::path::Path) {
    std::fs::write(
        root.join("package.json"),
        r#"{ "dependencies": { "react": "18.0.0" } }"#,
    )
    .unwrap();

    std::fs::create_dir_all(root.join("src/deep/nested")).unwrap();
    std::fs::write(
        root.join("src/deep/nested/a.ts"),
        "export function alpha(): string {\n  return 'a';\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/root.ts"), "export const R = 1;\n").unwrap();

    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(
        root.join("lib/helper.py"),
        "def helper():\n    return 1\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("tests/helper_test.rs"),
        "pub fn works() -> i32 {\n    1\n}\n",
    )
    .unwrap();

    // Docs only: no indexable file anywhere inside.
    std::fs::create_dir_all(root.join("docs/guides")).unwrap();
    std::fs::write(root.join("docs/README.md"), "# docs\n").unwrap();
    std::fs::write(root.join("docs/guides/start.md"), "# start\n").unwrap();
}

fn modules(root: &std::path::Path) -> Vec<(String, String, String)> {
    let db = Connection::open(root.join(".archex/archex.db")).unwrap();
    let mut stmt = db
        .prepare("SELECT name, layer, path_pattern FROM modules ORDER BY name")
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}

fn rule_count(root: &std::path::Path) -> i64 {
    let db = Connection::open(root.join(".archex/archex.db")).unwrap();
    db.query_row("SELECT COUNT(*) FROM rules", [], |r| r.get(0))
        .unwrap()
}

fn mapped_files(root: &std::path::Path) -> i64 {
    let db = Connection::open(root.join(".archex/archex.db")).unwrap();
    db.query_row("SELECT COUNT(*) FROM file_map", [], |r| r.get(0))
        .unwrap()
}

/// Every repo gets modules -- not just Next.js. `archex init` on a plain
/// TypeScript/Python/Rust tree used to seed ZERO modules, so every file came
/// back unmapped and `get_context` had nothing to say.
#[test]
fn plain_repo_seeds_one_module_per_code_directory() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    init(root);

    let seeded = modules(root);
    let names: Vec<&str> = seeded.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, vec!["lib", "src", "tests"]);
}

/// layer = the directory name, verbatim. archex does not get to decide that
/// `src` means "ui" or that `lib` means "service" -- that is a claim it cannot
/// keep, and Step 2 boundary rules check against it.
#[test]
fn generic_layer_is_the_directory_name_verbatim() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    init(root);

    for (name, layer, pattern) in modules(root) {
        assert_eq!(layer, name, "layer must be the directory name verbatim");
        assert_eq!(pattern, format!("{}/**", name));
    }
}

/// A directory with no indexable file is not a module. A module that maps zero
/// files is noise that later reads as "this repo has an architecture here".
#[test]
fn documentation_only_directory_gets_no_module() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    init(root);

    let names: Vec<String> = modules(root).into_iter().map(|(n, _, _)| n).collect();
    assert!(!names.iter().any(|n| n == "docs"), "docs/ has only .md files");
}

/// No semantics, no boundaries, nothing to cry wolf with. Rules arrive when a
/// human writes them.
#[test]
fn generic_seeding_creates_no_rules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    init(root);

    assert_eq!(rule_count(root), 0);
}

/// Modules are only names; the index still has to land.
#[test]
fn generic_modules_map_their_files_and_produce_symbols() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    init(root);

    // 4 indexable files: src/root.ts, src/deep/nested/a.ts, lib/helper.py,
    // tests/helper_test.rs.
    assert_eq!(mapped_files(root), 4);

    let db = Connection::open(root.join(".archex/archex.db")).unwrap();
    let symbols: i64 = db
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert!(symbols > 0, "generic seeding must still index symbols");
}

/// Skip dirs already exist in the scanner; seeding reuses that list instead of
/// inventing a second one that can drift.
#[test]
fn skip_directories_are_not_seeded_as_modules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    std::fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
    std::fs::write(root.join("node_modules/left-pad/index.ts"), "export const x = 1;\n").unwrap();
    std::fs::create_dir_all(root.join("dist")).unwrap();
    std::fs::write(root.join("dist/bundle.ts"), "export const y = 1;\n").unwrap();
    init(root);

    let names: Vec<String> = modules(root).into_iter().map(|(n, _, _)| n).collect();
    assert!(!names.iter().any(|n| n == "node_modules"));
    assert!(!names.iter().any(|n| n == "dist"));
}

/// Dot directories are project tooling (.git, .github, .archex), never
/// architecture.
#[test]
fn hidden_directories_are_not_seeded_as_modules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    std::fs::create_dir_all(root.join(".cache")).unwrap();
    std::fs::write(root.join(".cache/scratch.ts"), "export const z = 1;\n").unwrap();
    init(root);

    let names: Vec<String> = modules(root).into_iter().map(|(n, _, _)| n).collect();
    assert!(!names.iter().any(|n| n.starts_with('.')));
}

/// The manifest still guarantees the route convention, so the curated modules
/// keep their real semantic layers -- they are an overlay, not a replacement.
#[test]
fn nextjs_repo_keeps_the_curated_seed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    let seeded = modules(root);
    for (name, layer, pattern) in &seeded {
        if name == "src" {
            continue; // the generic catch-all for anything curated misses
        }
        let (_, curated_layer, curated_pattern) = CURATED
            .iter()
            .find(|(n, _, _)| n == name)
            .unwrap_or_else(|| panic!("{} is not a curated module", name));
        assert_eq!(layer, *curated_layer, "{} lost its curated layer", name);
        assert_eq!(pattern, *curated_pattern);
    }

    let components = seeded.iter().find(|(n, _, _)| n == "components").unwrap();
    assert_eq!(components.1, "ui", "curated layers are semantic");
    assert_eq!(components.2, "src/components/**");

    assert_eq!(rule_count(root), 8, "all 4 rule-bearing modules exist here");
}

/// A curated entry whose directory holds no code is not seeded: an empty
/// module is noise that later reads as architecture, and the rules hanging
/// off it can never fire.
#[test]
fn curated_entries_without_files_are_not_seeded() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    let names: Vec<String> = modules(root).into_iter().map(|(n, _, _)| n).collect();
    for empty in ["db_placeholder"] {
        assert!(!names.contains(&empty.to_string()));
    }
    // The fixture has no src/db/hooks/jobs/server/models/... directories.
    for absent in ["hooks", "jobs", "middleware", "models", "server", "validations", "actions"] {
        assert!(!names.contains(&absent.to_string()), "{} has no files here", absent);
    }
}

/// The measured reason the curated list became an overlay: on LORB the curated
/// list alone left 77% of the repo unclassified. The overlay must leave no
/// source file unmapped.
#[test]
fn nextjs_repo_leaves_no_source_file_unmapped() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    let db = Connection::open(root.join(".archex/archex.db")).unwrap();
    let unmapped: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM file_map WHERE file_path LIKE 'src/components/Button.tsx' OR file_path LIKE 'src/app/page.tsx' OR file_path LIKE 'src/services/userService.ts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unmapped, 3, "curated coverage must not leave files behind");
}

/// Seeding is a bootstrap default, and it is idempotent like everything else.
#[test]
fn init_twice_does_not_duplicate_generic_modules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    plain_repo(root);
    init(root);
    init(root);

    let names: Vec<String> = modules(root).into_iter().map(|(n, _, _)| n).collect();
    assert_eq!(names, vec!["lib", "src", "tests"]);
    assert_eq!(rule_count(root), 0);
}

/// Regression guard: the plain-repo path must not disturb the Next.js path.
#[test]
fn init_twice_keeps_nextjs_seed_stable() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);
    init(root);

    let names: Vec<String> = modules(root).into_iter().map(|(n, _, _)| n).collect();
    assert_eq!(names.len(), 6, "re-init must not resurrect dropped curated entries");
    assert_eq!(rule_count(root), 8);
}