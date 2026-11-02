use assert_cmd::Command;
use tempfile::TempDir;

mod common;

use common::{fixture_project, two_layer_fixture};

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

/// Step 3.8: proposed rules are report-only (exit 0), accepted rules enforce (exit 1)
#[test]
fn proposed_rule_reports_but_exits_zero_accepted_enforces() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // Add a PROPOSED boundary rule (same violation as ui-no-db but proposed)
    std::fs::write(
        root.join(".archex/rules/ui-no-db-proposed.toml"),
        r#"schema_version = 1
id = "ui-no-db-proposed"
status = "proposed"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "db"
message = "UI must not import the db layer (proposed)."
"#,
    )
    .unwrap();

    // The gate should REPORT the proposed rule but EXIT 0
    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    
    let stdout = String::from_utf8_lossy(&out.stdout);
    let exit_code = out.status.code().unwrap_or(-1);
    
    // Proposed rule should be reported
    assert!(stdout.contains("ui-no-db-proposed"), "proposed rule must be reported, got:\n{}", stdout);
    // But exit code should be 0 (not enforced)
    assert_eq!(exit_code, 0, "proposed rule must exit 0, got exit code {}:\n{}", exit_code, stdout);

    // Now change to ACCEPTED - should enforce (exit 1)
    std::fs::write(
        root.join(".archex/rules/ui-no-db-proposed.toml"),
        r#"schema_version = 1
id = "ui-no-db-proposed"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "db"
message = "UI must not import the db layer (proposed)."
"#,
    )
    .unwrap();

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    
    let stdout = String::from_utf8_lossy(&out.stdout);
    let exit_code = out.status.code().unwrap_or(-1);
    
    // Accepted rule should be reported AND enforced
    assert!(stdout.contains("ui-no-db-proposed"), "accepted rule must be reported, got:\n{}", stdout);
    assert_eq!(exit_code, 1, "accepted rule must exit 1, got exit code {}:\n{}", exit_code, stdout);
}
