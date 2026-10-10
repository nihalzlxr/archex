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

/// The pattern engines have no file-based implementation yet: the advisory
/// runs the boundary engine only, and `load_rules` warns that a `pattern` rule
/// is not enforced. So there is no end-to-end path to assert here yet.
///
/// What is asserted is that the engine cannot be left silently unenforced: a
/// committed `kind = "pattern"` rule must say so, on every path that loads it.
#[test]
fn an_unenforceable_rule_says_so_instead_of_looking_enforced() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fixture_project(root);
    init(root);

    let archex = root.join(".archex");
    std::fs::create_dir_all(archex.join("rules")).unwrap();
    std::fs::write(
        archex.join("rules/no-fetch.toml"),
        r#"schema_version = 1
id = "no-fetch"
status = "accepted"
kind = "pattern"
severity = "error"

[pattern]
scope = "src/components/**"
mode = "forbidden"
pattern = "fetch\\("
message = "No direct fetches in components."
"#,
    )
    .unwrap();

    // The gate loads it, does not enforce it, and says so.
    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("no-fetch") && stderr.contains("NOT enforced"),
        "a loaded-but-unenforced rule must be reported, got stderr:\n{}",
        stderr
    );
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
