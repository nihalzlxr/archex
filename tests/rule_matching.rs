use assert_cmd::Command;
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
