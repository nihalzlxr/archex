use assert_cmd::Command;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture};

/// A two-layer fixture with three ways for `components` to reach `db`:
/// a plain value import, a type-only import, and a dynamic import. Only the
/// first and third create runtime coupling.
fn import_kind_fixture(root: &std::path::Path) {
    two_layer_fixture(root);

    std::fs::write(
        root.join("src/db/client.ts"),
        "export function client() {\n  return null;\n}\nexport interface Row {\n  id: string;\n}\n",
    )
    .unwrap();

    // A value import across the boundary: a real violation.
    std::fs::write(
        root.join("src/components/Widget.tsx"),
        "import { client } from '../db/client';\nexport function Widget() {\n  return client();\n}\n",
    )
    .unwrap();

    // A type-only import: compiles to nothing. It cannot create a runtime
    // dependency, so flagging it is a false positive by construction.
    std::fs::write(
        root.join("src/components/Types.tsx"),
        "import type { Row } from '../db/client';\nexport function useRow(r: Row) {\n  return r.id;\n}\n",
    )
    .unwrap();

    // A dynamic import: runtime coupling, just deferred.
    std::fs::write(
        root.join("src/components/Loader.tsx"),
        "export async function load() {\n  return import('../db/client');\n}\n",
    )
    .unwrap();
}

fn boundary_rule(root: &std::path::Path, extra: &str) {
    std::fs::write(
        root.join(".archex/rules/ui-no-db.toml"),
        format!(
            r#"schema_version = 1
id = "ui-no-db"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "ui"
to_layer = "db"
{extra}message = "UI must not import the db layer. Go through services."
"#
        ),
    )
    .unwrap();
}

/// The exit behaviour of the kind column: a type-only edge is not a violation
/// by default, a dynamic import still is. 6.7% of LORB's edges are type-only —
/// flagging them is how a guard cries wolf.
#[test]
fn type_only_edges_are_skipped_by_default_but_runtime_edges_still_fire() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    import_kind_fixture(root);
    boundary_rule(root, "");
    init(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !out.status.success(),
        "the value import must still fail the gate, got:\n{}",
        stdout
    );

    assert!(
        stdout.contains("src/components/Widget.tsx"),
        "a value import is a violation:\n{}",
        stdout
    );
    assert!(
        stdout.contains("src/components/Loader.tsx"),
        "a dynamic import is runtime coupling and must fire:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("src/components/Types.tsx"),
        "a type-only import must not fire by default:\n{}",
        stdout
    );
}

/// `include_type_only = true` is the opt-in: compile-time coupling is still
/// coupling, and some teams want to see it. Per-rule, never a blanket skip.
#[test]
fn include_type_only_opts_back_in() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    import_kind_fixture(root);
    boundary_rule(root, "include_type_only = true\n");
    init(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !out.status.success(),
        "expected failures, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("src/components/Types.tsx"),
        "include_type_only = true must make type-only edges fire:\n{}",
        stdout
    );
}

/// A type-only import must not become a coverage hole either: it is a real
/// edge, classified, not dropped from the index.
#[test]
fn type_only_imports_still_count_as_classified() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    import_kind_fixture(root);
    boundary_rule(root, "");
    init(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        stdout.contains("0 unresolved first-party import(s)")
            && stdout.contains("0 external package import(s)")
            && !stdout.contains("OUTSIDE the index"),
        "every import in the fixture resolves, including the type-only one:\n{}",
        stdout
    );
}
/// `reachable = true` is rejected at validation until the engine computes
/// transitive closure. A field that is accepted and silently does nothing is
/// the silently-empty-rule failure arriving through the front door: the rule
/// would read as transitive while enforcing direct edges only.
#[test]
fn reachable_true_is_rejected_until_the_engine_computes_closure() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    import_kind_fixture(root);
    boundary_rule(root, "reachable = true\n");
    init(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "a rule that cannot be honored must not load:\n{}",
        text
    );
    assert!(
        text.contains("reachable") && text.contains("not supported"),
        "the error must name the field and why, got:\n{}",
        text
    );
}

/// The gate reports how many type-only edges it did not judge. A rule that
/// quietly skipped everything must never look like a clean run.
#[test]
fn the_gate_reports_type_only_edges_it_did_not_judge() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    import_kind_fixture(root);
    boundary_rule(root, "");
    init(root);

    let out = Command::cargo_bin("archex")
        .unwrap()
        .arg("check")
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        stdout.contains("type-only edge(s) not judged"),
        "skipped edges must be counted, not swallowed:\n{}",
        stdout
    );
}
