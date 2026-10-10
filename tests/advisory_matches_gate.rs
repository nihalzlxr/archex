use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// A two-layer fixture with a committed config.toml and a committed boundary
/// rule. `Widget.tsx` violates `ui-no-db` (a value import across the boundary);
/// `client.ts` is the target side and violates nothing.
fn two_layer_fixture(root: &Path) {
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
        archex.join("rules/ui-no-db.toml"),
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

    fs::write(
        root.join("src/db/client.ts"),
        "export function client() {\n  return null;\n}\n",
    )
    .unwrap();

    // A real value import across the boundary: this is the violation.
    fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '../db/client';\nexport function Widget() {\n  return client();\n}\n",
    )
    .unwrap();
}

fn init(root: &Path) {
    Command::cargo_bin("archex")
        .unwrap()
        .arg("init")
        .current_dir(root)
        .write_stdin("")
        .assert()
        .success();
}

/// The exit test for Step 2.5 item 1: the per-file advisory result for X
/// equals the gate's findings filtered to X, measured on a fixture where X
/// contains a violation. Advisory and gate must never disagree, or agents
/// learn to distrust the advisory.
#[test]
fn advisory_matches_gate_filtered_to_the_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
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