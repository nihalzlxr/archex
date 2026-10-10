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
        .args(["rule", "add", "--module", "src", "--rule-type", "forbidden",
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

/// Regression guard: a lowercase pattern must keep matching after any
/// change to matching semantics. The rule is added explicitly rather than
/// seeded -- seeding creates no rules any more, on purpose.
#[test]
fn check_still_catches_lowercase_rules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "add", "--module", "src", "--rule-type", "forbidden",
               "--description", "No direct fetches", "--pattern", r"fetch\("])
        .current_dir(root)
        .assert()
        .success();

    // src/components/Client.tsx contains `fetch(`.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", "src/components/Client.tsx"])
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicate::str::contains("No direct fetches"));
    let root = dir.path();
    fixture_project(root);
    init(root);

    // Add a rule whose pattern contains an uppercase literal.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "add", "--module", "src", "--rule-type", "forbidden",
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

/// `rule add` prints an ID. It was printing the total rule COUNT, not the ID
/// of the rule just created — the two only agree while nothing is ever
/// deleted. Add one, delete it, add another: the second printed ID must not
/// be the count, and `rule remove` must be able to delete it by that ID.
#[test]
fn rule_add_reports_the_id_of_the_rule_it_created() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    let add = |description: &str| {
        Command::cargo_bin("archex")
            .unwrap()
            .args(["rule", "add", "--module", "src", "--rule-type", "forbidden",
                   "--description", description, "--pattern", "nope"])
            .current_dir(root)
            .output()
            .unwrap()
    };

    let stdout_of = |out: &std::process::Output| String::from_utf8_lossy(&out.stdout).to_string();
    let id_in = |text: &str| -> i64 {
        text.lines()
            .find_map(|l| l.trim().strip_prefix("[OK] Rule added successfully (ID: ").and_then(|r| r.strip_suffix(")")))
            .and_then(|r| r.trim().parse().ok())
            .unwrap_or_else(|| panic!("no rule id in output:\n{}", text))
    };

    let first = id_in(&stdout_of(&add("first rule")));
    // Delete it, so the count and the id sequence diverge.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--rule-id", &first.to_string()])
        .current_dir(root)
        .write_stdin("y\n")
        .assert()
        .success();

    let second = id_in(&stdout_of(&add("second rule")));
    assert_ne!(
        second, first,
        "printed the rule count, not the new rule's id"
    );
    let reported = second;

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
