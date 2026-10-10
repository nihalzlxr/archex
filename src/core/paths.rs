use std::path::Path;

/// Canonical form of a path as archex stores and looks it up: repo-relative,
/// forward slashes, no `.` or `..` segments. Every path that reaches the DB --
/// written by the scanner, read back by `get_context` from whatever string the
/// agent happened to pass -- goes through here, so both sides agree.
pub fn normalize_in(input: &str, root: &Path) -> String {
    let input = input.trim();
    if input.is_empty() {
        return String::new();
    }

    let unified = input.replace('\\', "/");
    let root_slash = root.to_string_lossy().replace('\\', "/");
    let root_slash = root_slash.trim_end_matches('/');

    // An absolute path under the repo root becomes repo-relative. Anything
    // else stays absolute: guessing a root it does not belong to would file
    // the entry under a module that does not own it.
    let absolute = looks_absolute(&unified);
    let relative = if absolute && !root_slash.is_empty() {
        unified.strip_prefix(root_slash)
    } else {
        None
    };

    let (stripped, made_relative) = match relative {
        Some(rest) => (rest.trim_start_matches('/').to_string(), true),
        None => (unified.clone(), false),
    };

    let mut out: Vec<String> = Vec::new();
    for segment in stripped.split('/') {
        match segment {
            "" | "." => {}
            ".." => match out.last().map(|s| s.as_str()) {
                // A real segment above us: fold it away.
                Some(last) if last != ".." => {
                    out.pop();
                }
                // Nothing left to fold: keep the `..` so the path stays honest
                // about pointing outside the repo.
                _ => out.push("..".to_string()),
            },
            _ => out.push(segment.to_string()),
        }
    }

    let joined = out.join("/");
    if made_relative {
        return joined;
    }
    if !absolute {
        return joined;
    }
    if joined.is_empty() {
        return root_slash.to_string();
    }
    // Still absolute: the leading separator was consumed by split('/').
    format!("/{}", joined)
}

/// `/x` and `C:/x` are both absolute; `C:x` is a drive-relative path and is
/// not. On Unix `Path::is_absolute` says no to `C:/x`, so match by hand.
fn looks_absolute(path: &str) -> bool {
    path.starts_with('/') || {
        let bytes = path.as_bytes();
        bytes.len() > 2 && bytes[1] == b':' && (bytes[2] == b'/' || bytes[2] == b'\\')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

}