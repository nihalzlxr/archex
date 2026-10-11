use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture, ui_no_db_rule};

#[test]
fn advisory_matches_gate_filtered_to_the_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    let violating = "src/components/Widget.tsx";
    let clean = "src/db/client.ts";

    // The gate sees the boundary violation and fails.
    let gate = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .failure();
    let gate_out = String::from_utf8_lossy(&gate.get_output().stdout).to_string();
    assert!(
        gate_out.contains("ui-no-db"),
        "gate must report the boundary rule, got:\n{}",
        gate_out
    );
    assert!(
        gate_out.contains(violating),
        "gate must name the violating file, got:\n{}",
        gate_out
    );

    // The advisory for the same file must report the same thing and fail too.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", violating])
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicate::str::contains("ui-no-db"))
        .stdout(predicate::str::contains("src/db/client.ts"));

    // The advisory for a clean file is filtered to that file: it passes even
    // though the gate as a whole fails.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", clean])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("ui-no-db").not());
}