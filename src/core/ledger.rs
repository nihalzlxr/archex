//! The ledger index: committed records in, a local cache out.
//!
//! Nothing here writes to a committed file. The markdown in
//! `.archex/` is the record; this is a cache of it, and every number the
//! caller sees is counted rather than estimated, because a cache that
//! silently skips a file is the exact failure mode this tool exists to
//! remove.

use crate::core::config;
use crate::core::decision::{self, Decision};
use crate::core::db::{Db, DecisionRecord, DB_PATH};
use crate::core::hash;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Default)]
pub struct SyncReport {
    pub imported: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub heads: usize,
    pub superseded: usize,
    /// `scope` entries that match no module and no layer in config.toml.
    /// A decision nobody can be scoped to is a decision nobody is told.
    pub orphan_scopes: Vec<String>,
}

fn record_path(name: &str) -> String {
    format!(".archex/decisions/{}", name)
}

pub fn sync(root: &Path) -> Result<SyncReport, String> {
    let db = Db::open(&root.join(DB_PATH)).map_err(|e| e.to_string())?;
    db.init_schema().map_err(|e| e.to_string())?;

    let decisions = decision::load_decisions(root).map_err(|e| e.to_string())?;
    let heads = decision::chain_heads(&decisions).map_err(|e| e.to_string())?;

    let indexed: HashMap<String, String> = db
        .decision_index()
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();

    let mut report = SyncReport::default();
    for record in &decisions {
        let name = match &record.path {
            Some(name) => name.clone(),
            None => continue,
        };
        let path = record_path(&name);
        let content = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
        let content_hash = hash::content_hash(content.as_bytes());

        if indexed.get(&path) == Some(&content_hash) {
            report.unchanged += 1;
            continue;
        }

        db.upsert_decision(&DecisionRecord {
            id: record.meta.id.clone(),
            slug: name
                .split_once('-')
                .map(|(_, slug)| slug.trim_end_matches(".md").to_string())
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
        })
        .map_err(|e| e.to_string())?;
        report.imported += 1;
    }

    // A record deleted from git must leave the index, or a withdrawn
    // decision is injected forever.
    let present: HashSet<String> = decisions
        .iter()
        .filter_map(|record| record.path.as_deref())
        .map(record_path)
        .collect();
    for (path, _) in db.decision_index().map_err(|e| e.to_string())? {
        if !present.contains(&path) {
            db.delete_decision_at_path(&path).map_err(|e| e.to_string())?;
            report.removed += 1;
        }
    }

    let head_ids: Vec<String> = heads.iter().map(|head| head.meta.id.clone()).collect();
    db.set_decision_heads(&head_ids)
        .map_err(|e| e.to_string())?;
    report.heads = head_ids.len();
    report.superseded = decisions.len().saturating_sub(head_ids.len());
    report.orphan_scopes = orphan_scopes(root, &decisions);

    Ok(report)
}

/// Scope entries that name nothing the config knows. Not fatal — a
/// decision may be recorded about a module that does not exist yet — but
/// never silent, because the alternative is a ledger that looks complete
/// and injects nothing.
fn orphan_scopes(root: &Path, decisions: &[Decision]) -> Vec<String> {
    let config = match config::Config::load(root) {
        Ok(config) => config,
        Err(_) => return Vec::new(),
    };
    let modules: Vec<&str> = config.modules.iter().map(|m| m.name.as_str()).collect();
    let layers: Vec<&str> = config.layers.iter().map(|l| l.as_str()).collect();

    let mut orphans = Vec::new();
    for record in decisions {
        for entry in &record.meta.scope {
            if !modules.contains(&entry.as_str()) && !layers.contains(&entry.as_str()) {
                orphans.push(format!(
                    "{}: scope `{}` matches no module or layer",
                    decision::short_id(&record.meta.id),
                    entry
                ));
            }
        }
    }
    orphans
}

/// The brief an agent actually reads. Markdown, so the shape matches the
/// committed file, and bounded, because context is the scarcest resource
/// in a session.
pub fn summary(record: &DecisionRecord, limit: usize) -> String {
    let mut out = format!("## Decision\n\n{}\n", bounded(&record.decision, limit));
    if !record.alternatives.is_empty() {
        out.push_str(&format!(
            "\n## Alternatives rejected\n\n{}\n",
            bounded(&record.alternatives, limit)
        ));
    }
    out.trim_end().to_string()
}

/// Tidy a body without flattening it. Whitespace runs collapse, single line
/// breaks survive: `- each rejected option` must not come back as one line
/// of `- first - second`, which reads as a garbled sentence rather than a
/// list.
fn bounded(text: &str, limit: usize) -> String {
    let mut out = String::with_capacity(text.len().min(limit + 1));
    let mut pending_space = false;
    for character in text.chars() {
        if character == '\n' || character == '\r' {
            while out.ends_with(' ') {
                out.pop();
            }
            // Paragraphs survive; runs of blank lines do not.
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
            pending_space = false;
            continue;
        }
        if character.is_whitespace() {
            pending_space = !out.ends_with('\n');
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(character);
        if out.chars().count() > limit {
            break;
        }
    }
    let mut trimmed: String = out.chars().take(limit).collect();
    while trimmed.ends_with(' ') || trimmed.ends_with('\n') {
        trimmed.pop();
    }
    if trimmed.chars().count() < out.trim_end().chars().count() {
        trimmed.push('…');
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(decision: &str, alternatives: &str) -> DecisionRecord {
        DecisionRecord {
            id: "01J8K2A7B3C4D5E6F7G8H9J0KM".to_string(),
            slug: "use-drizzle".to_string(),
            title: "Use Drizzle as the ORM".to_string(),
            status: "accepted".to_string(),
            author: "human".to_string(),
            scope: vec!["db".to_string()],
            supersedes: vec![],
            task: None,
            recorded_at: "2026-10-08T14:22:00Z".to_string(),
            context: "Billing needed writes.".to_string(),
            decision: decision.to_string(),
            alternatives: alternatives.to_string(),
            path: ".archex/decisions/01J8K2A7B3C4D5E6F7G8H9J0KM-use-drizzle.md".to_string(),
            content_hash: "deadbeefdeadbeef".to_string(),
            is_head: true,
        }
    }

    #[test]
    fn the_summary_keeps_the_rejected_alternatives() {
        let summary = summary(&record("Drizzle.", "Prisma — slower cold starts."), 400);
        assert!(summary.contains("## Decision"));
        assert!(
            summary.contains("Prisma — slower cold starts."),
            "got:\n{}",
            summary
        );
    }

    /// A truncated brief must say so. An agent cannot tell a summary that
    /// ends because it was cut from one that ends there.
    #[test]
    fn a_long_brief_is_cut_with_a_marker() {
        let long = "word ".repeat(500);
        let summary = summary(&record(&long, ""), 100);
        assert!(summary.ends_with('…'), "got:\n{}", summary);
        assert!(summary.chars().count() < 200);
    }

    #[test]
    fn a_short_brief_is_left_alone() {
        assert_eq!(
            bounded("Drizzle. No second ORM.", 100),
            "Drizzle. No second ORM."
        );
    }

    /// Rejected options arrive as a markdown list. Flattened onto one line
    /// they read as one garbled option, which is the worst possible thing
    /// to hand an agent deciding what not to do.
    #[test]
    fn a_list_stays_a_list() {
        let list = "- each route owns its SQL\n- lint the exceptions away";
        assert_eq!(bounded(list, 100), list);
        assert!(
            bounded(list, 100).contains("\n- lint"),
            "got:\n{}",
            bounded(list, 100)
        );
    }

    #[test]
    fn ragged_whitespace_collapses_without_touching_the_lines() {
        assert_eq!(bounded("line one\n\n\nline two", 100), "line one\n\nline two");
        assert_eq!(bounded("lots   of\tspace", 100), "lots of space");
    }
}