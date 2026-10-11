use regex::Regex;

/// Case-insensitive rule-pattern match against file content.
///
/// The pattern is treated as a regex and matched against the ORIGINAL content
/// with the `(?i)` flag — pattern case and content case must never disagree.
/// If the pattern is not valid regex, falls back to case-insensitive
/// substring containment.
///
/// This is the single matcher for all rule-checking paths (check CLI,
/// check_drift MCP tool, rule test). Never compile a rule pattern against
/// lowercased content with an original-case pattern — that silently never
/// matches (the bug this function replaced).
///
/// Currently unreferenced by any enforcement path: the file-based pattern
/// engine (PLAN.md §19.1) is the next slice, and the DB-backed path that used
/// this matcher was removed in Step 2.5. Kept, with its case-sensitivity
/// regression tests, because the file-based engine is the reason it exists.
#[allow(dead_code)]
pub fn pattern_matches(content: &str, pattern: &str) -> bool {
    let case_insensitive = format!("(?i){}", pattern);
    if let Ok(re) = Regex::new(&case_insensitive) {
        re.is_match(content)
    } else {
        let content_lower = content.to_lowercase();
        let pattern_lower = pattern.to_lowercase();
        content_lower.contains(&pattern_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::pattern_matches;

    // The original bug: the pattern was compiled with its original case and
    // matched against lowercased content, so any pattern containing an
    // uppercase literal silently never fired. Guards the matching semantics
    // for the file-based pattern engine (PLAN.md §19.1), which is where these
    // rules are evaluated once it lands.
    #[test]
    fn uppercase_pattern_matches_lowercase_content() {
        assert!(pattern_matches("  fetch('/api/user');\n", r"Fetch\("));
    }

    #[test]
    fn lowercase_pattern_matches_lowercase_content() {
        assert!(pattern_matches("  fetch('/api/user');\n", r"fetch\("));
    }

    #[test]
    fn pattern_does_not_match_unrelated_content() {
        assert!(!pattern_matches("export function Widget() {}\n", r"fetch\("));
    }

    // A pattern that is not valid regex falls back to substring containment,
    // case-insensitively, rather than silently never matching.
    #[test]
    fn invalid_regex_falls_back_to_case_insensitive_substring() {
        assert!(pattern_matches("import { z } from 'zod';\n", "from 'zod'"));
        assert!(!pattern_matches("import { y } from 'yup';\n", "from 'zod'"));
    }
}
