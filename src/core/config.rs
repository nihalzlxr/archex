//! `.archex/config.toml` — the committed architecture.
//!
//! Presence of this file is what makes a module real. Without it archex
//! infers a bootstrap default (see `cli::init::seeding_plan`), which is a
//! guess; with it, the humans own the architecture and archex enforces it.


use serde::Deserialize;

/// The config format this build understands. A newer file is a hard error,
/// not something to guess at.
pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

/// Files matched here are skipped by boundary rules unless a rule opts in.
/// Safe behaviour by default: a rule that flags every test file is a rule
/// that gets disabled on day one.
pub const DEFAULT_TEST_GLOBS: &[&str] = &[
    "**/*.test.ts",
    "**/*.test.tsx",
    "**/*.spec.ts",
    "**/*.spec.tsx",
    "**/test/**",
    "**/tests/**",
    "**/__tests__/**",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: i64,
    pub name: String,
    pub layers: Vec<String>,
    #[serde(default)]
    pub test_globs: Option<Vec<String>>,
    #[serde(default, rename = "module")]
    pub modules: Vec<ModuleConfig>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ModuleConfig {
    pub name: String,
    pub layer: String,
    pub path: String,
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

impl From<rusqlite::Error> for ConfigError {
    fn from(error: rusqlite::Error) -> Self {
        ConfigError(error.to_string())
    }
}

impl Config {
    pub fn test_globs(&self) -> Vec<&str> {
        match &self.test_globs {
            Some(globs) => globs.iter().map(|g| g.as_str()).collect(),
            None => DEFAULT_TEST_GLOBS.to_vec(),
        }
    }

    pub fn parse_str(body: &str) -> Result<Self, ConfigError> {
        let value: toml::Value =
            toml::from_str(body).map_err(|e| ConfigError(format!("invalid TOML: {}", e)))?;

        // A bare key after a table header is absorbed into that table by
        // TOML's own rules, so `layers = [...]` under a [[module]] parses
        // cleanly and leaves the top level empty. Catch it before
        // serde does, while we still know the key was misplaced.
        if let Some(misplaced) = misplaced_top_level_key(&value) {
            return Err(ConfigError(format!(
                "`{}` must appear before any [[module]] table: a bare key after a                  table header is silently absorbed into that table, so it never                  reaches the top level.",
                misplaced
            )));
        }

        let config: Config = value
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError(format!("invalid config: {}", e)))?;

        config.validate()?;
        Ok(config)
    }

    pub fn load(root: &std::path::Path) -> Result<Self, ConfigError> {
        let path = root.join(".archex/config.toml");
        let body = std::fs::read_to_string(&path).map_err(|e| {
            ConfigError(format!("cannot read {}: {}", path.display(), e))
        })?;
        Self::parse_str(&body)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ConfigError(format!(
                "schema_version {} is not supported (this build understands {})",
                self.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }

        let mut seen: Vec<&str> = Vec::new();
        for module in &self.modules {
            if seen.contains(&module.name.as_str()) {
                return Err(ConfigError(format!(
                    "duplicate module name `{}`: each module maps one directory tree",
                    module.name
                )));
            }
            seen.push(&module.name);

            // An undeclared layer is a mistake, not a module that quietly
            // matches nothing. A gate that silently passes is worse than one
            // that refuses to start.
            if !self.layers.iter().any(|l| l == &module.layer) {
                return Err(ConfigError(format!(
                    "module `{}` is in layer `{}`, which is not declared in layers = [...]",
                    module.name, module.layer
                )));
            }
        }
        Ok(())
    }
}

/// Names that must live at the top level, checked for accidental capture by
/// a table header.
fn misplaced_top_level_key(value: &toml::Value) -> Option<String> {
    let table = value.as_table()?;
    const MUST_BE_TOP_LEVEL: &[&str] = &["schema_version", "layers", "test_globs"];
    for module in table.get("module")?.as_array()? {
        let module = module.as_table()?;
        for key in MUST_BE_TOP_LEVEL {
            if module.contains_key(*key) {
                return Some((*key).to_string());
            }
        }
    }
    None
}

/// One rule, one file. `kind` must match exactly one engine table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub schema_version: i64,
    pub id: String,
    pub status: Status,
    pub kind: Kind,
    pub severity: Severity,
    #[serde(default)]
    pub boundary: Option<BoundaryRule>,
    #[serde(default)]
    pub pattern: Option<PatternRule>,
    #[serde(default)]
    pub presence: Option<PresenceRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Accepted,
    Proposed,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Boundary,
    Pattern,
    Presence,
}

impl Kind {
    fn table_name(&self) -> &'static str {
        match self {
            Kind::Boundary => "boundary",
            Kind::Pattern => "pattern",
            Kind::Presence => "presence",
        }
    }
}

/// error = CI exit 1. warning = report only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryRule {
    /// A layer name, or a module name. Resolved against declared layers
    /// first, then module names; ambiguity is an error, never a coin flip.
    pub from_layer: String,
    pub to_layer: String,
    /// Targets exempt from this rule: a package's public entrypoint is the
    /// legitimate way in, and internals are the thing being policed.
    #[serde(default)]
    pub allow_to: Vec<String>,
    /// Skip files matching the config's test_globs unless this is true.
    #[serde(default)]
    pub include_tests: bool,
    /// Anything else the rule should not apply to.
    #[serde(default)]
    pub exclude: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternRule {
    pub scope: String,
    pub mode: PatternMode,
    pub pattern: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatternMode {
    Forbidden,
    Required,
    Warning,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceRule {
    pub applies_to: String,
    pub requires: String,
    pub message: String,
}

impl Rule {
    pub fn parse_str(body: &str) -> Result<Self, ConfigError> {
        let value: toml::Value =
            toml::from_str(body).map_err(|e| ConfigError(format!("invalid TOML: {}", e)))?;
        let rule: Rule = value
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError(format!("invalid rule: {}", e)))?;
        rule.validate()?;
        Ok(rule)
    }

    /// A retired rule is kept for history and enforces nothing.
    pub fn is_active(&self) -> bool {
        self.status != Status::Retired
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ConfigError(format!(
                "rule `{}`: schema_version {} is not supported",
                self.id, self.schema_version
            )));
        }

        let present: Vec<(&str, bool)> = vec![
            ("boundary", self.boundary.is_some()),
            ("pattern", self.pattern.is_some()),
            ("presence", self.presence.is_some()),
        ];
        let count = present.iter().filter(|(_, is_present)| *is_present).count();

        if count != 1 {
            let named: Vec<&str> = present
                .iter()
                .filter(|(_, is_present)| *is_present)
                .map(|(name, _)| *name)
                .collect();
            return Err(ConfigError(format!(
                "rule `{}`: kind = \"{}\" must match exactly one engine table, found {} ({})",
                self.id,
                self.kind.table_name(),
                count,
                if named.is_empty() {
                    "none".to_string()
                } else {
                    named.join(", ")
                }
            )));
        }

        if self.kind.table_name() != present.iter().find(|(_, p)| *p).unwrap().0 {
            return Err(ConfigError(format!(
                "rule `{}`: kind = \"{}\" does not match its engine table",
                self.id,
                self.kind.table_name()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<Config, ConfigError> {
        Config::parse_str(body)
    }

    #[test]
    fn parses_a_complete_config() {
        let cfg = parse(
            r#"
schema_version = 1
name = "fitness"
layers = ["ui", "service", "engine"]

[[module]]
name = "web"
layer = "ui"
path = "apps/web/**"

[[module]]
name = "engine"
layer = "engine"
path = "packages/engine/**"
"#,
        )
        .expect("valid config");

        assert_eq!(cfg.name, "fitness");
        assert_eq!(cfg.layers, vec!["ui", "service", "engine"]);
        assert_eq!(cfg.modules.len(), 2);
        assert_eq!(cfg.modules[0].name, "web");
        assert_eq!(cfg.modules[0].path, "apps/web/**");
    }

    #[test]
    fn test_globs_default_when_absent() {
        let cfg = parse("schema_version = 1\nname = \"x\"\nlayers = []\n").expect("valid");
        assert_eq!(cfg.test_globs(), DEFAULT_TEST_GLOBS);
    }

    #[test]
    fn test_globs_override_the_default() {
        let cfg = parse(
            "schema_version = 1\nname = \"x\"\nlayers = []\ntest_globs = [\"spec/**/*.spec.ts\"]\n",
        )
        .expect("valid");
        assert_eq!(cfg.test_globs(), vec!["spec/**/*.spec.ts"]);
    }

    /// A bare key after a table header is NOT a TOML error -- it is silently
    /// absorbed into that table. The result is a config that parses cleanly
    /// with no top-level layers at all, which is exactly the kind of quiet
    /// misconfiguration this tool exists to prevent.
    #[test]
    fn layers_misplaced_after_a_module_header_is_rejected() {
        let err = parse(
            r#"
schema_version = 1
name = "x"

[[module]]
name = "web"
layer = "ui"
path = "apps/web/**"
layers = ["ui"]
"#,
        )
        .expect_err("must not parse");
        assert!(
            err.to_string().contains("layers"),
            "the error must name the offending key: {}",
            err
        );
    }

    /// deny_unknown_fields catches every typo, not just the one above.
    #[test]
    fn unknown_top_level_key_is_rejected() {
        let err = parse("schema_version = 1\nname = \"x\"\nlayers = []\nlayerz = [\"ui\"]\n")
            .expect_err("must not parse");
        assert!(err.to_string().contains("layerz"), "{}", err);
    }

    #[test]
    fn unknown_module_key_is_rejected() {
        let err = parse(
            "[[module]]\nname = \"web\"\nlayer = \"ui\"\npath = \"apps/web/**\"\npatterns = \"x\"\n",
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("patterns"), "{}", err);
    }

    #[test]
    fn duplicate_module_names_are_rejected() {
        let err = parse(
            r#"
schema_version = 1
name = "x"
layers = ["ui"]
[[module]]
name = "web"
layer = "ui"
path = "apps/web/**"
[[module]]
name = "web"
layer = "ui"
path = "apps/web-admin/**"
"#,
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("duplicate"), "{}", err);
    }

    /// A module in a layer that was never declared is a config error, never a
    /// module that quietly matches nothing.
    #[test]
    fn module_in_an_undeclared_layer_is_rejected() {
        let err = parse(
            r#"
schema_version = 1
name = "x"
layers = ["ui"]
[[module]]
name = "engine"
layer = "engine"
path = "packages/engine/**"
"#,
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("engine"), "{}", err);
        assert!(err.to_string().contains("layer"), "{}", err);
    }

    #[test]
    fn a_layer_with_no_modules_is_allowed() {
        // Declaring a layer ahead of the module that uses it is fine.
        let cfg = parse("schema_version = 1\nname = \"x\"\nlayers = [\"ui\", \"service\"]\n").expect("valid");
        assert_eq!(cfg.layers.len(), 2);
    }

    #[test]
    fn schema_version_must_be_known() {
        let err = parse("schema_version = 99\nname = \"x\"\nlayers = []\n").expect_err("must not parse");
        assert!(err.to_string().contains("schema_version"), "{}", err);
    }

    #[test]
    fn name_is_required() {
        let err = parse("schema_version = 1\nlayers = []\n").expect_err("must not parse");
        assert!(err.to_string().contains("name"), "{}", err);
    }

    #[test]
    fn every_module_needs_a_path() {
        let err = parse("schema_version = 1\nname = \"x\"\nlayers = [\"ui\"]\n[[module]]\nname = \"web\"\nlayer = \"ui\"\n")
            .expect_err("must not parse");
        assert!(err.to_string().contains("path"), "{}", err);
    }
}

/// `rules/<id>.toml` — one rule per file.
///
/// Three engines, one schema, `kind` selects. The invariant that matters:
/// `kind` must match exactly one engine table. A rule with two tables, or
/// none, is not "mostly fine" — it is a rule nobody knows what it checks.
#[cfg(test)]
mod rule_tests {
    use super::*;

    fn parse(body: &str) -> Result<Rule, ConfigError> {
        Rule::parse_str(body)
    }

    const BOUNDARY: &str = r#"
schema_version = 1
id = "web-uses-engine-entry"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "web"
to_layer = "engine"
allow_to = ["packages/engine/src/index.ts"]
message = "Import the engine through its public entrypoint."
"#;

    #[test]
    fn parses_a_boundary_rule() {
        let rule = parse(BOUNDARY).expect("valid rule");
        assert_eq!(rule.id, "web-uses-engine-entry");
        assert_eq!(rule.severity, Severity::Error);
        let boundary = rule.boundary.expect("boundary table");
        assert_eq!(boundary.from_layer, "web");
        assert_eq!(boundary.to_layer, "engine");
        assert_eq!(boundary.allow_to, vec!["packages/engine/src/index.ts"]);
        assert!(!boundary.include_tests);
    }

    #[test]
    fn include_tests_opts_back_in() {
        let rule = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "a"
to_layer = "b"
include_tests = true
message = "m"
"#,
        )
        .expect("valid");
        assert!(rule.boundary.unwrap().include_tests);
    }

    #[test]
    fn allow_to_defaults_to_empty() {
        let rule = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "a"
to_layer = "b"
message = "m"
"#,
        )
        .expect("valid");
        assert!(rule.boundary.unwrap().allow_to.is_empty());
    }

    #[test]
    fn kind_boundary_with_no_boundary_table_is_rejected() {
        let err = parse(
            "schema_version = 1\nid = \"r\"\nstatus = \"accepted\"\nkind = \"boundary\"\nseverity = \"error\"\nmessage = \"m\"\n",
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("boundary"), "{}", err);
    }

    #[test]
    fn two_engine_tables_are_rejected() {
        let err = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "a"
to_layer = "b"
message = "m"

[pattern]
scope = "apps/web/**"
mode = "forbidden"
pattern = "x"
message = "m"
"#,
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("exactly one"), "{}", err);
    }

    /// The exact failure the plan warns about: a typo in `allow_to` that
    /// silently disables the exception.
    #[test]
    fn typo_in_allow_to_is_rejected() {
        let err = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "a"
to_layer = "b"
allow_too = ["x"]
message = "m"
"#,
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("allow_too"), "{}", err);
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let err = parse(
            "schema_version = 1\nid = \"r\"\nstatus = \"accepted\"\nkind = \"vibes\"\nseverity = \"error\"\n",
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("vibes"), "{}", err);
    }

    #[test]
    fn unknown_status_is_rejected() {
        let err = parse(
            "schema_version = 1\nid = \"r\"\nstatus = \"maybe\"\nkind = \"boundary\"\nseverity = \"error\"\n",
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("maybe"), "{}", err);
    }

    #[test]
    fn boundary_from_and_to_are_required() {
        let err = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
message = "m"
"#,
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("from_layer"), "{}", err);
    }

    #[test]
    fn boundary_message_is_required() {
        let err = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "error"

[boundary]
from_layer = "a"
to_layer = "b"
"#,
        )
        .expect_err("must not parse");
        assert!(err.to_string().contains("message"), "{}", err);
    }

    #[test]
    fn severity_warning_is_allowed() {
        let rule = parse(
            r#"
schema_version = 1
id = "r"
status = "accepted"
kind = "boundary"
severity = "warning"

[boundary]
from_layer = "a"
to_layer = "b"
message = "m"
"#,
        )
        .expect("valid");
        assert_eq!(rule.severity, Severity::Warning);
    }
}
