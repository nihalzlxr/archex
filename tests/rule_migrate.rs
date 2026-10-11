use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture};

fn legacy_db_rule(root: &std::path::Path, module: &str, description: &str, pattern: &str) {
    let conn = Connection::open(root.join(".archex/archex.db")).unwrap();
    let module_id: i64 = conn
        .query_row(
            "SELECT id FROM modules WHERE name = ?1",
            [module],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO rules (module_id, rule_type, description, pattern) VALUES (?1, 'forbidden', ?2, ?3)",
        (module_id, description, pattern),
    )
    .unwrap();
}

/// Legacy DB rules become committed files: same intent, validated by the real
/// validator, visible to `rule list`, and enforceable by the gate.
#[test]
fn migrate_converts_legacy_db_rules_to_files() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);
    legacy_db_rule(root, "components", "No direct db imports", "from ['\"]@/db");

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "migrate"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));

    let entries: Vec<String> = std::fs::read_dir(root.join(".archex/rules"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 1, "exactly one file per legacy rule: {:?}", entries);

    let body = std::fs::read_to_string(root.join(".archex/rules").join(&entries[0])).unwrap();
    assert!(body.contains("No direct db imports"), "description carried over:\n{}", body);
    assert!(body.contains(r#"kind = "pattern""#), "a DB pattern rule is a pattern rule:\n{}", body);

    // It is a real file rule now: `rule list` sees it, and it validates.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "list"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("no-direct-db-imports"))
        .stdout(predicate::str::contains("pattern"));
}

/// Migrating twice must not duplicate anything, and with nothing left to
/// migrate it says so rather than claiming success.
#[test]
fn migrate_is_idempotent_and_honest_when_there_is_nothing_to_do() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // Nothing to migrate: plain and specific.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "migrate"])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to migrate"));

    legacy_db_rule(root, "components", "No direct db imports", "from ['\"]@/db");
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "migrate"])
        .current_dir(root)
        .assert()
        .success();

    // Second run: the rule is already a file. No second file, no failure.
    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "migrate"])
        .current_dir(root)
        .assert()
        .success();

    let entries: Vec<_> = std::fs::read_dir(root.join(".archex/rules"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "a second migrate must not duplicate the rule");
}

/// A legacy rule whose module has no layer in config.toml is REFUSED with a
/// clear message. The migration must not invent a layer to make a rule fit —
/// an invented layer is a wrong layer, and a wrong layer is a false violation
/// the moment someone writes a boundary rule against it.
#[test]
fn migrate_refuses_a_rule_whose_module_is_not_in_the_config() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // A module row the DB still knows about, which config.toml does not: the
    // exact shape a pre-2.5 index leaves behind.
    {
        let conn = Connection::open(root.join(".archex/archex.db")).unwrap();
        conn.execute(
            "INSERT INTO modules (name, layer, path_pattern) VALUES ('legacy', 'legacy', 'src/legacy/**')",
            [],
        )
        .unwrap();
    }
    legacy_db_rule(root, "legacy", "Old rule with nowhere to live", "x");

    Command::cargo_bin("archex")
        .unwrap()
        .args(["rule", "migrate"])
        .current_dir(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("legacy"))
        .stderr(predicate::str::contains("config.toml"));

    let entries: Vec<_> = std::fs::read_dir(root.join(".archex/rules"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "a refused rule must not be written anyway: {:?}",
        entries
    );
}

/// While legacy DB rules exist and are ignored, every path that enforces says
/// so. A silently-ignored rule is the exact failure this step exists to fix.
#[test]
fn the_gate_says_when_legacy_db_rules_are_being_ignored() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);
    legacy_db_rule(root, "components", "No direct db imports", "from ['\"]@/db");

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
        text.contains("legacy DB rule") && text.contains("archex rule migrate"),
        "the gate must say legacy DB rules are ignored and how to fix it, got:\n{}",
        text
    );

    // And the per-file advisory says it too, since that is what an agent sees.
    let advisory = Command::cargo_bin("archex")
        .unwrap()
        .args(["check", "--file-path", "src/components/Widget.tsx"])
        .current_dir(root)
        .output()
        .unwrap();
    let advisory_text = format!(
        "{}{}",
        String::from_utf8_lossy(&advisory.stdout),
        String::from_utf8_lossy(&advisory.stderr)
    );
    assert!(
        advisory_text.contains("legacy DB rule"),
        "the advisory must say it too, got:\n{}",
        advisory_text
    );
}