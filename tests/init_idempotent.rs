use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

/// Build a minimal Next.js-shaped project inside `root` so that
/// `archex init` detects Next.js, seeds modules, and indexes symbols/imports.
/// Content is kept JSX-free so the TypeScript grammar parses it cleanly —
/// this test is about idempotency, not extraction quality.
fn fixture_project(root: &Path) {
    fs::write(
        root.join("package.json"),
        r#"{ "dependencies": { "next": "14.0.0" } }"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::write(
        root.join("src/components/Button.tsx"),
        "export function Button(label: string): string {\n  return label;\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/db")).unwrap();
    fs::write(
        root.join("src/db/client.ts"),
        "export function db() {\n  return null;\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/app/api/auth")).unwrap();
    fs::write(
        root.join("src/app/api/auth/route.ts"),
        "import { db } from '@/db/client';\nexport function GET(): Response {\n  return Response.json({ ok: true });\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/app")).unwrap();
    fs::write(
        root.join("src/app/page.tsx"),
        "import { Button } from '@/components/Button';\nexport default function Page() {\n  return Button('hi');\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/lib")).unwrap();
    fs::write(
        root.join("src/lib/utils.ts"),
        "export function formatDate(d: Date): string {\n  return d.toISOString();\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src/services")).unwrap();
    fs::write(
        root.join("src/services/userService.ts"),
        "import { db } from '@/db/client';\nexport function getUser(id: string) {\n  return db();\n}\n",
    )
    .unwrap();
}

fn db_path(root: &Path) -> std::path::PathBuf {
    root.join(".archex/archex.db")
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
    assert_eq!(first.0, 16, "Next.js seeding must create exactly 16 modules");
    assert!(first.1 > 0, "symbols should be extracted");
    assert!(first.2 > 0, "imports should be extracted");
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
    assert!(duplicate_module_names(dir.path()).is_empty(), "no duplicate module names after second init");
}
