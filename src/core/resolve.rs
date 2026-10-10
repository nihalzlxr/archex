//! File-level import resolution.
//!
//! This is the gate's foundation: a boundary rule is an edge query over
//! RESOLVED imports, so an unresolved import is not a small problem, it is
//! the difference between a real violation and a false accusation.
//!
//! The rule the whole design follows: **unresolved means unclassified, never
//! a violation.** A resolver that guesses produces confident nonsense. Every
//! ambiguous case below returns `None`.

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

    // --- relative: the case that must never break ---

    #[test]
    fn relative_to_ts_file() {
        let fx = Fixture::new().file("src/a.ts").file("src/components/b.ts");
        assert_eq!(res(&fx, "src/components/b.ts", "../a"), Some("src/a.ts".into()));
    }

    #[test]
    fn relative_to_tsx_file() {
        let fx = Fixture::new().file("src/a.tsx").file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./a"), Some("src/a.tsx".into()));
    }

    #[test]
    fn relative_directory_with_index() {
        let fx = Fixture::new().file("src/lib/index.ts").file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./lib"), Some("src/lib/index.ts".into()));
    }

    /// The dominant real-world shape in fitness: NodeNext ESM imports that
    /// carry a `.js` extension for a TypeScript source.
    #[test]
    fn relative_js_extension_resolves_to_ts_source() {
        let fx = Fixture::new().file("src/rag/corpus.ts").file("test/corpus.test.ts");
        assert_eq!(
            res(&fx, "test/corpus.test.ts", "../src/rag/corpus.js"),
            Some("src/rag/corpus.ts".into())
        );
    }

    #[test]
    fn relative_explicit_ts_extension() {
        let fx = Fixture::new().file("src/a.ts").file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./a.ts"), Some("src/a.ts".into()));
    }

    #[test]
    fn relative_barrel_re_export_chain() {
        let fx = Fixture::new()
            .file("src/lib/index.ts")
            .file("src/lib/db.ts")
            .file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./lib"), Some("src/lib/index.ts".into()));
    }

    #[test]
    fn relative_missing_file_is_unclassified() {
        let fx = Fixture::new().file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./does-not-exist"), None);
    }

    #[test]
    fn relative_escaping_the_repo_is_unclassified() {
        let fx = Fixture::new().file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "../../../etc/passwd"), None);
    }

    // --- tsconfig aliases: the LORB shape, and the plan's named risk ---

    #[test]
    fn tsconfig_path_alias() {
        let fx = Fixture::new()
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
        let fx = Fixture::new()
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
        let fx = Fixture::new()
            .json("tsconfig.json", r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#)
            .file("src/app/page.tsx");
        assert_eq!(res(&fx, "src/app/page.tsx", "@/db/client"), None);
    }

    #[test]
    fn tsconfig_extends_chain_is_followed() {
        let fx = Fixture::new()
            .json(
                "tsconfig.base.json",
                r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
            )
            .json("apps/web/tsconfig.json", r#"{"extends":"../../tsconfig.base.json"}"#)
            .file("src/db/client.ts")
            .file("apps/web/app/page.tsx");
        // Paths are resolved relative to the file that declares them.
        assert_eq!(
            res(&fx, "apps/web/app/page.tsx", "@/db/client"),
            Some("src/db/client.ts".into())
        );
    }

    #[test]
    fn tsconfig_base_url_relative_import() {
        let fx = Fixture::new()
            .json(
                "tsconfig.json",
                r#"{"compilerOptions":{"baseUrl":"src","paths":{"~/*":["lib/*"]}}}"#,
            )
            .file("src/lib/thing.ts")
            .file("src/app.ts");
        assert_eq!(res(&fx, "src/app.ts", "~/thing"), Some("src/lib/thing.ts".into()));
        assert_eq!(
            res(&fx, "src/app.ts", "config"),
            res(&fx, "src/app.ts", "config")
        );
    }

    #[test]
    fn nearest_tsconfig_wins() {
        let fx = Fixture::new()
            .json("tsconfig.json", r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#)
            .json(
                "packages/engine/tsconfig.json",
                r#"{"compilerOptions":{"paths":{"@/*":["engine/*"]}}}"#,
            )
            .file("src/shared/thing.ts")
            .file("packages/engine/engine/thing.ts")
            .file("packages/engine/src/x.ts");
        // The engine package's own alias must not be shadowed by the root one.
        let r = fx.resolver();
        assert_eq!(
            r.resolve("packages/engine/src/x.ts", "@/thing"),
            Some("packages/engine/engine/thing.ts".into())
        );
        assert_eq!(
            r.resolve("src/x.ts", "@/thing"),
            Some("src/shared/thing.ts".into())
        );
    }

    // --- workspace packages: the fitness shape ---

    #[test]
    fn workspace_package_by_name() {
        let fx = Fixture::new()
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
        let fx = Fixture::new()
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
        let fx = Fixture::new()
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
        let fx = Fixture::new()
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
        let fx = Fixture::new()
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

    // --- everything that must stay unclassified ---

    #[test]
    fn third_party_package_is_unclassified() {
        let fx = Fixture::new().file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "react"), None);
        assert_eq!(res(&fx, "src/b.ts", "drizzle-orm/postgres-js"), None);
    }

    #[test]
    fn node_builtin_is_unclassified() {
        let fx = Fixture::new().file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "node:fs/promises"), None);
    }

    #[test]
    fn subpath_import_marker_is_unclassified() {
        let fx = Fixture::new().file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "#internal/thing"), None);
    }

    #[test]
    fn unresolved_is_never_a_guess() {
        let fx = Fixture::new().file("src/b.ts");
        // Nothing exists: the answer must be "unclassified", not the nearest
        // file that happens to share a suffix.
        assert_eq!(res(&fx, "src/b.ts", "@/db/client"), None);
        assert_eq!(res(&fx, "src/b.ts", "./deeply/nested/missing"), None);
    }

    #[test]
    fn resolution_is_case_sensitive_on_disk_but_returns_normalized_paths() {
        let fx = Fixture::new().file("src/Components/Button.tsx").file("src/app/page.tsx");
        assert_eq!(
            res(&fx, "src/app/page.tsx", "../Components/Button"),
            Some("src/Components/Button.tsx".into())
        );
    }

    #[test]
    fn json_and_style_imports_are_unclassified() {
        let fx = Fixture::new().file("src/b.ts");
        assert_eq!(res(&fx, "src/b.ts", "./styles.css"), None);
        assert_eq!(res(&fx, "src/b.ts", "./data.json"), None);
    }
}