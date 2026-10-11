use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture};

const A: &str = "01J8K2A7B3C4D5E6F7G8H9J0KM";
const B: &str = "01J8K2A7B3C4D5E6F7G8H9J0NN";

fn decision_file(id: &str, slug: &str, supersedes: &str, scope: &str) -> String {
    format!(
        r#"---
schema_version = 1
id = "{id}"
title = "Use Drizzle as the ORM"
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

Prisma — slower cold starts.
"#
    )
}

fn write_decision(root: &Path, id: &str, slug: &str, supersedes: &str, scope: &str) -> String {
    let dir = root.join(".archex/decisions");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}-{}.md", id, slug));
    fs::write(&path, decision_file(id, slug, supersedes, scope)).unwrap();
    path.to_string_lossy().to_string()
}

fn sync(root: &Path) -> String {
    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("sync")
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sync must succeed, got:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn sync_imports_decision_files_into_the_index() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, A, "use-drizzle", "", "\"db\"");
    init(root);

    let out = sync(root);
    assert!(
        out.contains("1 imported") || out.contains("1 decision imported"),
        "sync must say what it imported, got:\n{}",
        out
    );
}

#[test]
fn sync_twice_reports_nothing_to_do() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, A, "use-drizzle", "", "\"db\"");
    init(root);
    sync(root);

    let second = sync(root);
    assert!(
        second.contains("1 unchanged"),
        "a second sync must recognize the file is already imported, got:\n{}",
        second
    );
    assert!(
        !second.contains("1 imported"),
        "a second sync must import nothing, got:\n{}",
        second
    );
}

/// The committed files are the record. archex reads them; it does not
/// rewrite, reformat, or "fix" them. Anything else is a sync that can lose
/// a teammate's uncommitted thinking.
#[test]
fn sync_never_modifies_a_committed_decision_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    let path = write_decision(root, A, "use-drizzle", "", "\"db\"");
    init(root);

    let before = fs::read_to_string(&path).unwrap();
    let before_meta = fs::metadata(&path).unwrap().modified().unwrap();

    sync(root);

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "sync rewrote a committed decision file"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        before_meta,
        "sync touched a committed decision file's mtime"
    );
}

#[test]
fn sync_drops_a_deleted_decision_from_the_index() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    let path = write_decision(root, A, "use-drizzle", "", "\"db\"");
    init(root);
    sync(root);

    fs::remove_file(&path).unwrap();
    let out = sync(root);
    assert!(
        out.contains("1 removed"),
        "a deleted record must leave the index, got:\n{}",
        out
    );
}

/// Superseded is derived. sync says how many chains it resolved, so a
/// ledger full of stale decisions cannot look current.
#[test]
fn sync_resolves_the_chain_and_serves_only_its_head() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, A, "use-drizzle", "", "\"db\"");
    write_decision(root, B, "use-kysely", &format!("\"{}\"", A), "\"db\"");
    init(root);

    let out = sync(root);
    assert!(
        out.contains("1 superseded"),
        "the replaced decision must be counted as superseded, got:\n{}",
        out
    );
    assert!(
        out.contains("1 chain head"),
        "only the head of a chain is current, got:\n{}",
        out
    );
}

/// Unmatched scope is a decision nobody will ever be told about. It must be
/// visible, not silently dropped — the same rule as unclassified imports.
#[test]
fn sync_says_when_a_decision_scope_matches_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, A, "use-drizzle", "", "\"ghost\"");
    init(root);

    let out = sync(root);
    assert!(
        out.contains("ghost"),
        "a scope entry that matches no module must be named, got:\n{}",
        out
    );
}

#[test]
fn sync_refuses_an_invalid_decision_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    let dir_path = root.join(".archex/decisions");
    fs::create_dir_all(&dir_path).unwrap();
    fs::write(
        dir_path.join(format!("{}-broken.md", A)),
        "---\nschema_version = 1\nid = \"not-a-ulid\"\n---\n## Decision\nx\n",
    )
    .unwrap();
    init(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("sync")
        .current_dir(root)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "an unreadable ledger file must fail the sync, got:\n{}",
        text
    );
    assert!(
        text.contains("broken.md"),
        "the failing file must be named, got:\n{}",
        text
    );
}