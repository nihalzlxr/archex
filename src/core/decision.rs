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

/// `superseded` is deliberately not a status a file may carry: it is DERIVED
/// from the chain, never written. The variant exists only so validation can
/// answer with the reason instead of serde's "unknown variant".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    Human,
    Agent,
}

impl DecisionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionStatus::Proposed => "proposed",
            DecisionStatus::Accepted => "accepted",
            DecisionStatus::Superseded => "superseded",
        }
    }
}

impl Author {
    pub fn as_str(&self) -> &'static str {
        match self {
            Author::Human => "human",
            Author::Agent => "agent",
        }
    }
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
    /// Parse a file whose name is part of its identity: the name must carry
    /// the same ULID the frontmatter claims.
    pub fn parse_named(file_name: &str, body: &str) -> Result<Self, DecisionError> {
        let (front, markdown) = split_frontmatter(body)?;
        let meta: DecisionFrontmatter = toml::from_str(&front)
            .map_err(|e| DecisionError(format!("invalid frontmatter: {}", e)))?;

        validate(&meta, file_name)?;

        let sections = Sections::split(&markdown);
        let decision = sections.require("Decision")?;
        Ok(Decision {
            meta,
            context: sections.get("Context"),
            decision,
            alternatives: sections.get("Alternatives rejected"),
            path: if file_name.is_empty() {
                None
            } else {
                Some(file_name.to_string())
            },
        })
    }
}

/// The `---` fenced header, TOML inside. Everything after it is markdown.
fn split_frontmatter(body: &str) -> Result<(String, String), DecisionError> {
    let trimmed = body.trim_start_matches('\u{feff}');
    let rest = trimmed.strip_prefix("---\n").ok_or_else(|| {
        DecisionError(
            "missing frontmatter: a decision file starts with a `---` line".to_string(),
        )
    })?;

    // The closing fence is the first line that is exactly `---`.
    let mut split_at = None;
    for (index, line) in rest.split_inclusive('\n').enumerate() {
        if line.trim_end() == "---" {
            split_at = Some(index);
            break;
        }
    }
    let index = split_at.ok_or_else(|| {
        DecisionError("missing frontmatter: the opening `---` is never closed".to_string())
    })?;

    let mut lines: Vec<&str> = rest.split_inclusive('\n').collect();
    let front: String = lines[..index].concat();
    let markdown: String = lines[index + 1..].concat();
    lines.clear();
    Ok((front, markdown))
}

/// `## Heading` blocks, in document order.
struct Sections {
    found: Vec<(String, String)>,
}

impl Sections {
    fn split(markdown: &str) -> Self {
        let mut found: Vec<(String, String)> = Vec::new();
        let mut current: Option<(String, Vec<String>)> = None;

        for line in markdown.lines() {
            let heading = line.strip_prefix("## ").map(|rest| rest.trim().to_string());
            match heading {
                Some(title) => {
                    if let Some((name, body)) = current.take() {
                        found.push((name, body.join("\n")));
                    }
                    current = Some((title, Vec::new()));
                }
                None => {
                    if let Some((_, body)) = current.as_mut() {
                        body.push(line.to_string());
                    }
                }
            }
        }
        if let Some((name, body)) = current.take() {
            found.push((name, body.join("\n")));
        }
        Sections { found }
    }

    fn get(&self, heading: &str) -> String {
        self.found
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(heading))
            .map(|(_, body)| body.trim().to_string())
            .unwrap_or_default()
    }

    /// A decision record with no decision in it is a note, and a note that
    /// claims to be a decision is worse than no file.
    fn require(&self, heading: &str) -> Result<String, DecisionError> {
        let body = self.get(heading);
        if body.is_empty() {
            return Err(DecisionError(format!(
                "`## {}` is missing or empty. A decision that records no decision \
                 tells the next agent session nothing.",
                heading
            )));
        }
        Ok(body)
    }
}

fn validate(meta: &DecisionFrontmatter, file_name: &str) -> Result<(), DecisionError> {
    if meta.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(DecisionError(format!(
            "schema_version {} is not supported (this build understands {})",
            meta.schema_version, SUPPORTED_SCHEMA_VERSION
        )));
    }

    if !is_ulid(&meta.id) {
        return Err(DecisionError(format!(
            "`{}` is not a ULID. A decision id is 26 Crockford base32 characters \
             so records sort by time and never collide.",
            meta.id
        )));
    }

    if meta.status == DecisionStatus::Superseded {
        return Err(DecisionError(
            "status = \"superseded\" is not written by hand: it is derived from the \
             supersedes chain, so a record that claims it is asserting a fact sync \
             computes. Write the new decision with `supersedes = [<old id>]` instead."
                .to_string(),
        ));
    }

    if meta.title.trim().is_empty() {
        return Err(DecisionError("title is empty".to_string()));
    }

    if !is_rfc3339_utc(&meta.recorded_at) {
        return Err(DecisionError(format!(
            "recorded_at `{}` is not an RFC 3339 UTC timestamp \
             (2026-10-08T14:22:00Z). Two machines must agree on when a decision \
             was recorded without asking either of them.",
            meta.recorded_at
        )));
    }

    for entry in &meta.scope {
        if entry.trim().is_empty() || entry.contains('/') || entry.contains('\\') {
            return Err(DecisionError(format!(
                "scope entry `{}` is not a module or layer name. Scope selects \
                 which files get this decision injected; a path never matches, \
                 and a decision nobody is told about is invisible by construction.",
                entry
            )));
        }
    }

    for entry in &meta.supersedes {
        if !is_ulid(entry) {
            return Err(DecisionError(format!(
                "supersedes `{}` is not a ULID", entry
            )));
        }
        if entry == &meta.id {
            return Err(DecisionError(format!(
                "decision {} supersedes itself: a record that replaces itself \
                 has no head, so it can never be served",
                meta.id
            )));
        }
    }

    if !file_name.is_empty() {
        validate_file_name(file_name, &meta.id)?;
    }

    Ok(())
}

/// `<ULID>-<slug>.md`. The name is the identity: two files claiming one id
/// would shadow each other in the index, and a name without the ULID means
/// the frontmatter and the filesystem disagree about which record this is.
fn validate_file_name(file_name: &str, id: &str) -> Result<(), DecisionError> {
    let stem = file_name
        .strip_suffix(".md")
        .ok_or_else(|| DecisionError(format!("{}: decision files are .md", file_name)))?;
    let (file_id, slug) = stem.split_once('-').ok_or_else(|| {
        DecisionError(format!(
            "{}: a decision file is named <ULID>-<slug>.md",
            file_name
        ))
    })?;
    if file_id != id {
        return Err(DecisionError(format!(
            "{}: the frontmatter id is {}, so this file and that id name different \
             records. Rename the file to {}-<slug>.md",
            file_name, id, id
        )));
    }
    if slug.is_empty()
        || slug.starts_with('-')
        || slug.ends_with('-')
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(DecisionError(format!(
            "{}: `{}` is not a slug. Use lowercase kebab-case — it lands in \
             `git status` and in agent output.",
            file_name, slug
        )));
    }
    Ok(())
}

/// Crockford base32, 26 characters. The first character is capped at `7`
/// because a ULID holds a 48-bit millisecond timestamp, not a full 130 bits.
pub fn is_ulid(value: &str) -> bool {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bytes = value.as_bytes();
    if bytes.len() != 26 {
        return false;
    }
    if bytes[0] > b'7' {
        return false;
    }
    bytes.iter().all(|b| ALPHABET.contains(b))
}

/// The part of a ULID that is safe to show in a list: ULIDs sort by time,
/// so ten characters is both readable and chronologically useful.
pub fn short_id(id: &str) -> &str {
    &id[..id.len().min(10)]
}

/// A fresh id for a decision written right now: 48-bit millisecond
/// timestamp, 80 bits of randomness, Crockford base32. Lexical order is
/// chronological order, so `ls` and `git log` tell you which decision is
/// newer without reading a timestamp.
pub fn new_ulid() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    encode_ulid(millis, random_bytes())
}

fn encode_ulid(timestamp_ms: u64, randomness: [u8; 10]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut entropy: u128 = 0;
    for byte in randomness {
        entropy = (entropy << 8) | byte as u128;
    }
    // 48-bit timestamp in the high bits, 80 bits of entropy below it.
    let mut value: u128 = (((timestamp_ms as u128) & 0xFFFF_FFFF_FFFF) << 80) | entropy;
    let mut chars = [b'0'; 26];
    for slot in chars.iter_mut().rev() {
        *slot = ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("Crockford base32 is ASCII")
}

/// The current instant, in the one shape `recorded_at` accepts.
pub fn now_rfc3339() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    format_rfc3339(seconds)
}

fn format_rfc3339(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
/// Written out because chrono is not a dependency of a tool that only ever
/// writes one timestamp field.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 80 bits of randomness for a ULID. `/dev/urandom` where it exists; where
/// it does not, the clock, the pid and a counter — weaker, and documented
/// as weaker, because a decision id only has to not collide on one machine.
fn random_bytes() -> [u8; 10] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let mut bytes = [0u8; 10];
        if std::io::Read::read_exact(&mut file, &mut bytes).is_ok() {
            return bytes;
        }
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seed = [
        nanos.to_le_bytes().to_vec(),
        std::process::id().to_le_bytes().to_vec(),
        COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes().to_vec(),
    ]
    .concat();

    let mut bytes = [0u8; 10];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = crate::core::hash::content_hash(&seed[index..]).as_bytes()[0];
    }
    bytes
}

/// `YYYY-MM-DDTHH:MM:SSZ`, and the fields must be real ones. Deliberately
/// strict: offsets and fractional seconds are legal RFC 3339 but the ledger
/// writes one shape, so that `ls` and `git log` agree on ordering.
fn is_rfc3339_utc(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 {
        return false;
    }
    let digits_at = |positions: &[usize]| positions.iter().all(|i| bytes[*i].is_ascii_digit());
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':'
        || bytes[16] != b':' || bytes[19] != b'Z'
    {
        return false;
    }
    if !digits_at(&[0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]) {
        return false;
    }
    let number = |range: std::ops::Range<usize>| -> u32 {
        value[range].parse().unwrap_or(0)
    };
    let (month, day, hour, minute, second) = (
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    );
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour <= 23
        && minute <= 59
        // Leap seconds exist; a ledger that recorded 60 would sort wrong.
        && second <= 59
}

/// Every `decisions/*.md` in `.archex/decisions/`, in ULID order (which is
/// time order), each parsed and validated.
pub fn load_decisions(root: &std::path::Path) -> Result<Vec<Decision>, DecisionError> {
    let dir = root.join(".archex/decisions");
    let mut entries: Vec<_> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        // No decisions directory is a valid repo, not an error.
        Err(_) => return Ok(Vec::new()),
    };
    entries.sort_by_key(|e| e.file_name());

    let mut decisions = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        let body = std::fs::read_to_string(&path)
            .map_err(|e| DecisionError(format!("cannot read {}: {}", path.display(), e)))?;
        decisions.push(
            Decision::parse_named(&file_name, &body)
                .map_err(|e| DecisionError(format!("{}: {}", path.display(), e)))?,
        );
    }
    Ok(decisions)
}

/// The decisions an agent should be told: the head of every chain.
///
/// Walk each decision's `supersedes` chain to the newest record and serve
/// only that. The chain is followed rather than trusted, because a cycle is
/// a data-entry accident and an infinite loop is the worst possible answer
/// to it.
pub fn chain_heads(decisions: &[Decision]) -> Result<Vec<&Decision>, DecisionError> {
    let known: std::collections::HashSet<&str> =
        decisions.iter().map(|d| d.meta.id.as_str()).collect();

    // Every chain is walked, not only the ones that start at a head: in a
    // cycle nothing is a head, so a head-first walk would report a clean
    // empty result for the one case that must never be silent.
    for decision in decisions {
        for entry in &decision.meta.supersedes {
            if !known.contains(entry.as_str()) {
                return Err(DecisionError(format!(
                    "decision {} supersedes {}, which is not in .archex/decisions/. \
                     Either commit that record or drop the entry — otherwise both \
                     records stay live and neither looks replaced.",
                    decision.meta.id, entry
                )));
            }
        }

        let mut seen = std::collections::HashSet::new();
        seen.insert(decision.meta.id.as_str());
        let mut cursor: &Decision = decision;
        while let Some(next) = decisions
            .iter()
            .find(|d| d.meta.supersedes.iter().any(|s| s == &cursor.meta.id))
        {
            if !seen.insert(next.meta.id.as_str()) {
                return Err(DecisionError(format!(
                    "decision {} supersedes {} which supersedes it back: a cycle \
                     has no head, so nothing in this chain can be served",
                    decision.meta.id, next.meta.id
                )));
            }
            cursor = next;
        }
    }

    let superseded: std::collections::HashSet<&str> = decisions
        .iter()
        .flat_map(|d| d.meta.supersedes.iter().map(|s| s.as_str()))
        .collect();

    let mut heads: Vec<&Decision> = decisions
        .iter()
        .filter(|d| !superseded.contains(d.meta.id.as_str()))
        .collect();

    // Newest first: ULIDs are time-ordered, so this is chronological order
    // without reading a timestamp format.
    heads.sort_by(|a, b| b.meta.id.cmp(&a.meta.id));
    Ok(heads)
}

/// A decision about to be written. One shape for both writers — the CLI
/// and the MCP tool — so a record an agent writes goes through exactly
/// the validator a human's does.
#[derive(Debug, Clone)]
pub struct NewDecision {
    pub id: String,
    pub title: String,
    pub decision: String,
    pub context: Option<String>,
    pub alternatives: Vec<String>,
    pub scope: Vec<String>,
    pub task: Option<String>,
    pub status: DecisionStatus,
    pub author: Author,
    pub supersedes: Vec<String>,
    pub recorded_at: String,
}

/// Render, validate, write. The order matters: a record that only fails at
/// sync was written by a broken writer.
pub fn write_new(root: &std::path::Path, new: &NewDecision) -> Result<String, DecisionError> {
    let slug = slugify(&new.title);
    let name = format!("{}-{}.md", new.id, slug);
    let path = root.join(".archex/decisions").join(&name);
    if path.exists() {
        return Err(DecisionError(format!(
            "refusing to overwrite {}",
            path.display()
        )));
    }

    let body = render(new);
    // Parsed with the same name the file will carry, so the identity check
    // runs before anything reaches the disk.
    Decision::parse_named(&name, &body)?;

    std::fs::create_dir_all(root.join(".archex/decisions"))
        .map_err(|e| DecisionError(format!("cannot create .archex/decisions: {}", e)))?;
    std::fs::write(&path, &body)
        .map_err(|e| DecisionError(format!("cannot write {}: {}", path.display(), e)))?;

    Ok(format!(".archex/decisions/{}", name))
}

/// Canonical on purpose: fixed field order, defaults omitted, so the diff
/// of a decision is the decision and two machines writing the same record
/// produce the same bytes.
pub fn render(new: &NewDecision) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!(
        "schema_version = {}\n",
        SUPPORTED_SCHEMA_VERSION
    ));
    out.push_str(&format!("id = {}\n", toml_string(&new.id)));
    out.push_str(&format!("title = {}\n", toml_string(new.title.trim())));
    out.push_str(&format!(
        "status = {}\n",
        toml_string(new.status.as_str())
    ));
    out.push_str(&format!(
        "supersedes = [{}]\n",
        new.supersedes.iter().map(|id| toml_string(id)).collect::<Vec<_>>().join(", ")
    ));
    if let Some(task) = &new.task {
        out.push_str(&format!("task = {}\n", toml_string(task.trim())));
    }
    out.push_str(&format!(
        "scope = [{}]\n",
        new.scope
            .iter()
            .map(|entry| toml_string(entry.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!("author = {}\n", toml_string(new.author.as_str())));
    out.push_str(&format!("recorded_at = {}\n", toml_string(&new.recorded_at)));
    out.push_str("---\n");
    out.push_str("## Context\n\n");
    out.push_str(new.context.as_deref().unwrap_or("").trim());
    out.push_str("\n\n## Decision\n\n");
    out.push_str(new.decision.trim());
    if !new.alternatives.is_empty() {
        out.push_str("\n\n## Alternatives rejected\n\n");
        for alternative in &new.alternatives {
            out.push_str("- ");
            out.push_str(alternative.trim());
            out.push('\n');
        }
    }
    out
}

fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // Control characters would silently mangle a record; a space is
            // the least surprising flattening for a one-line field.
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Lowercase kebab-case, the same shape a rule file name takes: it lands
/// in `git status` and in agent output.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = true;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "decision".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ULID: &str = "01J8K2A7B3C4D5E6F7G8H9J0KM";

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
        Decision::parse_named("", body)
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
            .replace(ULID, "01J8K2A7B3C4D5E6F7G8H9J0MN")
            .replace("status = \"accepted\"", "status = \"accepted\"")
            .replace("supersedes = []", &format!("supersedes = [\"{}\"]", ULID))
            .replace("title = \"Use Drizzle as the ORM\"", "title = \"Use Kysely instead\"");
        let new = parse(&new_body).expect("valid new");

        let chain = [old, new];
        let heads = chain_heads(&chain).expect("resolvable");
        assert_eq!(heads.len(), 1, "only the head is served");
        assert_eq!(heads[0].meta.id, "01J8K2A7B3C4D5E6F7G8H9J0MN");
    }

    /// A decision that replaces one nobody committed would leave the old record
/// alive forever as a chain head: two heads, both current, no way to tell
/// which one won.
#[test]
fn a_supersedes_entry_that_points_at_nothing_is_an_error() {
    let lone = parse(&good().replace("supersedes = []", &format!(
        "supersedes = [\"01J8K2A7B3C4D5E6F7G8H9J0PZ\"]"
    )))
    .expect("valid");

    let err = chain_heads(&[lone]).expect_err("dangling supersedes must be refused");
    assert!(err.to_string().contains("01J8K2A7B3C4D5E6F7G8H9J0PZ"), "{}", err);
}

/// A cycle is a data-entry accident, and a naive resolver loops forever
    /// or silently drops both links. It must name the chain and stop.
    #[test]
    fn a_supersede_cycle_is_an_error_not_a_silent_drop() {
        let a = "01J8K2A7B3C4D5E6F7G8H9J0KM";
        let b = "01J8K2A7B3C4D5E6F7G8H9J0MN";
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

    #[test]
    fn a_generated_id_is_a_ulid_and_sorts_by_time() {
        let first = new_ulid();
        assert!(is_ulid(&first), "generated {} is not a ULID", first);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = new_ulid();
        assert!(is_ulid(&second));
        assert_ne!(first, second, "two ids in a row must not collide");
        assert!(
            first < second,
            "ULIDs must sort chronologically: {} !< {}",
            first,
            second
        );
        // The first ten characters ARE the timestamp. If they do not move
        // when the clock does, ordering is luck, not design.
        assert_ne!(
            &first[..10],
            &second[..10],
            "the timestamp prefix must carry the time: {} vs {}",
            first,
            second
        );
    }

    #[test]
    fn the_epoch_formats_as_the_epoch() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_762_000_000), "2025-11-01T12:26:40Z");
    }

    /// `recorded_at` is compared and sorted as text in git and in the
    /// index, so the formatter must only ever produce the accepted shape.
    #[test]
    fn now_is_stampable_and_round_trips_validation() {
        let stamp = now_rfc3339();
        assert!(is_rfc3339_utc(&stamp), "generated {} was refused", stamp);
    }
}