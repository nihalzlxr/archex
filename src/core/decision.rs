//! `.archex/decisions/<ULID>-<slug>.md` — the ledger record.
//!
//! One decision, one file, plain text in git. Same discipline as
//! `.archex/rules/*.toml`: the committed file is the record, the local index
//! is a derived cache, and anything that cannot be understood is a loud
//! error rather than a silent skip.
//!
//! Frontmatter is TOML, not YAML, even though the body is markdown. One
//! parser, one `deny_unknown_fields` discipline, no second dependency —
//! the YAML-looking example in PLAN.md §5 parses here with `=` and `[ ]`.

use serde::Deserialize;

/// The decision format this build understands.
pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionFrontmatter {
    pub schema_version: i64,
    pub id: String,
    pub title: String,
    pub status: DecisionStatus,
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub scope: Vec<String>,
    pub author: Author,
    pub recorded_at: String,
}

/// `superseded` is deliberately absent: it is DERIVED from the chain, never
/// written. A record that claims to be superseded is asking to be trusted
/// about a fact sync computes, so validation refuses it instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    Human,
    Agent,
}

#[derive(Debug)]
pub struct DecisionError(pub String);

impl std::fmt::Display for DecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for DecisionError {}

#[derive(Debug, Clone)]
pub struct Decision {
    pub meta: DecisionFrontmatter,
    pub context: String,
    pub decision: String,
    pub alternatives: String,
    /// Path as written, for error messages. Repo-relative is set by the
    /// loader; `None` when a decision was parsed from a string.
    pub path: Option<String>,
}

impl Decision {
    pub fn parse_str(body: &str) -> Result<Self, DecisionError> {
        todo!("3.1 red: decisions have no parser yet")
    }

    /// Parse a file whose name is part of its identity: the name must carry
    /// the same ULID the frontmatter claims.
    pub fn parse_named(file_name: &str, body: &str) -> Result<Self, DecisionError> {
        todo!("3.1 red: decisions have no named parser yet")
    }
}

/// Every `decisions/*.md` in `.archex/decisions/`, in ULID order (which is
/// time order), each parsed and validated.
pub fn load_decisions(root: &std::path::Path) -> Result<Vec<Decision>, DecisionError> {
    todo!("3.1 red: there is no loader for decision files yet")
}

/// The decisions an agent should be told: the head of every chain.
pub fn chain_heads(decisions: &[Decision]) -> Result<Vec<&Decision>, DecisionError> {
    todo!("3.1 red: chains are not resolved yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ULID: &str = "01J8K2A7B3C4D5E6F7G8H9J0K";

    fn good() -> String {
        format!(
            r#"---
schema_version = 1
id = "{ULID}"
title = "Use Drizzle as the ORM"
status = "accepted"
supersedes = []
task = "add-billing-webhooks"
scope = ["db", "services"]
author = "human"
recorded_at = "2026-10-08T14:22:00Z"
---
## Context

Billing needed writes and migrations; hand-rolled SQL had already caused
one incident.

## Decision

Drizzle with the existing Postgres. No second ORM.

## Alternatives rejected

Prisma — slower cold starts; a second query builder in the tree.
"#
        )
    }

    fn parse(body: &str) -> Result<Decision, DecisionError> {
        Decision::parse_str(body)
    }

    #[test]
    fn parses_a_complete_decision() {
        let d = parse(&good()).expect("valid decision");
        assert_eq!(d.meta.id, ULID);
        assert_eq!(d.meta.title, "Use Drizzle as the ORM");
        assert_eq!(d.meta.status, DecisionStatus::Accepted);
        assert_eq!(d.meta.author, Author::Human);
        assert_eq!(d.meta.scope, vec!["db", "services"]);
        assert_eq!(d.meta.task.as_deref(), Some("add-billing-webhooks"));
        assert!(d.decision.contains("Drizzle"));
        assert!(d.context.contains("Billing needed"));
        assert!(d.alternatives.contains("Prisma"));
    }

    /// The ledger exists because of this: an agent session must be able to
    /// read a decision and its rejected alternatives without a human.
    #[test]
    fn alternatives_survive_as_readable_text() {
        let d = parse(&good()).expect("valid");
        assert!(
            d.alternatives.contains("Prisma") && d.alternatives.contains("slower"),
            "the why-not must survive parsing: {:?}",
            d.alternatives
        );
    }

    #[test]
    fn rejects_an_unknown_schema_version() {
        let err = parse(&good().replace("schema_version = 1", "schema_version = 2"))
            .expect_err("must reject");
        assert!(err.to_string().contains("schema_version"), "{}", err);
    }

    #[test]
    fn rejects_an_id_that_is_not_a_ulid() {
        let err = parse(&good().replace(ULID, "use-drizzle-please"))
            .expect_err("must reject");
        assert!(err.to_string().contains("ULID"), "{}", err);
    }

    #[test]
    fn rejects_an_unknown_status() {
        let err = parse(&good().replace("status = \"accepted\"", "status = \"merged\""))
            .expect_err("must reject");
        assert!(err.to_string().contains("status"), "{}", err);
    }

    /// `superseded` is derived from the chain. A file that writes it is
    /// asserting a fact sync computes, which is how a ledger starts lying.
    #[test]
    fn rejects_status_superseded_because_it_is_derived() {
        let err = parse(&good().replace("status = \"accepted\"", "status = \"superseded\""))
            .expect_err("must reject");
        let text = err.to_string();
        assert!(text.contains("superseded") && text.contains("derived"), "{}", text);
    }

    #[test]
    fn rejects_a_decision_that_decides_nothing() {
        let body = good().replace(
            "## Decision\n\nDrizzle with the existing Postgres. No second ORM.",
            "## Decision\n",
        );
        let err = parse(&body).expect_err("must reject");
        assert!(err.to_string().contains("Decision"), "{}", err);
    }

    #[test]
    fn rejects_an_unknown_frontmatter_key() {
        let body = good().replace(
            "recorded_at = \"2026-10-08T14:22:00Z\"",
            "recorded_at = \"2026-10-08T14:22:00Z\"\nowner = \"nihal\"",
        );
        let err = parse(&body).expect_err("must reject");
        assert!(err.to_string().contains("owner"), "{}", err);
    }

    #[test]
    fn rejects_a_recorded_at_that_is_not_rfc3339() {
        let body = good().replace("2026-10-08T14:22:00Z", "yesterday");
        let err = parse(&body).expect_err("must reject");
        assert!(err.to_string().contains("recorded_at"), "{}", err);
    }

    /// Scope selects which files get this decision injected. A path where a
    /// module name belongs never matches anything, and a decision nobody is
    /// ever told about is the quietest failure in the product.
    #[test]
    fn rejects_a_scope_entry_that_is_a_path() {
        let body = good().replace("scope = [\"db\", \"services\"]", "scope = [\"src/db\"]");
        let err = parse(&body).expect_err("must reject");
        assert!(err.to_string().contains("scope"), "{}", err);
    }

    #[test]
    fn rejects_a_supersedes_entry_that_is_not_a_ulid() {
        let body = good().replace("supersedes = []", "supersedes = [\"old-one\"]");
        let err = parse(&body).expect_err("must reject");
        assert!(err.to_string().contains("supersedes"), "{}", err);
    }

    #[test]
    fn rejects_a_decision_that_supersedes_itself() {
        let body = good().replace("supersedes = []", &format!("supersedes = [\"{}\"]", ULID));
        let err = parse(&body).expect_err("must reject");
        assert!(err.to_string().contains("supersedes"), "{}", err);
    }

    /// The file name is the identity: `<ULID>-<slug>.md`. Two files claiming
    /// one id would silently shadow each other in the index.
    #[test]
    fn rejects_a_file_name_that_does_not_carry_the_id() {
        let err = Decision::parse_named("use-drizzle.md", &good()).expect_err("must reject");
        assert!(err.to_string().contains("use-drizzle.md"), "{}", err);
    }

    #[test]
    fn accepts_the_canonical_file_name() {
        let d = Decision::parse_named(
            &format!("{}-use-drizzle-as-the-orm.md", ULID),
            &good(),
        )
        .expect("valid");
        assert_eq!(d.path.as_deref(), Some(&*format!("{}-use-drizzle-as-the-orm.md", ULID)));
    }

    #[test]
    fn chain_heads_serve_only_the_newest_link() {
        let old = parse(&good()).expect("valid old");
        let new_body = good()
            .replace(ULID, "01J8K2A7B3C4D5E6F7G8H9J0M")
            .replace("status = \"accepted\"", "status = \"accepted\"")
            .replace("supersedes = []", &format!("supersedes = [\"{}\"]", ULID))
            .replace("title = \"Use Drizzle as the ORM\"", "title = \"Use Kysely instead\"");
        let new = parse(&new_body).expect("valid new");

        let chain = [old, new];
        let heads = chain_heads(&chain).expect("resolvable");
        assert_eq!(heads.len(), 1, "only the head is served");
        assert_eq!(heads[0].meta.id, "01J8K2A7B3C4D5E6F7G8H9J0M");
    }

    /// A cycle is a data-entry accident, and a naive resolver loops forever
    /// or silently drops both links. It must name the chain and stop.
    #[test]
    fn a_supersede_cycle_is_an_error_not_a_silent_drop() {
        let a = "01J8K2A7B3C4D5E6F7G8H9J0K";
        let b = "01J8K2A7B3C4D5E6F7G8H9J0M";
        // a supersedes b, b supersedes a.
        let first = parse(&good().replace("supersedes = []", &format!("supersedes = [\"{}\"]", b)))
            .expect("valid");
        let second = parse(
            &good()
                .replace(ULID, b)
                .replace("supersedes = []", &format!("supersedes = [\"{}\"]", a)),
        )
        .expect("valid");

        let err = chain_heads(&[first, second]).expect_err("cycle must be refused");
        assert!(err.to_string().contains("supersedes"), "{}", err);
    }
}