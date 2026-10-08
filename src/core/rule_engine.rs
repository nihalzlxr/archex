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
