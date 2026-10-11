#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// Build a minimal Next.js-shaped project inside `root` so that
/// `archex init` detects Next.js, seeds modules, and indexes symbols/imports.
/// Content is kept JSX-free so the TypeScript grammar parses it cleanly.
pub fn fixture_project(root: &Path) {
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

    fs::write(
        root.join("src/components/Client.tsx"),
        "export function loadUser() {\n  fetch('/api/user');\n}\n",
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

    fs::write(
        root.join("src/app/dashboard.tsx"),
        "export function Dashboard() {\n  fetch('/api/data');\n  return null;\n}\n",
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

/// Two-layer fixture: a committed config.toml and one real cross-boundary
/// import. NO rules — tests write or add the rules they exercise, so a rule's
/// absence is always visible in the test body.
pub fn two_layer_fixture(root: &Path) {
    let archex = root.join(".archex");
    fs::create_dir_all(archex.join("rules")).unwrap();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/db")).unwrap();

    fs::write(
        archex.join("config.toml"),
        r#"schema_version = 1
name = "fixture"
layers = ["ui", "db"]

[[module]]
name = "components"
layer = "ui"
path = "src/components/**"

[[module]]
name = "db"
layer = "db"
path = "src/db/**"
"#,
    )
    .unwrap();

    fs::write(
        root.join("src/db/client.ts"),
        "export function client() {\n  return null;\n}\n",
    )
    .unwrap();

    // A real value import across the boundary: the violation every rule test
    // in this fixture hunts.
    fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '../db/client';\nexport function Widget() {\n  return client();\n}\n",
    )
    .unwrap();
}

/// The boundary rule used by the gate/advisory/MCP agreement tests.
pub fn ui_no_db_rule(root: &Path) {
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
message = "UI must not import the db layer. Go through services."
"#,
    )
    .unwrap();
}

pub fn init(root: &Path) {
    use assert_cmd::Command;
    Command::cargo_bin("archex")
        .unwrap()
        .arg("init")
        .current_dir(root)
        .write_stdin("")
        .assert()
        .success();
}
