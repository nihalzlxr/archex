use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture};

const OLD: &str = "01J8K2A7B3C4D5E6F7G8H9J0KM";
const NEW: &str = "01J8K2A7B3C4D5E6F7G8H9J0NN";

fn committed_decision(root: &Path, id: &str, slug: &str, supersedes: &str, title: &str) {
    let dir = root.join(".archex/decisions");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{}-{}.md", id, slug)),
        format!(
            r#"---
schema_version = 1
id = "{id}"
title = "{title}"
status = "accepted"
supersedes = [{supersedes}]
scope = ["db"]
author = "human"
recorded_at = "2026-10-08T14:22:00Z"
---
## Context

Billing needed writes.

## Decision

Ship it.

## Alternatives rejected

Nothing else was considered.
"#
        ),
    )
    .unwrap();
}

fn decide(root: &Path, args: &[&str]) -> String {
    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("decide")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn decide_add_writes_a_file_the_validator_accepts() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    let out = decide(
        root,
        &[
            "add",
            "--title",
            "Use Drizzle as the ORM",
            "--decision",
            "Drizzle with the existing Postgres.",
            "--context",
            "Billing needed writes and migrations.",
            "--alternative",
            "Prisma — slower cold starts.",
            "--scope",
            "db",
        ],
    );
    assert!(
        out.contains("Decision written"),
        "must say it wrote a file, got:\n{}",
        out
    );

    let dir_path = root.join(".archex/decisions");
    let written: Vec<std::path::PathBuf> = fs::read_dir(&dir_path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(written.len(), 1, "one decision, one file");

    let name = written[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        name.ends_with(".md"),
        "decisions are markdown, got {}",
        name
    );
    let stem = name.trim_end_matches(".md");
    let (id, slug) = stem.split_once('-').expect("<ULID>-<slug>.md");
    assert_eq!(id.len(), 26, "ids are ULIDs, got {}", id);
    assert!(!slug.is_empty() && slug.chars().all(|c| c.is_ascii_lowercase() || c == '-'));

    let body = fs::read_to_string(&written[0]).unwrap();
    assert!(body.contains("## Decision"), "got:\n{}", body);
    assert!(body.contains("## Alternatives rejected"), "got:\n{}", body);
    assert!(body.contains("Use Drizzle as the ORM"), "got:\n{}", body);
}

/// The headline feature of the ledger cannot be a row in a local database:
/// a decision that is not committed is a decision no teammate's agent ever
/// reads. This is the same trap `archex rule` walked out of in Step 2.5.
#[test]
fn decide_add_writes_nothing_to_the_database() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    decide(
        root,
        &[
            "add",
            "--title",
            "Use Drizzle as the ORM",
            "--decision",
            "Drizzle with the existing Postgres.",
            "--scope",
            "db",
        ],
    );

    // Before sync, the index knows nothing. The file is the record.
    let out = decide(root, &["list"]);
    assert!(
        out.contains("Use Drizzle"),
        "list must read the committed files, not the index, got:\n{}",
        out
    );
}

#[test]
fn decide_add_refuses_an_invalid_scope_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .args([
            "decide",
            "add",
            "--title",
            "Use Drizzle",
            "--decision",
            "Drizzle.",
            "--scope",
            "src/db",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "a path is not a scope, got:\n{}", text);
    assert!(
        text.contains("scope") && text.contains("src/db"),
        "the error must name the field and the value, got:\n{}",
        text
    );
    assert!(
        !root.join(".archex/decisions").exists()
            || fs::read_dir(root.join(".archex/decisions")).unwrap().count() == 0,
        "nothing may be written when validation fails, got:\n{}",
        text
    );
}

#[test]
fn an_agents_decision_is_a_proposal_until_a_human_accepts_it() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    decide(
        root,
        &[
            "add",
            "--title",
            "Cache user lookups",
            "--decision",
            "Memoize for 30s.",
            "--author",
            "agent",
        ],
    );
    let body = fs::read_to_string(
        fs::read_dir(root.join(".archex/decisions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(
        body.contains("status = \"proposed\"") && body.contains("author = \"agent\""),
        "an agent proposes, a human accepts — got:\n{}",
        body
    );
}

#[test]
fn decide_list_serves_only_chain_heads() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    committed_decision(root, OLD, "use-drizzle", "", "Use Drizzle as the ORM");
    committed_decision(
        root,
        NEW,
        "use-kysely",
        &format!("\"{}\"", OLD),
        "Use Kysely as the ORM",
    );

    let out = decide(root, &["list"]);
    assert!(
        out.contains("Use Kysely as the ORM"),
        "the head must be listed, got:\n{}",
        out
    );
    assert!(
        !out.contains("Use Drizzle as the ORM"),
        "a superseded decision must not be served as current, got:\n{}",
        out
    );
}

#[test]
fn decide_show_prints_the_whole_record_including_alternatives() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    committed_decision(root, OLD, "use-drizzle", "", "Use Drizzle as the ORM");

    let out = decide(root, &["show", OLD]);
    assert!(out.contains("Use Drizzle as the ORM"), "got:\n{}", out);
    assert!(
        out.contains("Nothing else was considered"),
        "the why-not is the part an agent needs, got:\n{}",
        out
    );
}

/// An empty ledger is a valid repo, and it must say where records go
/// instead of printing nothing at all.
#[test]
fn decide_list_on_an_empty_ledger_says_where_decisions_live() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);

    let out = decide(root, &["list"]);
    assert!(
        out.contains(".archex/decisions"),
        "an empty ledger must point at the directory, got:\n{}",
        out
    );
}