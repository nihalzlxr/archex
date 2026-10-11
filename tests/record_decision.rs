use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

use common::{call_mcp_tool, init, two_layer_fixture};

fn record(root: &Path, arguments: &str) -> String {
    call_mcp_tool(root, "record_decision", arguments)
}

const ARGUMENTS: &str = r#"{
    "title": "Billing writes go through the db module only",
    "decision": "Route handlers call services; only services import db.",
    "context": "A feature route imported db directly and broke the rule on day two.",
    "alternatives": ["let each route own its SQL", "allow db imports in route files with a lint"],
    "scope": ["db"],
    "task": "harden-routes"
}"#;

/// The exit test of Step 3: an agent logs a decision mid-session, and the
/// NEXT session starts knowing it. Not the same process, not a manual
/// sync — a fresh server, exactly what a teammate's merged PR looks like
/// to this machine.
#[test]
fn a_decision_recorded_mid_session_is_known_to_the_next_one() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    let recorded = record(root, ARGUMENTS);
    assert!(
        recorded.contains("Billing writes go through the db module only"),
        "the tool must confirm what it recorded, got:\n{}",
        recorded
    );

    // A brand new server process, as if a teammate merged the PR and this
    // is the next session.
    let next = call_mcp_tool(root, "get_context", r#"{"file_path":"src/db/client.ts"}"#);
    assert!(
        next.contains("Billing writes go through the db module only"),
        "the next session must start knowing it, got:\n{}",
        next
    );
    assert!(
        next.contains("let each route own its SQL"),
        "the rejected alternative is the part that stops the agent \
         re-proposing it, got:\n{}",
        next
    );
}

/// Agents propose. A record an agent wrote and marked accepted would be an
/// agent deciding, which is the one thing the authority ordering forbids.
#[test]
fn an_agent_cannot_record_an_accepted_decision() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // Control: the tool works, so the refusal below is about the flags.
    record(root, ARGUMENTS);

    let out = record(
        root,
        r#"{"title": "Sneak one in", "decision": "x", "status": "accepted", "author": "human"}"#,
    );
    let lower = out.to_lowercase();
    assert!(
        lower.contains("error") || lower.contains("unknown field") || lower.contains("invalid"),
        "the tool must reject authority flags it does not accept, got:\n{}",
        out
    );
    let dir_path = root.join(".archex/decisions");
    let written = if dir_path.exists() {
        fs::read_dir(&dir_path).unwrap().count()
    } else {
        0
    };
    assert_eq!(
        written, 1,
        "only the control record may exist, got:\n{}",
        out
    );
}

#[test]
fn record_decision_refuses_a_scope_that_cannot_match_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    // Control: a valid record lands, so the refusal below is about the
    // scope and not about a broken tool.
    record(root, ARGUMENTS);

    let out = record(
        root,
        r#"{"title": "Bad scope", "decision": "x", "scope": ["src/db"]}"#,
    );
    assert!(
        out.contains("scope") || out.contains("error"),
        "a path is not a scope, got:\n{}",
        out
    );
    let dir_path = root.join(".archex/decisions");
    let written = if dir_path.exists() {
        fs::read_dir(&dir_path).unwrap().count()
    } else {
        0
    };
    assert_eq!(
        written, 1,
        "a refused decision must leave no file beyond the control"
    );
}

/// The committed ledger is append-only from the tool's side: a second
/// record is a second file, and the first one is untouched.
#[test]
fn recording_twice_appends_and_never_rewrites() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    record(root, ARGUMENTS);
    let dir_path = root.join(".archex/decisions");
    let first: Vec<_> = fs::read_dir(&dir_path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(first.len(), 1);
    let before = fs::read_to_string(&first[0]).unwrap();

    let second_out = record(
        root,
        r#"{"title": "Cache user lookups", "decision": "Memoize for 30 seconds.", "scope": ["db"]}"#,
    );
    assert!(
        second_out.contains("Cache user lookups"),
        "the second record must succeed too, got:\n{}",
        second_out
    );

    let all: Vec<_> = fs::read_dir(&dir_path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(all.len(), 2, "two decisions, two files");
    assert!(
        all.contains(&first[0]),
        "the first record must still be there"
    );
    assert_eq!(
        fs::read_to_string(&first[0]).unwrap(),
        before,
        "recording must never rewrite an existing record"
    );
}

/// A recorded decision is a proposal until a human accepts it, and the
/// agent reading it back must be able to tell.
#[test]
fn a_recorded_decision_is_marked_proposed_until_a_human_accepts_it() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    two_layer_fixture(root);
    init(root);

    record(root, ARGUMENTS);
    let dir_path = root.join(".archex/decisions");
    let body = fs::read_to_string(
        fs::read_dir(&dir_path)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();

    assert!(
        body.contains("status = \"proposed\""),
        "an agent's record starts as a proposal, got:\n{}",
        body
    );
    assert!(
        body.contains("author = \"agent\""),
        "and it says who wrote it, got:\n{}",
        body
    );
}