use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Extensions a relative import may resolve to, in the order a bundler would
/// try them. `.ts`/`.tsx` before `.js` because in a TypeScript repo the
/// source is the truth and the `.js` specifier is a NodeNext convention.
const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];

const INDEX_FILES: &[&str] = &[
    "index.ts",
    "index.tsx",
    "index.js",
    "index.jsx",
    "index.mjs",
];

/// Non-code specifiers: never resolvable, never a violation.
const NON_CODE_EXTENSIONS: &[&str] = &[
    "css", "scss", "sass", "less", "json", "svg", "png", "jpg", "jpeg", "gif", "webp", "avif",
    "ico", "woff", "woff2", "ttf", "eot", "md", "mdx", "txt", "yml", "yaml", "html", "wasm",
];

/// One tsconfig's worth of path knowledge.
#[derive(Debug, Clone, Default)]
struct TsConfig {
    /// Directory the `paths` replacements are relative to (baseUrl, or the
    /// tsconfig's own directory when it has no baseUrl).
    base: PathBuf,
    /// ("@/*", "src/*") pairs, longest prefix first.
    aliases: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Resolver {
    root: PathBuf,
    /// tsconfigs keyed by the directory holding them, so the nearest one wins.
    tsconfigs: HashMap<PathBuf, TsConfig>,
    /// package name -> package directory (workspace links).
    workspace: HashMap<String, PathBuf>,
    /// package name -> its exports/main entry points.
    entries: HashMap<String, PackageEntry>,
}

#[derive(Debug, Default)]
struct PackageEntry {
    /// (subpath, target) from `exports`, e.g. ("./program", "./src/program.ts")
    exports: Vec<(String, String)>,
    main: Option<String>,
}

/// Resolves an import specifier to a repo-relative path, or to `None`.
///
/// `None` means UNCLASSIFIED, not "no violation". Callers must treat the two
/// differently: an unclassified edge is invisible to boundary rules.
impl Resolver {
    pub fn new(root: &Path) -> std::io::Result<Self> {
        let mut resolver = Resolver {
            root: root.to_path_buf(),
            ..Default::default()
        };
        resolver.load_workspace();
        resolver.load_tsconfigs(root, &mut Vec::new());
        Ok(resolver)
    }

    pub fn resolve(&self, from_file: &str, specifier: &str) -> Option<String> {
        if specifier.is_empty() {
            return None;
        }
        // Builtins, subpath imports, URLs and asset extensions are not code.
        if specifier.starts_with("node:")
            || specifier.starts_with('#')
            || specifier.starts_with("http://")
            || specifier.starts_with("https://")
            || specifier.starts_with("data:")
        {
            return None;
        }
        if let Some(ext) = std::path::Path::new(specifier)
            .extension()
            .and_then(|e| e.to_str())
        {
            if NON_CODE_EXTENSIONS.contains(&ext) {
                return None;
            }
        }

        if specifier.starts_with("./") || specifier.starts_with("../") {
            let dir = Path::new(from_file).parent().unwrap_or(Path::new(""));
            let joined = dir.join(specifier);
            return self.resolve_file_candidates(&joined);
        }

        if let Some(resolved) = self.resolve_alias(from_file, specifier) {
            return Some(resolved);
        }

        self.resolve_package(specifier)
    }

    /// Walk up from the importing file to the nearest tsconfig, then try each
    /// alias replacement. Nearest wins, because that is what the toolchain
    /// does.
    fn resolve_alias(&self, from_file: &str, specifier: &str) -> Option<String> {
        // Files are stored repo-relative but tsconfigs are keyed by absolute
        // directory: walk up from the absolute location.
        let from_dir = self.root.join(from_file);
        let from_dir = from_dir.parent()?.to_path_buf();
        for dir in ancestors(&from_dir) {
            let Some(config) = self.tsconfigs.get(&dir) else {
                continue;
            };
            for (pattern, replacement) in &config.aliases {
                if let Some(rest) = match_pattern(pattern, specifier) {
                    let target = config.base.join(replacement.replace('*', &rest));
                    if let Some(hit) = self.resolve_file_candidates(&target) {
                        return Some(hit);
                    }
                }
            }
            // A tsconfig was found: aliases below it are shadowed, and a
            // specifier it does not define is not its business.
            return None;
        }
        None
    }

    fn resolve_package(&self, specifier: &str) -> Option<String> {
        let (pkg, subpath) = split_package_specifier(specifier);
        let pkg = pkg.as_str();

        let dir = self.workspace.get(pkg)?;
        let entry = self.entries.get(pkg);

        if let Some(sub) = subpath.as_deref() {
            if let Some(entry) = entry {
                for (key, target) in &entry.exports {
                    if key == sub || key.strip_prefix("./").unwrap_or(key) == sub {
                        if let Some(hit) = self.resolve_file_candidates(&dir.join(target)) {
                            return Some(hit);
                        }
                    }
                    if let Some(rest) = match_pattern(key, sub) {
                        let expanded = target.replace('*', &rest);
                        if let Some(hit) = self.resolve_file_candidates(&dir.join(expanded)) {
                            return Some(hit);
                        }
                    }
                }
            }
            // No exports entry: try the subpath as a path inside the package,
            // then the same subpath under `src/` (the near-universal layout).
            // Both rungs still require the file to exist -- this never invents
            // a resolution, it only looks in one more place.
            return self
                .resolve_file_candidates(&dir.join(sub))
                .or_else(|| self.resolve_file_candidates(&dir.join("src").join(sub)));
        }

        if let Some(entry) = entry {
            if let Some(main) = &entry.main {
                if let Some(hit) = self.resolve_file_candidates(&dir.join(main)) {
                    return Some(hit);
                }
            }
            for (_, target) in &entry.exports {
                if let Some(hit) = self.resolve_file_candidates(&dir.join(target)) {
                    return Some(hit);
                }
            }
        }
        self.resolve_file_candidates(&dir.join("src/index.ts"))
            .or_else(|| self.resolve_file_candidates(&dir.join("index.ts")))
    }

    /// The candidate ladder for one target path: exact, ESM `.js` stripped,
    /// extension added, then a directory index.
    fn resolve_file_candidates(&self, target: &Path) -> Option<String> {
        let normalized = normalize_repo_path(&self.root, target)?;
        let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");

        // 1. exact file
        if self.root.join(&normalized).is_file() && is_code(ext) {
            return Some(normalized);
        }

        // 2. NodeNext: `./x.js` that really means `./x.ts`
        if ext == "js" || ext == "mjs" || ext == "cjs" {
            let stem = target.with_extension("");
            if let Some(hit) = self.resolve_file_candidates(&stem) {
                return Some(hit);
            }
        }

        // 3. add an extension
        for candidate_ext in EXTENSIONS {
            let with_ext = target.with_extension(candidate_ext);
            if let Some(norm) = normalize_repo_path(&self.root, &with_ext) {
                if self.root.join(&norm).is_file() {
                    return Some(norm);
                }
            }
        }

        // 4. directory with an index
        for index in INDEX_FILES {
            if let Some(norm) = normalize_repo_path(&self.root, &target.join(index)) {
                if self.root.join(&norm).is_file() {
                    return Some(norm);
                }
            }
        }
        None
    }

    // --- loading ---

    fn load_workspace(&mut self) {
        let globs = workspace_globs(&self.root);
        for pattern in globs {
            for dir in expand_glob_for(&self.root, &pattern) {
                let manifest = dir.join("package.json");
                let Ok(body) = std::fs::read_to_string(&manifest) else {
                    continue;
                };
                let Some(json) = parse_json_loose(&body) else {
                    continue;
                };
                let Some(name) = json.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let rel = crate::core::paths::normalize_in(
                    &dir.to_string_lossy(),
                    &self.root,
                );
                self.workspace.insert(name.to_string(), dir.clone());

                let mut entry = PackageEntry::default();
                entry.main = json
                    .get("main")
                    .or_else(|| json.get("module"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if let Some(exports) = json.get("exports") {
                    collect_exports(exports, &mut entry.exports);
                }
                let _ = rel;
                self.entries.insert(name.to_string(), entry);
            }
        }
    }

    /// `extends` names a FILE, and that file is often called something else
    /// entirely (`tsconfig.base.json`). Walk the chain by path.
    fn load_config_file(&mut self, config_path: &Path, seen: &mut Vec<PathBuf>) {
        if seen.iter().any(|p| p == config_path) {
            return;
        }
        seen.push(config_path.to_path_buf());

        let Ok(body) = std::fs::read_to_string(config_path) else {
            return;
        };
        let json = parse_json_loose(&body).unwrap_or_default();
        // `extends: "../../base.json"` leaves `..` components in the path;
        // the directory must be lexically clean or it will never match the
        // directory we walk up from when resolving.
        let dir = lexical_normalize(config_path.parent().unwrap_or(&self.root));
        let options = json.get("compilerOptions");

        // The parent is loaded first: a child inherits, and may add to, it.
        // `extends` is a TOP-LEVEL field, not a compilerOption -- reading it
        // from inside compilerOptions silently drops every config chain.
        if let Some(parent) = json.get("extends").and_then(|v| v.as_str()) {
            if let Some(parent_path) = resolve_extends(&self.root, &dir, parent) {
                self.load_config_file(&parent_path, seen);
            }
        }

        {
            let mut aliases: Vec<(String, String)> = Vec::new();
            if let Some(paths) = options.and_then(|o| o.get("paths")).and_then(|v| v.as_object())
            {
                for (key, value) in paths {
                    let Some(targets) = value.as_array() else {
                        continue;
                    };
                    if let Some(first) = targets.first().and_then(|v| v.as_str()) {
                        aliases.push((key.clone(), first.to_string()));
                    }
                }
            }
            // Longest pattern first so `@/components/*` beats `@/*`.
            aliases.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

            let base = options
                .and_then(|o| o.get("baseUrl"))
                .and_then(|v| v.as_str())
                .map(|b| dir.join(b))
                .unwrap_or_else(|| dir.clone());

            if !aliases.is_empty() {
                // The nearest config for a directory wins, so an inner
                // config overwrites an outer one.
                self.tsconfigs.insert(dir.clone(), TsConfig { base, aliases });
            }
        }
    }

    fn load_tsconfigs(&mut self, dir: &Path, seen: &mut Vec<PathBuf>) {
        if seen.iter().any(|p| p == &dir.join("tsconfig.json")) {
            return;
        }
        self.load_config_file(&dir.join("tsconfig.json"), seen);

        // Subdirectories may hold their own tsconfig (workspaces do).
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || crate::core::parser::SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            self.load_tsconfigs(&path, seen);
        }
    }
}

/// `@fitness/engine/program` is package `@fitness/engine`, subpath
/// `program`. Splitting on the first slash tears the scoped name in half --
/// which silently loses every scoped workspace link.
fn split_package_specifier(specifier: &str) -> (String, Option<String>) {
    let mut parts = specifier.splitn(3, '/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    let third = parts.next();

    if first.starts_with('@') {
        match (second, third) {
            (Some(name), sub) => (format!("{}/{}", first, name), sub.map(|s| s.to_string())),
            // Bare scope with no package name: not resolvable.
            (None, _) => (specifier.to_string(), None),
        }
    } else {
        // Unscoped: `drizzle-orm/postgres-js` -- only the root package can be
        // a workspace link, and only if it is one.
        (first.to_string(), None)
    }
}

fn is_code(ext: &str) -> bool {
    EXTENSIONS.contains(&ext)
}

/// Directories from `dir` up to the repo root, nearest first.
fn ancestors(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut current = Some(dir.to_path_buf());
    while let Some(path) = current {
        out.push(path.clone());
        current = path.parent().map(|p| p.to_path_buf());
    }
    out
}

/// `@/*` matching `components/Button` yields `components/Button`.
fn match_pattern(pattern: &str, value: &str) -> Option<String> {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            if let Some(rest) = value.strip_prefix(prefix) {
                if let Some(rest) = rest.strip_suffix(suffix) {
                    return Some(rest.to_string());
                }
            }
            None
        }
        None if pattern == value => Some(String::new()),
        None => None,
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn resolve_extends(root: &Path, from: &Path, extends: &str) -> Option<PathBuf> {
    let raw = Path::new(extends);
    let mut candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        from.join(raw)
    };
    // `extends: "../../base"` is as legal as `extends: "../../base.json"`.
    if candidate.extension().is_none() {
        candidate.set_extension("json");
    }
    candidate.starts_with(root).then_some(candidate)
}

pub(crate) fn workspace_globs(root: &Path) -> Vec<String> {
    let mut globs = Vec::new();

    if let Ok(body) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        let mut in_packages = false;
        for line in body.lines() {
            let line = line.trim();
            if line.starts_with("packages:") {
                in_packages = true;
                continue;
            }
            if !in_packages {
                continue;
            }
            if let Some(entry) = line.strip_prefix("- ") {
                let entry = entry.trim().trim_matches('"').trim_matches('\'');
                if !entry.is_empty() {
                    globs.push(entry.to_string());
                }
            }
        }
    }

    if globs.is_empty() {
        if let Ok(body) = std::fs::read_to_string(root.join("package.json")) {
            if let Some(json) = parse_json_loose(&body) {
                if let Some(list) = json.get("workspaces").and_then(|v| v.as_array()) {
                    for entry in list.iter().filter_map(|v| v.as_str()) {
                        globs.push(entry.to_string());
                    }
                }
            }
        }
    }

    if globs.is_empty() {
        globs.push("packages/*".to_string());
        globs.push("apps/*".to_string());
    }
    globs
}

/// Minimal glob: one `*` segment. Anything fancier is not worth guessing at.
pub(crate) fn expand_glob_for(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let segments: Vec<&str> = pattern.split('/').collect();
    let Some(star) = segments.iter().position(|s| *s == "*") else {
        let dir = root.join(pattern);
        if dir.is_dir() {
            out.push(dir);
        }
        return out;
    };

    let prefix = root.join(segments[..star].join("/"));
    let Ok(entries) = std::fs::read_dir(if prefix.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        prefix
    }) else {
        return out;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.path().is_dir() {
            continue;
        }
        let rest = &segments[star + 1..];
        let mut path = entry.path();
        let mut ok = true;
        for segment in rest {
            path = path.join(segment);
            if !path.exists() {
                ok = false;
                break;
            }
        }
        if ok && path.is_dir() {
            out.push(path);
        }
    }
    out
}

/// `exports` is either a string, or an object of subpaths, possibly nested
/// under conditions. Collect the string targets; ignore the condition
/// machinery -- high-confidence resolution only.
fn collect_exports(value: &serde_json::Value, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::String(target) => out.push((".".to_string(), target.clone())),
        serde_json::Value::Object(map) => {
            for (key, target) in map {
                if key.starts_with('.') {
                    match target {
                        serde_json::Value::String(s) => out.push((key.clone(), s.clone())),
                        other => collect_exports(other, out),
                    }
                } else if key == "default" || key == "import" || key == "require" {
                    collect_exports(target, out);
                }
            }
        }
        _ => {}
    }
}

/// tsconfig is JSONC: comments and trailing commas are legal and common.
pub fn parse_json_loose(body: &str) -> Option<serde_json::Value> {
    let without_block = strip_jsonc(body);
    serde_json::from_str(&without_block).ok()
}

fn strip_jsonc(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(c),
        }
    }

    // Trailing commas.
    let mut cleaned = String::with_capacity(out.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = out.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            cleaned.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            cleaned.push(c);
            continue;
        }
        if c == ',' {
            let rest: String = chars[i + 1..].iter().collect();
            if rest.trim_start().starts_with('}') || rest.trim_start().starts_with(']') {
                continue;
            }
        }
        cleaned.push(c);
    }
    cleaned
}

/// Repo-relative, forward-slashed, and provably inside the repo. A path that
/// escapes is not a resolution.
fn normalize_repo_path(root: &Path, target: &Path) -> Option<String> {
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    };
    let relative = absolute.strip_prefix(root).ok()?;
    // Same normalizer the DB and the scanner use, so a resolved path and a
    // scanned path are the same string.
    let text = crate::core::paths::normalize_in(&relative.to_string_lossy(), root);
    if text.is_empty() || text.starts_with("..") {
        return None;
    }
    Some(text)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            Self { dir }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn file(&self, rel: &str) -> &Self {
            let path = self.root().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
            self
        }

        fn src(&self, rel: &str, body: &str) -> &Self {
            let path = self.root().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
            self
        }

        fn json(&self, rel: &str, body: &str) -> &Self {
            let path = self.root().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
            self
        }

        fn resolver(&self) -> Resolver {
            Resolver::new(self.root()).expect("resolver")
        }
    }

    fn res(fx: &Fixture, from: &str, spec: &str) -> Option<String> {
        fx.resolver().resolve(from, spec)
    }

    #[test]
    fn relative_to_ts_file() {
        let fx = Fixture::new();
        let fx = fx.file("src/a.ts").file("src/components/b.ts");
        assert_eq!(res(&fx, "src/components/b.ts", "../a"), Some("src/a.ts".into()));
    }

    #[test]
    fn relative_to_tsx_file() {
        let fx = Fixture::new();
        let fx = fx.file("src/a.tsx").file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./a"), Some("src/a.tsx".into()));
    }

    #[test]
    fn relative_directory_with_index() {
        let fx = Fixture::new();
        let fx = fx.file("src/lib/index.ts").file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./lib"), Some("src/lib/index.ts".into()));
    }

    #[test]
    fn relative_js_extension_resolves_to_ts_source() {
        let fx = Fixture::new();
        let fx = fx.file("src/rag/corpus.ts").file("test/corpus.test.ts");
        assert_eq!(
            res(&fx, "test/corpus.test.ts", "../src/rag/corpus.js"),
            Some("src/rag/corpus.ts".into())
        );
    }

    #[test]
    fn relative_explicit_ts_extension() {
        let fx = Fixture::new();
        let fx = fx.file("src/a.ts").file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./a.ts"), Some("src/a.ts".into()));
    }

    #[test]
    fn relative_barrel_re_export_chain() {
        let fx = Fixture::new();
        let fx = fx
            .file("src/lib/index.ts")
            .file("src/lib/db.ts")
            .file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./lib"), Some("src/lib/index.ts".into()));
    }

    #[test]
    fn relative_missing_file_is_unclassified() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./does-not-exist"), None);
    }

    #[test]
    fn relative_escaping_the_repo_is_unclassified() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "../../../etc/passwd"), None);
    }

    #[test]
    fn tsconfig_path_alias() {
        let fx = Fixture::new();
        let fx = fx
            .json(
                "tsconfig.json",
                r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
            )
            .file("src/db/client.ts")
            .file("src/app/page.tsx");
        assert_eq!(
            res(&fx, "src/app/page.tsx", "@/db/client"),
            Some("src/db/client.ts".into())
        );
    }

    #[test]
    fn tsconfig_alias_with_tsx() {
        let fx = Fixture::new();
        let fx = fx
            .json("tsconfig.json", r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#)
            .file("src/components/Button.tsx")
            .file("src/app/page.tsx");
        assert_eq!(
            res(&fx, "src/app/page.tsx", "@/components/Button"),
            Some("src/components/Button.tsx".into())
        );
    }

    #[test]
    fn tsconfig_alias_pointing_at_nothing_is_unclassified() {
        let fx = Fixture::new();
        let fx = fx
            .json("tsconfig.json", r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#)
            .file("src/app/page.tsx");
        assert_eq!(res(&fx, "src/app/page.tsx", "@/db/client"), None);
    }

    #[test]
    fn tsconfig_extends_chain_is_followed() {
        let fx = Fixture::new();
        let fx = fx
            .json(
                "tsconfig.base.json",
                r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
            )
            .json("apps/web/tsconfig.json", r#"{"extends":"../../tsconfig.base.json"}"#)
            .file("src/db/client.ts")
            .file("apps/web/app/page.tsx");
        assert_eq!(
            res(&fx, "apps/web/app/page.tsx", "@/db/client"),
            Some("src/db/client.ts".into())
        );
    }

    #[test]
    fn tsconfig_base_url_relative_import() {
        let fx = Fixture::new();
        let fx = fx
            .json(
                "tsconfig.json",
                r#"{"compilerOptions":{"baseUrl":"src","paths":{"~/*":["lib/*"]}}}"#,
            )
            .file("src/lib/thing.ts")
            .file("src/app.ts");
        assert_eq!(res(&fx, "src/app.ts", "~/thing"), Some("src/lib/thing.ts".into()));
    }

    #[test]
    fn nearest_tsconfig_wins() {
        let fx = Fixture::new();
        let fx = fx
            .json("tsconfig.json", r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#)
            .json(
                "packages/engine/tsconfig.json",
                r#"{"compilerOptions":{"paths":{"@/*":["engine/*"]}}}"#,
            )
            .file("src/thing.ts")
            .file("packages/engine/engine/thing.ts")
            .file("packages/engine/src/x.ts");
        let r = fx.resolver();
        assert_eq!(
            r.resolve("packages/engine/src/x.ts", "@/thing"),
            Some("packages/engine/engine/thing.ts".into())
        );
        assert_eq!(
            r.resolve("src/x.ts", "@/thing"),
            Some("src/thing.ts".into())
        );
    }

    #[test]
    fn workspace_package_by_name() {
        let fx = Fixture::new();
        let fx = fx
            .json("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n")
            .json(
                "packages/engine/package.json",
                r#"{"name":"@fitness/engine","exports":{".":"./src/index.ts","./program":"./src/program.ts"}}"#,
            )
            .file("packages/engine/src/index.ts")
            .file("packages/agent/src/x.ts");
        assert_eq!(
            res(&fx, "packages/agent/src/x.ts", "@fitness/engine"),
            Some("packages/engine/src/index.ts".into())
        );
    }

    #[test]
    fn workspace_package_subpath() {
        let fx = Fixture::new();
        let fx = fx
            .json("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n")
            .json(
                "packages/engine/package.json",
                r#"{"name":"@fitness/engine","exports":{".":"./src/index.ts","./program":"./src/program.ts"}}"#,
            )
            .file("packages/engine/src/index.ts")
            .file("packages/engine/src/program.ts")
            .file("packages/agent/src/x.ts");
        assert_eq!(
            res(&fx, "packages/agent/src/x.ts", "@fitness/engine/program"),
            Some("packages/engine/src/program.ts".into())
        );
    }

    #[test]
    fn workspace_package_without_exports_falls_back_to_main() {
        let fx = Fixture::new();
        let fx = fx
            .json("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n")
            .json(
                "packages/content/package.json",
                r#"{"name":"@fitness/content","main":"src/index.ts"}"#,
            )
            .file("packages/content/src/index.ts")
            .file("apps/web/app/page.tsx");
        assert_eq!(
            res(&fx, "apps/web/app/page.tsx", "@fitness/content"),
            Some("packages/content/src/index.ts".into())
        );
    }

    #[test]
    fn workspace_package_subpath_without_exports_entry() {
        let fx = Fixture::new();
        let fx = fx
            .json("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n")
            .json(
                "packages/content/package.json",
                r#"{"name":"@fitness/content","main":"src/index.ts"}"#,
            )
            .file("packages/content/src/index.ts")
            .file("packages/content/src/repository/program.ts")
            .file("apps/web/app/page.tsx");
        assert_eq!(
            res(&fx, "apps/web/app/page.tsx", "@fitness/content/repository/program"),
            Some("packages/content/src/repository/program.ts".into())
        );
    }

    #[test]
    fn npm_workspace_globs_from_package_json() {
        let fx = Fixture::new();
        let fx = fx
            .json("package.json", r#"{"workspaces":["apps/*","packages/*"]}"#)
            .json(
                "packages/engine/package.json",
                r#"{"name":"@fitness/engine","main":"src/index.ts"}"#,
            )
            .file("packages/engine/src/index.ts")
            .file("apps/web/app/page.tsx");
        assert_eq!(
            res(&fx, "apps/web/app/page.tsx", "@fitness/engine"),
            Some("packages/engine/src/index.ts".into())
        );
    }

    #[test]
    fn third_party_package_is_unclassified() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "react"), None);
        assert_eq!(res(&fx, "src/b.ts", "drizzle-orm/postgres-js"), None);
    }

    #[test]
    fn node_builtin_is_unclassified() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "node:fs/promises"), None);
    }

    #[test]
    fn subpath_import_marker_is_unclassified() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "#internal/thing"), None);
    }

    #[test]
    fn unresolved_is_never_a_guess() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "@/db/client"), None);
        assert_eq!(res(&fx, "src/b.ts", "./deeply/nested/missing"), None);
    }

    #[test]
    fn resolution_returns_the_path_as_written_on_disk() {
        let fx = Fixture::new();
        let fx = fx.file("src/Components/Button.tsx").file("src/app/page.tsx");
        assert_eq!(
            res(&fx, "src/app/page.tsx", "../Components/Button"),
            Some("src/Components/Button.tsx".into())
        );
    }

    #[test]
    fn json_and_style_imports_are_unclassified() {
        let fx = Fixture::new();
        let fx = fx.file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./styles.css"), None);
        assert_eq!(res(&fx, "src/b.ts", "./data.json"), None);
    }

    /// Resolution is only useful if it reaches the index: the boundary engine
    /// queries resolved edges, and the eval measures them. Third-party and
    /// builtin imports must land as NULL -- unclassified, never a violation.
    #[test]
    fn scan_stores_resolved_import_targets() {
        let fx = Fixture::new();
        fx.json(
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        fx.json("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n");
        fx.json(
            "packages/engine/package.json",
            r#"{"name":"@fitness/engine","main":"src/index.ts"}"#,
        );
        fx.src("src/db/client.ts", "export const db = () => null;\n");
        fx.src("packages/engine/src/index.ts", "export const engine = 1;\n");
        fx.src(
            "src/app/page.tsx",
            "import { db } from '@/db/client';\nimport { engine } from '@fitness/engine';\nimport React from 'react';\nexport default function Page() { return null; }\n",
        );

        let db = crate::core::db::Db::open_in_memory().unwrap();
        db.init_schema().unwrap();
        db.insert_module("src", "src", "src/**").unwrap();
        db.insert_module("packages", "packages", "packages/**").unwrap();
        db.insert_rule(db.get_module_id_by_name("src").unwrap().unwrap(), "forbidden", "no db", Some("x")).unwrap();

        crate::core::parser::Parser::new(std::sync::Arc::new(db.clone()))
            .scan(fx.root())
            .unwrap();

        let imports = db.get_imports_by_file("src/app/page.tsx").unwrap();
        let resolved: Vec<(String, Option<String>)> = imports
            .iter()
            .map(|i| (i.imported_from.clone(), i.resolved_to.clone()))
            .collect();

        assert!(
            resolved.contains(&("@/db/client".to_string(), Some("src/db/client.ts".to_string()))),
            "alias import must resolve: {:?}",
            resolved
        );
        assert!(
            resolved.contains(&("@fitness/engine".to_string(), Some("packages/engine/src/index.ts".to_string()))),
            "workspace import must resolve: {:?}",
            resolved
        );
        assert!(
            resolved.contains(&("react".to_string(), None)),
            "third-party import must stay unclassified, never a guess: {:?}",
            resolved
        );
    }

    /// An internals violation looks exactly like this: reaching past a
    /// package's public entry. If it does not resolve, every internals rule
    /// is a silent false negative.
    #[test]
    fn workspace_import_reaching_past_the_entry_resolves_to_the_internals_file() {
        let fx = Fixture::new();
        fx.json("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n");
        fx.json(
            "packages/engine/package.json",
            r#"{"name":"@fitness/engine","exports":{".":"./src/index.ts"}}"#,
        );
        fx.file("packages/engine/src/index.ts");
        fx.file("packages/engine/src/internal/scheduler.ts");
        fx.file("packages/engine/src/load.ts");
        assert_eq!(
            res(&fx, "packages/agent/src/x.ts", "@fitness/engine/src/internal/scheduler"),
            Some("packages/engine/src/internal/scheduler.ts".into())
        );
        assert_eq!(
            res(&fx, "packages/agent/src/x.ts", "@fitness/engine/load"),
            Some("packages/engine/src/load.ts".into()),
            "a subpath that is not in exports must still resolve when the file exists"
        );
    }

    /// The other internals shape: a relative path that crosses a package
    /// boundary.
    #[test]
    fn relative_import_crossing_a_package_boundary_resolves() {
        let fx = Fixture::new();
        fx.file("packages/engine/src/index.ts");
        fx.file("packages/engine/src/internal/scheduler.ts");
        fx.file("packages/agent/src/narrator.ts");
        assert_eq!(
            res(&fx, "packages/agent/src/narrator.ts", "../../engine/src/internal/scheduler"),
            Some("packages/engine/src/internal/scheduler.ts".into())
        );
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_parse() {
        let fx = Fixture::new();
        let fx = fx
            .json(
                "tsconfig.json",
                "// leading comment\n{\n  /* block */\n  \"compilerOptions\": {\n    \"paths\": {\n      \"@/*\": [\"src/*\"],\n    },\n  },\n}\n",
            )
            .file("src/thing.ts")
            .file("src/a.ts");
        assert_eq!(res(&fx, "src/a.ts", "@/thing"), Some("src/thing.ts".into()));
    }
}
