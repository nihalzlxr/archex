use assert_cmd::Command;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture, ui_no_db_rule};

/// Step 3.9: hook pre-edit runs check on a file and returns violations
/// hook post-edit runs check + typecheck + tests (for now just check)
#[test]
fn hook_pre_edit_reports_violations_on_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    // Pre-edit on a violating file should report the violation
    let out = Command::cargo_bin("archex")
        .unwrap()
        .args(["hook", "pre-edit", "src/components/Widget.tsx"])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(out.status.success(), "pre-edit should exit 0 even with violations (fail open)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ui-no-db"), "pre-edit must report the boundary violation");
    assert!(stdout.contains("src/db/client.ts"), "pre-edit must name the import target");
}

#[test]
fn hook_pre_edit_is_clean_on_compliant_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    // Pre-edit on a clean file should pass
    let out = Command::cargo_bin("archex")
        .unwrap()
        .args(["hook", "pre-edit", "src/db/client.ts"])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(out.status.success(), "pre-edit on clean file should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Clean"), "pre-edit must report clean");
    assert!(!stdout.contains("ui-no-db"), "clean file must not report violations");
}

#[test]
fn hook_post_edit_runs_verification() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    // Post-edit on a violating file should report the violation
    let out = Command::cargo_bin("archex")
        .unwrap()
        .args(["hook", "post-edit", "src/components/Widget.tsx"])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(out.status.success(), "post-edit should exit 0 even with violations (fail open)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ui-no-db"), "post-edit must report the boundary violation");
}

#[test]
fn hook_command_exists() {
    let out = Command::cargo_bin("archex")
        .unwrap()
        .args(["hook", "--help"])
        .output()
        .unwrap();

    assert!(out.status.success(), "hook --help should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pre-edit"), "hook must have pre-edit subcommand");
    assert!(stdout.contains("post-edit"), "hook must have post-edit subcommand");
}

/// Step 3.9: setup --agent opencode wires hooks
#[test]
fn setup_opencode_wires_hooks() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    // Run setup for opencode
    let binary = assert_cmd::cargo::cargo_bin("archex");
    Command::new(&binary)
        .args(["setup", "--agent", "opencode"])
        .current_dir(root)
        .assert()
        .success();

    // Check that opencode.json has hooks configured
    let config = std::fs::read_to_string(root.join("opencode.json")).unwrap();
    assert!(config.contains("pre_edit"), "opencode config must have pre_edit hook");
    assert!(config.contains("post_edit"), "opencode config must have post_edit hook");
    assert!(config.contains("hook"), "opencode hooks must reference archex hook command");
}