use std::path::Path;
use tempfile::TempDir;

mod common;

use common::{call_mcp_tool, init, two_layer_fixture, write_decision};

const OLD: &str = "01J8K2A7B3C4D5E6F7G8H9J0KM";
const NEW: &str = "01J8K2A7B3C4D5E6F7G8H9J0NN";

fn context_for(root: &Path, file: &str) -> String {
    call_mcp_tool(root, "get_context", &format!(r#"{{"file_path":"{}"}}"#, file))
}

/// The whole point of the ledger: an agent opening a file is told why it is
/// built the way it is. No decision in the context means the session starts
/// from zero and re-litigates the choice in the first PR it touches.
#[test]
fn get_context_carries_the_decisions_scoped_to_the_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, OLD, "use-drizzle", "Use Drizzle as the ORM", "\"db\"", "");
    init(root);

    let out = context_for(root, "src/db/client.ts");
    assert!(
        out.contains("Use Drizzle as the ORM"),
        "a decision scoped to db must reach a db file, got:\n{}",
        out
    );
    assert!(
        out.contains("Alternatives rejected"),
        "the rejected alternative is the half an agent cannot reconstruct, got:\n{}",
        out
    );
}

/// Injection is scope-matched, not keyword-matched. A decision about the
/// components layer must not appear in the context of a database file.
#[test]
fn a_decision_scoped_elsewhere_is_not_injected() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(
        root,
        OLD,
        "components-decide",
        "Components own no data fetching",
        "\"components\"",
        "",
    );
    init(root);

    // It must be in ITS file's context, or the negative below would pass
    // for the wrong reason.
    let scoped = context_for(root, "src/components/Widget.tsx");
    assert!(
        scoped.contains("Components own no data fetching"),
        "the decision must be injected where it is scoped, got:\n{}",
        scoped
    );

    let out = context_for(root, "src/db/client.ts");
    assert!(
        !out.contains("Components own no data fetching"),
        "a components decision must not be injected into a db file, got:\n{}",
        out
    );
}

/// A layer is a legal scope, so "everything in db does X" needs no
/// per-module list.
#[test]
fn a_layer_scoped_decision_reaches_every_file_in_that_layer() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, OLD, "ui-rule", "UI never talks to the db", "\"ui\"", "");
    init(root);

    let out = context_for(root, "src/components/Widget.tsx");
    assert!(
        out.contains("UI never talks to the db"),
        "a layer-scoped decision must reach a file in that layer, got:\n{}",
        out
    );
}

/// Superseded is derived from the chain, and only heads are ever injected —
/// an agent told a replaced decision has been told a stale fact.
#[test]
fn a_superseded_decision_is_not_served() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    write_decision(root, OLD, "use-drizzle", "Use Drizzle as the ORM", "\"db\"", "");
    write_decision(
        root,
        NEW,
        "use-kysely",
        "Use Kysely as the ORM",
        "\"db\"",
        &format!("\"{}\"", OLD),
    );
    init(root);

    let out = context_for(root, "src/db/client.ts");
    assert!(
        out.contains("Use Kysely as the ORM"),
        "the head of the chain must be served, got:\n{}",
        out
    );
    assert!(
        !out.contains("Use Drizzle as the ORM"),
        "a superseded decision must never be injected, got:\n{}",
        out
    );
}

/// The promise that makes the ledger worth committing: a teammate's
/// `git pull` gives your agents their decisions, with no command to
/// remember. sync is not the trigger here — server start is.
#[test]
fn a_decision_written_after_init_reaches_the_next_session() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // Nothing was committed at init; the record lands later, as it does
    // when a teammate's PR merges.
    write_decision(root, OLD, "use-drizzle", "Use Drizzle as the ORM", "\"db\"", "");

    let out = context_for(root, "src/db/client.ts");
    assert!(
        out.contains("Use Drizzle as the ORM"),
        "the server must pick the record up without being told to sync, got:\n{}",
        out
    );
}

/// A deleted record must stop being injected. Otherwise a withdrawn
/// decision outlives the pull that removed it.
#[test]
fn a_deleted_decision_stops_being_injected() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    let path = write_decision(root, OLD, "use-drizzle", "Use Drizzle as the ORM", "\"db\"", "");
    init(root);

    assert!(context_for(root, "src/db/client.ts").contains("Use Drizzle"));

    std::fs::remove_file(&path).unwrap();
    let out = context_for(root, "src/db/client.ts");
    assert!(
        !out.contains("Use Drizzle"),
        "a decision deleted from git must not keep being injected, got:\n{}",
        out
    );
}

/// Context is a budget. A cap that silently truncates reads as "that was
/// all of them"; the count of what was left out is the honest version.
#[test]
fn a_long_ledger_is_capped_and_says_how_many_were_left_out() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    for index in 0..12u32 {
        let id = format!("01J8K2A7B3C4D5E6F7G8H9J{:03}", 400 + index);
        write_decision(
            root,
            &id,
            &format!("decision-{}", index),
            &format!("Decision number {}", index),
            "\"db\"",
            "",
        );
    }
    init(root);

    let out = context_for(root, "src/db/client.ts");
    assert!(
        out.contains("Decision number 11"),
        "decisions must actually be injected, got:\n{}",
        out
    );
    assert!(
        out.contains("more decision"),
        "what was left out must be counted, not swallowed, got:\n{}",
        out
    );
}