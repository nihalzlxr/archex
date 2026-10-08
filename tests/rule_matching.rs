use assert_cmd::Command;
use predicates::prelude::*;
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

/// A rule pattern containing an uppercase literal (e.g. `Fetch\(`) must still
/// match case-insensitively. Today the regex is compiled with the original
/// pattern case but matched against LOWERCASED content — so any pattern with
/// an uppercase letter silently never matches.
#[test]
fn check_catches_rules_with_uppercase_patterns() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    // Add a rule whose pattern contains an uppercase literal.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "add", "--module", "components", "--rule-type", "forbidden",
               "--description", "No fetch calls in components",
               "--pattern", r"Fetch\("])
        .current_dir(root)
        .assert()
        .success();

    // src/components/Client.tsx contains `fetch(` — the rule must fire.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", "src/components/Client.tsx"])
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicate::str::contains("No fetch calls in components"));
}

/// Regression guard: lowercase seeded rules must keep firing after any
/// matching-semantics change.
#[test]
fn check_still_catches_lowercase_seeded_rules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    // src/app/dashboard.tsx contains `fetch(` — seeded app rule must fire:
    // "Direct API calls without hooks, use src/hooks" (pattern `fetch\(|axios\.`)
    Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", "src/app/dashboard.tsx"])
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicate::str::contains("Direct API calls without hooks"));
}
