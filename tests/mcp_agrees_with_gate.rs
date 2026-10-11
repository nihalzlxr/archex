use assert_cmd::Command;
use tempfile::TempDir;

mod common;

use common::{call_mcp_tool, init, two_layer_fixture, ui_no_db_rule};

/// `check_drift` is the tool agents actually call. It must report the same
/// violation the gate and the advisory report — same rule id, same target
/// file. Today it reads DB rules, so it answers "clean" on a file the gate
/// blocks: the agent is told there is nothing to fix.
#[test]
fn mcp_check_drift_agrees_with_the_gate() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    let out = call_mcp_tool(
        root,
        "check_drift",
        r#"{"file_path":"src/components/Widget.tsx"}"#,
    );

    assert!(
        out.contains("ui-no-db"),
        "MCP check_drift must report the boundary rule the gate reports, got:\n{}",
        out
    );
    assert!(
        out.contains("src/db/client.ts"),
        "MCP check_drift must name the import target, got:\n{}",
        out
    );
}

/// A clean file stays clean over MCP — the tool must not become
/// violation-happy in the other direction.
#[test]
fn mcp_check_drift_is_quiet_on_a_clean_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    let out = call_mcp_tool(
        root,
        "check_drift",
        r#"{"file_path":"src/db/client.ts"}"#,
    );

    assert!(
        !out.contains("ui-no-db"),
        "a clean file must not report a boundary violation, got:\n{}",
        out
    );
}

/// Guards the fixture itself: this file must still be a real violation under
/// the gate, or both "green" tests above would be vacuous.
#[test]
fn the_fixture_is_still_a_real_violation_for_the_gate() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    ui_no_db_rule(root);
    init(root);

    Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .assert()
        .failure()
        .stdout(predicates::str::contains("ui-no-db"));
}