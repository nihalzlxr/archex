use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

use common::{call_mcp_tool, init, two_layer_fixture};

/// Every symbol_type archex stored for this repo. The index is the only
/// place the parser's typing decisions are observable, so this reads it.
fn archex_db_rows(root: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join(".archex/archex.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT symbol_type FROM symbols")
        .expect("symbols table");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("symbols rows");
    rows.filter_map(|r| r.ok()).collect()
}

fn check(root: &Path) -> String {
    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn boundary_rule(root: &Path) {
    fs::create_dir_all(root.join(".archex/rules")).unwrap();
    fs::write(
        root.join(".archex/rules/ui-no-db.toml"),
        r#"schema_version = 1
id = "ui-no-db"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "db"
message = "UI must not import the db layer."
"#,
    )
    .unwrap();
}

/// A pnpm monorepo, measured on project/fitness: a workspace import
/// resolves through node_modules to `packages/engine/dist/index.js`, which
/// is build output. archex indexes `packages/engine/src/**`, so the edge
/// resolves to a file no module owns, and the graph never sees it.
///
/// The rule then reports "0 candidate edges" — which reads as a clean run.
/// It is not a clean run. It is a hole in the graph wearing one.
#[test]
fn an_edge_that_resolves_outside_the_index_is_counted_not_dropped() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    boundary_rule(root);

    // The db module owns src/db/** only. Its package entry point lives in
    // dist/, which is gitignored and therefore never indexed.
    fs::create_dir_all(root.join("packages/db/dist")).unwrap();
    fs::write(
        root.join("packages/db/package.json"),
        r#"{ "name": "@fixture/db", "main": "./dist/index.js" }"#,
    )
    .unwrap();
    fs::write(
        root.join("packages/db/dist/index.js"),
        "export const client = () => null;\n",
    )
    .unwrap();
    fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '@fixture/db';\nexport function Widget() {\n  return client();\n}\n",
    )
    .unwrap();

    let config = fs::read_to_string(root.join(".archex/config.toml")).unwrap();
    fs::write(
        root.join(".archex/config.toml"),
        config.replace(
            r#"name = "db"
layer = "db"
path = "src/db/**""#,
            r#"name = "db"
layer = "db"
path = "packages/db/**""#,
        ),
    )
    .unwrap();

    init(root);
    let out = check(root);

    assert!(
        out.contains("OUTSIDE the index"),
        "an edge that resolves to an unindexed file must be counted, got:\n{}",
        out
    );
    assert!(
        out.contains("packages/db/dist/index.js"),
        "and the graph hole must be named, got:\n{}",
        out
    );
}

/// The same hole, from the rule's side: a boundary rule whose from-files
/// and to-files exist but whose edges all vanish into build output must
/// not read as "clean". It must say the rule could not judge them.
#[test]
fn a_rule_whose_edges_all_left_the_graph_says_it_could_not_judge_them() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    boundary_rule(root);

    fs::create_dir_all(root.join("packages/db/dist")).unwrap();
    fs::write(
        root.join("packages/db/package.json"),
        r#"{ "name": "@fixture/db", "main": "./dist/index.js" }"#,
    )
    .unwrap();
    fs::write(
        root.join("packages/db/dist/index.js"),
        "export const client = () => null;\n",
    )
    .unwrap();
    fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '@fixture/db';\nexport function Widget() {\n  return client();\n}\n",
    )
    .unwrap();
    let config = fs::read_to_string(root.join(".archex/config.toml")).unwrap();
    fs::write(
        root.join(".archex/config.toml"),
        config.replace(
            r#"name = "db"
layer = "db"
path = "src/db/**""#,
            r#"name = "db"
layer = "db"
path = "packages/db/**""#,
        ),
    )
    .unwrap();

    init(root);
    let out = check(root);

    assert!(
        out.contains("could not judge"),
        "a rule that examined nothing because the graph has holes must say so, got:\n{}",
        out
    );
}

/// Measured on fitness: 906 of 951 "unclassified" imports were react, next
/// and drizzle-orm — npm packages that are never in the repo and never
/// will be. Counting them next to the 23 genuinely unresolved first-party
/// imports means the number that matters is invisible.
#[test]
fn external_packages_are_not_counted_as_unresolved_imports() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '../db/client';\n\
         import { useState } from 'react';\n\
         import { next } from 'next/headers';\n\
         import { broken } from '@/lib/does-not-exist';\n\
         export function Widget() {\n  return client();\n}\n",
    )
    .unwrap();

    init(root);
    let out = check(root);

    assert!(
        out.contains("2 external package import(s)"),
        "npm packages must be counted as themselves, not as gaps, got:\n{}",
        out
    );
    assert!(
        out.contains("1 unresolved first-party import(s)"),
        "the first-party gap is the number that matters, got:\n{}",
        out
    );
    assert!(
        out.contains("does-not-exist"),
        "and it must be named, got:\n{}",
        out
    );
}
/// Measured on fitness: `@sentry/nextjs`, `@radix-ui/react-dialog` and
/// `@better-auth/passkey` do not resolve, and no syntactic rule can say
/// whether they are npm packages or an alias to a directory called
/// `radix-ui`. Guessing either way is the one thing the resolver refuses
/// to do, so the gate reports the ambiguity instead of folding it into a
/// count that means something else.
#[test]
fn a_scoped_specifier_that_did_not_resolve_is_reported_as_ambiguous() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '../db/client';\n\
         import { capture } from '@sentry/nextjs';\n\
         import { Dialog } from '@radix-ui/react-dialog';\n\
         import { gone } from '../lib/gone';\n\
         export function Widget() {\n  return client();\n}\n",
    )
    .unwrap();

    init(root);
    let out = check(root);

    assert!(
        out.contains("2 ambiguous import(s)"),
        "a scoped specifier that did not resolve must not be filed under \
         either count, got:\n{}",
        out
    );
    assert!(
        out.contains("radix-ui/react-dialog"),
        "and it must be named, got:\n{}",
        out
    );
    assert!(
        out.contains("1 unresolved first-party import(s)"),
        "while the unambiguous gap stays in its own count, got:\n{}",
        out
    );
}

/// Defect 1: every Python and Rust import was extracted with
/// `module_id: 0` and an empty `file_path`. The imports table has a foreign
/// key on module_id, so the whole per-file batch failed — and the error was
/// discarded with `.ok()`, while `init` still counted them as extracted.
///
/// `import os` in a .py file is the smallest possible witness: if the row
/// survives, the gate counts it as an external package.
#[test]
fn a_python_import_survives_the_index() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    fs::write(
        root.join("src/db/helpers.py"),
        "import os\n\n\ndef read(path):\n    return os.path.exists(path)\n",
    )
    .unwrap();

    init(root);
    let out = check(root);

    assert!(
        out.contains("1 external package import(s)"),
        "a Python import must reach the index, got:\n{}",
        out
    );
}

/// The same defect for Rust, and the same failure mode: the import is
/// counted by `init` and never stored.
#[test]
fn a_rust_import_survives_the_index() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    fs::write(
        root.join("src/db/client.rs"),
        "use serde::Serialize;\n\npub fn connect() -> i32 { 0 }\n",
    )
    .unwrap();

    init(root);
    let out = check(root);

    assert!(
        out.contains("1 unresolved first-party import(s)"),
        "a Rust use statement must reach the index as an unclassified edge, got:\n{}",
        out
    );
}

/// Defect 2: `.tsx` was parsed with the plain TypeScript grammar, which has
/// no JSX node types at all. Tree-sitter error recovery then produced a
/// malformed tree and symbols adjacent to JSX were silently lost — the file
/// that mattered most in a React codebase was the file we understood least.
#[test]
fn symbols_next_to_jsx_are_not_lost() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    fs::write(
        root.join("src/components/Panel.tsx"),
        r#"export function First() {
  return <div className="a">one</div>;
}

export function Second() {
  const rows = [1, 2, 3];
  return rows.map((row) => <span key={row}>{row}</span>);
}

export function Third() {
  return <ul><li>three</li></ul>;
}
"#,
    )
    .unwrap();

    init(root);

    let found = call_mcp_tool(
        root,
        "find_similar",
        &format!(r#"{{"description":"{}"}}"#, "Third"),
    );
    for symbol in ["First", "Second", "Third"] {
        assert!(
            found.contains(symbol),
            "a function declared in a .tsx file must be indexed, {} was lost. \
             Full response:\n{}",
            symbol,
            found
        );
    }
}
