use crate::core::ledger::{self, SyncReport};
use std::path::Path;

/// Import committed ledger records into the derived index.
///
/// Thin on purpose: `core::ledger` does the work and the counts, because
/// the MCP server runs the same thing at startup and must not print to
/// stdout — that stream is JSON-RPC.
pub fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let report = ledger::sync(root)?;
    print_report(&report);
    Ok(())
}

fn print_report(report: &SyncReport) {
    println!(
        "[OK] {} imported, {} unchanged, {} removed — {}",
        count(report.imported, "decision"),
        count(report.unchanged, "decision"),
        count(report.removed, "decision"),
        count(report.heads, "chain head"),
    );
    if report.superseded > 0 {
        println!(
            "  {} superseded — only chain heads are ever injected.",
            count(report.superseded, "decision")
        );
    }
    if !report.orphan_scopes.is_empty() {
        println!(
            "  warning: {} decision scope entr(y/ies) match no module or layer in config.toml:",
            report.orphan_scopes.len()
        );
        for orphan in &report.orphan_scopes {
            println!("    {}", orphan);
        }
        println!("    These decisions will not be injected into any file.");
    }
    println!("  Note: source files are indexed by `archex init`. sync imports the ledger only.");
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}