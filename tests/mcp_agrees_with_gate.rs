use std::io::Write;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

mod common;

use common::{init, two_layer_fixture, ui_no_db_rule};

/// Drives `archex serve` over stdio with hand-written JSON-RPC and returns
/// everything the server wrote for the tool call.
///
/// Two things about rmcp 0.1's stdio transport, both learned the hard way:
/// `clientInfo` requires `version` (a payload without it fails to deserialize,
/// the error is swallowed, and the server reports a misleading "expect
/// initialize request"), and the server tears down on stdin EOF — so stdin has
/// to stay open until the tool response actually arrives.
fn call_mcp_tool(root: &Path, tool: &str, arguments: &str) -> String {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    let binary = assert_cmd::cargo::cargo_bin("archex");
    let mut child = StdCommand::new(binary)
        .arg("serve")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn archex serve");

    let requests = format!(
        concat!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"test","version":"0"}}}}}}"#,
            "\n",
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized","params":{{}}}}"#,
            "\n",
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#,
            "\n"
        ),
        tool = tool,
        arguments = arguments
    );

    {
        let stdin = child.stdin.as_mut().expect("server stdin");
        stdin.write_all(requests.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    // NOTE: stdin is deliberately NOT dropped here. EOF makes the server exit
    // before it dispatches the tool call.

    let stdout = child.stdout.take().expect("server stdout");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut collected = String::new();
    let deadline = Duration::from_secs(20);
    loop {
        match rx.recv_timeout(deadline) {
            Ok(line) => {
                let is_tool_response = line.contains("\"id\":2");
                collected.push_str(&line);
                collected.push('\n');
                if is_tool_response {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    collected
}

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