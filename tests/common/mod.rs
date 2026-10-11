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

/// Drives `archex serve` over stdio with hand-written JSON-RPC and returns
/// everything the server wrote for the tool call.
///
/// Two things about rmcp 0.1's stdio transport, both learned the hard way:
/// `clientInfo` requires `version` (a payload without it fails to deserialize,
/// the error is swallowed, and the server reports a misleading "expect
/// initialize request"), and the server tears down on stdin EOF — so stdin has
/// to stay open until the tool response actually arrives.
pub fn call_mcp_tool(root: &Path, tool: &str, arguments: &str) -> String {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command as StdCommand, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let binary = assert_cmd::cargo::cargo_bin("archex");
    let mut child = StdCommand::new(binary)
        .arg("serve")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn archex serve");

    let requests = format!(
        concat!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"test","version":"0"}}}}}}"#,
            "\n",
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized","params":{{}}}}"#,
            "\n",
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#,
            "\n"
        ),
        tool = tool,
        arguments = arguments
    );

    {
        let stdin = child.stdin.as_mut().expect("server stdin");
        stdin.write_all(requests.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    // NOTE: stdin is deliberately NOT dropped here. EOF makes the server exit
    // before it dispatches the tool call.

    let stdout = child.stdout.take().expect("server stdout");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut collected = String::new();
    let deadline = Duration::from_secs(20);
    loop {
        match rx.recv_timeout(deadline) {
            Ok(line) => {
                let is_tool_response = line.contains("\"id\":2");
                collected.push_str(&line);
                collected.push('\n');
                if is_tool_response {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    collected
}

/// One committed decision, in the shape the ledger validator accepts.
pub fn decision_file(id: &str, title: &str, scope: &str, supersedes: &str) -> String {
    format!(
        r#"---
schema_version = 1
id = "{id}"
title = "{title}"
status = "accepted"
supersedes = [{supersedes}]
scope = [{scope}]
author = "human"
recorded_at = "2026-10-08T14:22:00Z"
---
## Context

Billing needed writes and migrations.

## Decision

Drizzle with the existing Postgres. No second ORM.

## Alternatives rejected

Prisma — slower cold starts; a second query builder in the tree.
"#
    )
}

/// Write a decision into the ledger, creating the directory if needed.
pub fn write_decision(
    root: &Path,
    id: &str,
    slug: &str,
    title: &str,
    scope: &str,
    supersedes: &str,
) -> String {
    let dir = root.join(".archex/decisions");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}-{}.md", id, slug));
    fs::write(&path, decision_file(id, title, scope, supersedes)).unwrap();
    path.to_string_lossy().to_string()
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
