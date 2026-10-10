use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

mod common;

use common::fixture_project;

fn db_path(root: &Path) -> std::path::PathBuf {
    root.join(".archex/archex.db")
}

fn rule_count(root: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    conn.query_row("SELECT COUNT(*) FROM rules", [], |r| r.get(0))
        .unwrap()
}

fn counts(root: &Path) -> (i64, i64, i64) {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    let modules: i64 = conn
        .query_row("SELECT COUNT(*) FROM modules", [], |r| r.get(0))
        .unwrap();
    let symbols: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    let imports: i64 = conn
        .query_row("SELECT COUNT(*) FROM imports", [], |r| r.get(0))
        .unwrap();
    (modules, symbols, imports)
}

fn duplicate_module_names(root: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM modules GROUP BY name HAVING COUNT(*) > 1")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

#[test]
fn init_twice_is_idempotent() {
    let dir = TempDir::new().unwrap();
    fixture_project(dir.path());

    // First init: no .archex dir yet, so no reinit prompt.
    Command::cargo_bin("archex")
        .unwrap()
        .arg("init")
        .current_dir(dir.path())
        .write_stdin("")
        .assert()
        .success();

    let first = counts(dir.path());
    // 6, not 16: a curated module whose directory holds no code is dropped
    // rather than seeded empty. The fixture has app, app/api, components,
    // db, lib, services -- the other 10 curated entries have no files here,
    // and on a real repo (LORB) 10 of 16 matched nothing.
    assert_eq!(first.0, 6, "seeded modules must be exactly the ones with files in them");
    assert!(first.1 > 0, "symbols should be extracted");
    assert!(first.2 > 0, "imports should be extracted");
    assert_eq!(rule_count(dir.path()), 8, "Next.js seeding must create exactly 8 rules (2 app, 2 components, 3 api, 1 services)");
    assert!(duplicate_module_names(dir.path()).is_empty(), "no duplicate module names after first init");

    // Second init: answer "y" to the reinitialize prompt.
    Command::cargo_bin("archex")
        .unwrap()
        .arg("init")
        .current_dir(dir.path())
        .write_stdin("y\n")
        .assert()
        .success();

    let second = counts(dir.path());
    assert_eq!(
        second, first,
        "init run twice must not duplicate modules, symbols, or imports (got {second:?}, expected {first:?})"
    );
    assert_eq!(rule_count(dir.path()), 8, "rules must not duplicate or vanish on re-init");
    assert!(duplicate_module_names(dir.path()).is_empty(), "no duplicate module names after second init");
}
