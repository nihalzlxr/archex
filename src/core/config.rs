//! `.archex/config.toml` — the committed architecture.
//!
//! Presence of this file is what makes a module real. Without it archex
//! infers a bootstrap default (see `cli::init::seeding_plan`), which is a
//! guess; with it, the humans own the architecture and archex enforces it.

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
        let cfg = parse("schema_version = 1\nlayers = []\n").expect("valid");
        assert_eq!(cfg.test_globs(), DEFAULT_TEST_GLOBS);
    }

    #[test]
    fn test_globs_override_the_default() {
        let cfg = parse(
            "schema_version = 1\nlayers = []\ntest_globs = [\"spec/**/*.spec.ts\"]\n",
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
        let err = parse("schema_version = 1\nlayers = []\nlayerz = [\"ui\"]\n")
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
        let cfg = parse("layers = [\"ui\", \"service\"]\n").expect("valid");
        assert_eq!(cfg.layers.len(), 2);
    }

    #[test]
    fn schema_version_must_be_known() {
        let err = parse("schema_version = 99\nlayers = []\n").expect_err("must not parse");
        assert!(err.to_string().contains("schema_version"), "{}", err);
    }

    #[test]
    fn name_is_required() {
        let err = parse("layers = []\n").expect_err("must not parse");
        assert!(err.to_string().contains("name"), "{}", err);
    }

    #[test]
    fn every_module_needs_a_path() {
        let err = parse("schema_version = 1\nname = \"x\"\nlayers = [\"ui\"]\n[[module]]\nname = \"web\"\nlayer = \"ui\"\n")
            .expect_err("must not parse");
        assert!(err.to_string().contains("path"), "{}", err);
    }
}
