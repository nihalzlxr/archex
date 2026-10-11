use crate::core::config;
use crate::core::decision::{self, Decision};
use crate::core::db::{Db, DecisionRecord, DB_PATH};
use crate::core::hash;
use std::collections::HashMap;
use std::path::Path;

/// Import committed ledger records into the derived index.
///
/// Two rules make it safe to run on every session start:
/// **it never writes to a committed file**, and **it only re-reads what
/// changed** — a decision whose content hash is unchanged is not even
/// opened. Everything it reports is counted, because an import that
/// silently skipped a file is the kind of gap this tool exists to remove.
pub fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = root.join(DB_PATH);
    let db = Db::open(&db_path)?;
    db.init_schema()?;

    let decisions = decision::load_decisions(root)?;
    let heads = decision::chain_heads(&decisions)?;

    let indexed: HashMap<String, String> = db.decision_index()?.into_iter().collect();

    let mut imported = 0usize;
    let mut unchanged = 0usize;
    for record in &decisions {
        let path = record
            .path
            .as_deref()
            .map(|name| format!(".archex/decisions/{}", name))
            .unwrap_or_default();
        let content = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
        let content_hash = hash::content_hash(content.as_bytes());

        if indexed.get(&path) == Some(&content_hash) {
            unchanged += 1;
            continue;
        }

        db.upsert_decision(&DecisionRecord {
            id: record.meta.id.clone(),
            slug: record
                .path
                .as_deref()
                .and_then(|name| name.split_once('-').map(|(_, slug)| slug.to_string()))
                .unwrap_or_default(),
            title: record.meta.title.clone(),
            status: record.meta.status.as_str().to_string(),
            author: record.meta.author.as_str().to_string(),
            scope: record.meta.scope.clone(),
            supersedes: record.meta.supersedes.clone(),
            task: record.meta.task.clone(),
            recorded_at: record.meta.recorded_at.clone(),
            context: record.context.clone(),
            decision: record.decision.clone(),
            alternatives: record.alternatives.clone(),
            path: path.clone(),
            content_hash,
            is_head: false,
        })?;
        imported += 1;
    }

    // A record that was deleted from git must leave the index too, or a
    // withdrawn decision keeps being injected forever.
    let present: std::collections::HashSet<String> = decisions
        .iter()
        .filter_map(|d| d.path.as_deref())
        .map(|name| format!(".archex/decisions/{}", name))
        .collect();
    let mut removed = 0usize;
    for (path, _) in db.decision_index()? {
        if !present.contains(&path) {
            db.delete_decision_at_path(&path)?;
            removed += 1;
        }
    }

    let head_ids: Vec<String> = heads.iter().map(|head| head.meta.id.clone()).collect();
    db.set_decision_heads(&head_ids)?;

    println!(
        "[OK] {} imported, {} unchanged, {} removed — {}",
        count(imported, "decision"),
        count(unchanged, "decision"),
        count(removed, "decision"),
        count(head_ids.len(), "chain head"),
    );
    let superseded = decisions.len().saturating_sub(head_ids.len());
    if superseded > 0 {
        println!(
            "  {} superseded — only chain heads are ever injected.",
            count(superseded, "decision")
        );
    }
    report_scope_coverage(root, &decisions)?;
    println!(
        "  Note: source files are indexed by `archex init`. sync imports the ledger only."
    );
    Ok(())
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

/// A scope that matches no module and no layer is a decision that will
/// never be injected into anything. Silently keeping it is how a ledger
/// fills with records nobody reads.
fn report_scope_coverage(root: &Path, decisions: &[Decision]) -> Result<(), Box<dyn std::error::Error>> {
    let config = match config::Config::load(root) {
        Ok(config) => config,
        // No committed config: nothing is known, so nothing can be called
        // a mismatch. `archex init` drafts one.
        Err(_) => return Ok(()),
    };

    let modules: Vec<&str> = config.modules.iter().map(|m| m.name.as_str()).collect();
    let layers: Vec<&str> = config.layers.iter().map(|l| l.as_str()).collect();

    let mut orphans: Vec<String> = Vec::new();
    for record in decisions {
        for entry in &record.meta.scope {
            if !modules.contains(&entry.as_str()) && !layers.contains(&entry.as_str()) {
                orphans.push(format!("{}: scope `{}`", &record.meta.id[..10.min(record.meta.id.len())], entry));
            }
        }
    }

    if orphans.is_empty() {
        return Ok(());
    }
    println!(
        "  warning: {} decision scope entr(y/ies) match no module or layer in config.toml:",
        orphans.len()
    );
    for orphan in orphans {
        println!("    {}", orphan);
    }
    println!("    These decisions will not be injected into any file.");
    Ok(())
}