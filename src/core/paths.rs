#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn strips_dot_slash_prefix() {
        assert_eq!(normalize_in("./src/a.ts", Path::new("/tmp/proj")), "src/a.ts");
    }

    #[test]
    fn collapses_duplicate_separators() {
        assert_eq!(normalize_in("src//a.ts", Path::new("/tmp/proj")), "src/a.ts");
    }

    #[test]
    fn converts_backslashes_to_forward_slashes() {
        assert_eq!(normalize_in("src\\a.ts", Path::new("/tmp/proj")), "src/a.ts");
    }

    #[test]
    fn resolves_parent_segments() {
        assert_eq!(normalize_in("src/lib/../a.ts", Path::new("/tmp/proj")), "src/a.ts");
    }

    #[test]
    fn resolves_dot_segments() {
        assert_eq!(normalize_in("./src/./lib/a.ts", Path::new("/tmp/proj")), "src/lib/a.ts");
    }

    #[test]
    fn absolute_path_under_root_becomes_relative() {
        assert_eq!(normalize_in("/tmp/proj/src/a.ts", Path::new("/tmp/proj")), "src/a.ts");
    }

    #[test]
    fn absolute_path_with_dot_segments_under_root_becomes_relative() {
        assert_eq!(
            normalize_in("/tmp/proj/src/lib/../a.ts", Path::new("/tmp/proj")),
            "src/a.ts"
        );
    }

    #[test]
    fn trailing_separator_is_dropped() {
        assert_eq!(normalize_in("/tmp/proj/src/", Path::new("/tmp/proj")), "src");
    }

    #[test]
    fn absolute_path_outside_root_is_left_absolute() {
        assert_eq!(normalize_in("/elsewhere/a.ts", Path::new("/tmp/proj")), "/elsewhere/a.ts");
    }

    #[test]
    fn parent_segments_escaping_root_are_not_resolved_away() {
        assert_eq!(normalize_in("../a.ts", Path::new("/tmp/proj")), "../a.ts");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(normalize_in("", Path::new("/tmp/proj")), "");
    }

    #[test]
    fn root_itself_normalizes_to_empty() {
        assert_eq!(normalize_in("/tmp/proj", Path::new("/tmp/proj")), "");
    }

    #[test]
    fn relative_path_is_unchanged_when_already_canonical() {
        assert_eq!(normalize_in("src/a.ts", Path::new("/tmp/proj")), "src/a.ts");
    }

    #[test]
    fn windows_drive_prefix_is_stripped_when_under_root() {
        assert_eq!(
            normalize_in("C:/proj/src/a.ts", Path::new("C:/proj")),
            "src/a.ts"
        );
    }

    #[test]
    fn repeated_normalization_is_idempotent() {
        let once = normalize_in("./src/lib/../a.ts", Path::new("/tmp/proj"));
        let twice = normalize_in(&once, Path::new("/tmp/proj"));
        assert_eq!(once, twice);
    }

    #[test]
    fn normalize_uses_current_dir_as_root() {
        let cwd = std::env::current_dir().unwrap();
        let abs: PathBuf = cwd.join("src/a.ts");
        assert_eq!(normalize(abs.to_str().unwrap()), "src/a.ts");
    }
}