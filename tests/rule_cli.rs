use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture, ui_no_db_rule};

fn add_boundary(root: &std::path::Path, id: &str) {
    Command::cargo_bin("archex")
        .unwrap()
        .args([
            "rule", "add",
            "--kind", "boundary",
            "--id", id,
            "--from", "ui",
            "--to", "db",
            "--message", "UI must not import the db layer. Go through services.",
        ])
        .current_dir(root)
        .assert()
        .success();
}

/// A rule added through the CLI must become a FILE and the gate must enforce
/// it. Before Step 2.5, `rule add` wrote a DB row the gate never read: the
/// command appeared to work and enforced nothing.
#[test]
fn rule_add_writes_a_file_the_gate_enforces() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // Non-vacuity: with no rules, the gate passes even though the violating
    // import exists. Whatever fails the gate below, the rule did it.
    Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .success();

    add_boundary(root, "ui-no-db");

    let file = root.join(".archex/rules/ui-no-db.toml");
    let body = std::fs::read_to_string(&file).unwrap_or_else(|_| {
        panic!("rule add must write {}", file.display())
    });
    assert!(body.contains(r#"from_layer = "ui""#), "file must carry the from side:\n{}", body);
    assert!(body.contains(r#"status = "accepted""#), "a new rule is accepted:\n{}", body);

    // The gate enforces the CLI-written rule: same fixture, now failing.
    Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicate::str::contains("ui-no-db"));

    // And the advisory agrees, so agents see it too.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", "src/components/Widget.tsx"])
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicate::str::contains("ui-no-db"));
}

/// Refuse to overwrite. Two rules can never silently share one id.
#[test]
fn rule_add_refuses_to_overwrite() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);
    add_boundary(root, "ui-no-db");

    let file = root.join(".archex/rules/ui-no-db.toml");
    let before = std::fs::read_to_string(&file).unwrap();

    Command::cargo_bin("archex")
        .unwrap()
        .args([
            "rule", "add",
            "--kind", "boundary",
            "--id", "ui-no-db",
            "--from", "ui",
            "--to", "db",
            "--message", "A different message that must never be written.",
        ])
        .current_dir(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"));

    let after = std::fs::read_to_string(&file).unwrap();
    assert_eq!(before, after, "the existing rule file must be untouched");
}

/// An invalid rule must fail at the CLI, not later in CI: unknown layer,
/// flags that do not belong to the declared kind, and a non-slug id are all
/// rejected before anything touches the rules directory.
#[test]
fn rule_add_fails_loudly_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    let cases: &[&[&str]] = &[
        // --from references nothing that exists
        &["rule", "add", "--kind", "boundary", "--id", "bad-layer",
          "--from", "nosuchlayer", "--to", "db", "--message", "x"],
        // boundary rule with a pattern flag: the kind and the flags disagree
        &["rule", "add", "--kind", "boundary", "--id", "mixed-flags",
          "--from", "ui", "--to", "db", "--pattern", "fetch\\(", "--message", "x"],
        // ids are file names: uppercase and spaces are not slugs
        &["rule", "add", "--kind", "boundary", "--id", "Bad Slug",
          "--from", "ui", "--to", "db", "--message", "x"],
        // boundary without --from/--to
        &["rule", "add", "--kind", "boundary", "--id", "half-boundary",
          "--message", "x"],
        // unknown kind outright
        &["rule", "add", "--kind", "quantum", "--id", "x",
          "--from", "ui", "--to", "db", "--message", "x"],
    ];

    for case in cases {
        Command::cargo_bin("archex")
            .unwrap()
            .args(*case)
            .current_dir(root)
            .assert()
            .failure();
    }

    let rules_dir = root.join(".archex/rules");
    let entries: Vec<_> = std::fs::read_dir(&rules_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "no invalid case may write a rule file, found: {:?}",
        entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
    );
}

/// `rule list` reads the committed files — a rule that exists only in the
/// legacy DB must not appear, and a file rule must.
#[test]
fn rule_list_reads_files_not_the_db() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "list"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("ui-no-db"))
        .stdout(predicate::str::contains("boundary"));

    let dir2 = TempDir::new().unwrap();
    let root2 = dir2.path();
    two_layer_fixture(root2);
    init(root2);

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "list"])
        .current_dir(root2)
        .assert()
        .success()
        .stdout(predicate::str::contains("no rules"));
}

/// `rule remove` retires by default: the file stays (decisions will reference
/// rule ids), the status flips, the gate stops enforcing, and a second remove
/// is honest about there being nothing to do.
#[test]
fn rule_remove_retires_by_default() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    // The rule is live and enforced.
    Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .failure();

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--id", "ui-no-db"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("retired"));

    let body = std::fs::read_to_string(root.join(".archex/rules/ui-no-db.toml")).unwrap();
    assert!(body.contains(r#"status = "retired""#), "file must say retired:\n{}", body);

    // Retired rules enforce nothing.
    Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .success();

    // The file is still listed, marked.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "list"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("retired"));

    // Idempotent: retiring a retired rule says so and succeeds.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--id", "ui-no-db"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("already retired"));

    // Removing an unknown id fails with the ids that do exist.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--id", "nosuchrule"])
        .current_dir(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("ui-no-db"));
}

/// `--purge` deletes the file for real. Git is the undo, so no prompt.
#[test]
fn rule_remove_purge_deletes_the_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "remove", "--id", "ui-no-db", "--purge"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("deleted"));

    assert!(
        !root.join(".archex/rules/ui-no-db.toml").exists(),
        "--purge must remove the file"
    );
}

/// A pattern rule can be written and listed, and the CLI says plainly that it
/// is not enforced yet — the same honesty `load_rules` already has.
#[test]
fn rule_add_pattern_rule_says_it_is_not_enforced_yet() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    Command::cargo_bin("archex")
        .unwrap()
        .args([
            "rule", "add",
            "--kind", "pattern",
            "--id", "no-fetch-in-ui",
            "--scope", "src/components/**",
            "--mode", "forbidden",
            "--pattern", r"fetch\(",
            "--message", "Use the data layer, not direct fetches.",
        ])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("not enforced yet"));

    assert!(root.join(".archex/rules/no-fetch-in-ui.toml").exists());
}