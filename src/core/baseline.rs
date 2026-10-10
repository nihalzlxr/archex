//! `baseline.toml` — recorded existing violations, checked in.
//!
//! CI is stateless: it cannot know what the codebase looked like last week.
//! So old debt is recorded in git, and the gate fails only on what is NEW.
//!
//! The key discipline is churn-proofness. A boundary violation is keyed by
//! (rule, from_file, to_file) and NEVER by line number: a violation that
//! moves down the file because someone added an import is the same
//! violation, and re-recording it every week trains people to ignore the
//! file.

use crate::core::boundary::Finding;
use crate::core::config::ConfigError;
use serde::{Deserialize, Serialize};

pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

/// Recorded debt. Committed, because CI is stateless.
#[derive(Debug, Default, Clone)]
pub struct Baseline {
    pub violations: Vec<Entry>,
}

/// One recorded violation. Boundary entries name both files and carry no
/// line number; pattern entries name a file and a count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BaselineFile {
    schema_version: i64,
    #[serde(default)]
    violation: Vec<Entry>,
}

impl Baseline {
    pub fn parse_str(body: &str) -> Result<Self, ConfigError> {
        let file: BaselineFile =
            toml::from_str(body).map_err(|e| ConfigError(format!("invalid baseline.toml: {}", e)))?;

        if file.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ConfigError(format!(
                "baseline.toml schema_version {} is not supported (this build understands {})",
                file.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }

        let mut violations: Vec<Entry> = Vec::new();
        for entry in file.violation {
            entry.validate()?;
            if !violations.contains(&entry) {
                violations.push(entry);
            }
        }
        Ok(Self { violations })
    }

    /// A repo with no baseline yet has no recorded debt. That is not an
    /// error, it is the state before you record any.
    pub fn load(root: &std::path::Path) -> Result<Self, ConfigError> {
        let path = root.join(".archex/baseline.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let body = std::fs::read_to_string(&path)
            .map_err(|e| ConfigError(format!("cannot read {}: {}", path.display(), e)))?;
        Self::parse_str(&body).map_err(|e| ConfigError(format!("{}: {}", path.display(), e)))
    }

    pub fn path(root: &std::path::Path) -> std::path::PathBuf {
        root.join(".archex/baseline.toml")
    }

    pub fn from_findings(findings: &[Finding]) -> Self {
        let mut violations: Vec<Entry> = findings
            .iter()
            .map(|f| Entry {
                rule: f.rule_id.clone(),
                from_file: Some(f.from_file.clone()),
                to_file: Some(f.to_file.clone()),
                file: None,
                count: None,
            })
            .collect();
        violations.sort_by(|a, b| {
            a.rule
                .cmp(&b.rule)
                .then(a.from_file.cmp(&b.from_file))
                .then(a.to_file.cmp(&b.to_file))
        });
        violations.dedup();
        Self { violations }
    }

    /// Line numbers are deliberately absent from the key. The same edge at a
    /// different line is the same violation, and re-recording it every week
    /// teaches people to ignore the file.
    pub fn absorbs(&self, finding: &Finding) -> bool {
        self.violations.iter().any(|entry| {
            entry.rule == finding.rule_id
                && entry.from_file.as_deref() == Some(finding.from_file.as_str())
                && entry.to_file.as_deref() == Some(finding.to_file.as_str())
        })
    }

    /// A pattern violation goes away when the count drops and comes back when
    /// it rises, so the count is compared, not just the file. The pattern
    /// engine is the next slice; count entries already parse and validate.
    #[allow(dead_code)]
    pub fn absorbs_count(&self, rule: &str, file: &str, count: i64) -> bool {
        self.violations
            .iter()
            .find(|entry| entry.rule == rule && entry.file.as_deref() == Some(file))
            .and_then(|entry| entry.count)
            .map(|recorded| count <= recorded)
            .unwrap_or(false)
    }

    pub fn to_toml(&self) -> String {
        let file = BaselineFile {
            schema_version: SUPPORTED_SCHEMA_VERSION,
            violation: self.violations.clone(),
        };
        toml::to_string_pretty(&file).expect("baseline serializes")
    }
}

impl Entry {
    fn validate(&self) -> Result<(), ConfigError> {
        let is_boundary = self.from_file.is_some() || self.to_file.is_some();
        let is_count = self.file.is_some() || self.count.is_some();

        if is_boundary && is_count {
            return Err(ConfigError(format!(
                "baseline entry for `{}` mixes boundary and count fields",
                self.rule
            )));
        }
        if is_boundary {
            let missing = if self.from_file.is_none() {
                "from_file"
            } else if self.to_file.is_none() {
                "to_file"
            } else {
                return Ok(());
            };
            return Err(ConfigError(format!(
                "baseline entry for `{}` is missing `{}`",
                self.rule, missing
            )));
        }
        if is_count {
            let missing = if self.file.is_none() {
                "file"
            } else if self.count.is_none() {
                "count"
            } else {
                return Ok(());
            };
            return Err(ConfigError(format!(
                "baseline entry for `{}` is missing `{}`",
                self.rule, missing
            )));
        }
        Err(ConfigError(format!(
            "baseline entry for `{}` has neither files nor a count: it records nothing",
            self.rule
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::boundary::Finding;
    use crate::core::config::Severity;

    fn finding(from: &str, to: &str, line: i64) -> Finding {
        Finding {
            rule_id: "r".to_string(),
            severity: Severity::Error,
            from_file: from.to_string(),
            to_file: to.to_string(),
            line_number: line,
            imported_from: format!("../{}", to),
            message: "m".to_string(),
        }
    }

    fn parse(body: &str) -> Baseline {
        Baseline::parse_str(body).expect("valid baseline")
    }

    #[test]
    fn round_trips_through_toml() {
        let baseline = Baseline::from_findings(&[
            finding("apps/web/a.ts", "packages/engine/src/projection.ts", 3),
            finding("apps/web/b.ts", "packages/engine/src/plan.ts", 9),
        ]);
        let text = baseline.to_toml();
        let again = parse(&text);
        assert_eq!(again.violations.len(), 2);
        assert!(again.absorbs(&finding("apps/web/a.ts", "packages/engine/src/projection.ts", 3)));
    }

    /// The whole point: recorded debt does not fail the gate.
    #[test]
    fn a_recorded_violation_is_absorbed() {
        let baseline = Baseline::from_findings(&[finding("a.ts", "b.ts", 1)]);
        assert!(baseline.absorbs(&finding("a.ts", "b.ts", 1)));
    }

    /// Line numbers churn every week. Keying on them would make the baseline
    /// useless within a day.
    #[test]
    fn moving_down_the_file_does_not_create_a_new_violation() {
        let baseline = Baseline::from_findings(&[finding("a.ts", "b.ts", 1)]);
        assert!(
            baseline.absorbs(&finding("a.ts", "b.ts", 480)),
            "the same edge at a different line is the same violation"
        );
    }

    #[test]
    fn a_different_target_is_a_new_violation() {
        let baseline = Baseline::from_findings(&[finding("a.ts", "b.ts", 1)]);
        assert!(!baseline.absorbs(&finding("a.ts", "c.ts", 1)));
    }

    #[test]
    fn a_different_source_file_is_a_new_violation() {
        let baseline = Baseline::from_findings(&[finding("a.ts", "b.ts", 1)]);
        assert!(!baseline.absorbs(&finding("z.ts", "b.ts", 1)));
    }

    #[test]
    fn a_different_rule_is_a_new_violation() {
        let baseline = Baseline::from_findings(&[finding("a.ts", "b.ts", 1)]);
        let mut other = finding("a.ts", "b.ts", 1);
        other.rule_id = "different".to_string();
        assert!(!baseline.absorbs(&other));
    }

    /// A file nobody has touched is not in the baseline, and a rule that was
    /// not written yet cannot retroactively bless old debt.
    #[test]
    fn an_empty_baseline_absorbs_nothing() {
        let baseline = Baseline::default();
        assert!(!baseline.absorbs(&finding("a.ts", "b.ts", 1)));
    }

    #[test]
    fn an_empty_baseline_serializes_without_an_empty_table() {
        let text = Baseline::default().to_toml();
        assert!(!text.contains("[[violation]]"), "{}", text);
        assert_eq!(parse(&text).violations.len(), 0);
    }

    #[test]
    fn entries_are_sorted_so_diffs_stay_small() {
        let baseline = Baseline::from_findings(&[
            finding("z.ts", "b.ts", 1),
            finding("a.ts", "c.ts", 1),
            finding("a.ts", "b.ts", 1),
        ]);
        let text = baseline.to_toml();
        let order: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("from_file = \""))
            .map(|l| l.split('"').next().unwrap_or(""))
            .collect();
        assert_eq!(order, vec!["a.ts", "a.ts", "z.ts"]);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = Baseline::parse_str(
            "schema_version = 1\n[[violation]]\nrule = \"r\"\nfrom_file = \"a\"\nto_file = \"b\"\nline_number = 3\n",
        )
        .expect_err("line numbers are not part of a boundary key");
        assert!(err.to_string().contains("line_number"), "{}", err);
    }

    #[test]
    fn a_half_written_entry_is_rejected() {
        let err = Baseline::parse_str(
            "schema_version = 1\n[[violation]]\nrule = \"r\"\nfrom_file = \"a\"\n",
        )
        .expect_err("a boundary entry needs both files");
        assert!(err.to_string().contains("to_file"), "{}", err);
    }

    #[test]
    fn count_entries_are_for_pattern_rules() {
        let baseline = parse(
            "schema_version = 1\n[[violation]]\nrule = \"no-fetch\"\nfile = \"src/a.tsx\"\ncount = 3\n",
        );
        assert!(baseline.absorbs_count("no-fetch", "src/a.tsx", 3));
        assert!(
            !baseline.absorbs_count("no-fetch", "src/a.tsx", 4),
            "one more occurrence is new debt"
        );
        assert!(baseline.absorbs_count("no-fetch", "src/a.tsx", 2));
    }

    #[test]
    fn schema_version_must_be_known() {
        let err = Baseline::parse_str("schema_version = 99\n").expect_err("unknown version");
        assert!(err.to_string().contains("schema_version"), "{}", err);
    }

    #[test]
    fn duplicate_entries_collapse() {
        let baseline = parse(
            "schema_version = 1\n\
             [[violation]]\nrule = \"r\"\nfrom_file = \"a\"\nto_file = \"b\"\n\
             [[violation]]\nrule = \"r\"\nfrom_file = \"a\"\nto_file = \"b\"\n",
        );
        assert_eq!(baseline.violations.len(), 1);
    }

    #[test]
    fn a_missing_baseline_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = Baseline::load(dir.path()).expect("no baseline yet is fine");
        assert!(baseline.violations.is_empty());
    }
}