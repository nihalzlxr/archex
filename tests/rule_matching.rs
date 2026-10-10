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

/// `rule add` prints an ID. It was printing the total rule COUNT, not the ID
/// of the rule just created — the two only agree while nothing is ever
/// deleted. Delete a seeded rule, then add one: the printed ID must be the
/// new rule's, and `rule remove` must be able to delete it by that ID.
#[test]
fn rule_add_reports_the_id_of_the_rule_it_created() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    // Delete a seeded rule so ids and counts diverge.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--rule-id", "1"])
        .current_dir(root)
        .write_stdin("y\n")
        .assert()
        .success();

    let output = Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "add", "--module", "components", "--rule-type", "forbidden",
               "--description", "A fresh rule", "--pattern", "nope"])
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Count is back to 8; the new rule's id must not be 8.
    let reported: i64 = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("[OK] Rule added successfully (ID: ").and_then(|r| r.strip_suffix(")")))
        .and_then(|r| r.trim().parse().ok())
        .unwrap_or_else(|| panic!("no rule id in output:\n{}", stdout));
    assert_ne!(reported, 8, "printed the rule count, not the new rule's id");

    // The reported ID must address the rule that was just created.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--rule-id", &reported.to_string()])
        .current_dir(root)
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(&format!("Rule {} deleted", reported)));
}
