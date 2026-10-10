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