use crate::core::boundary;
use crate::core::config::Config;
use crate::core::db::{Db, DB_PATH};
use crate::core::parser::{Parser, SKIP_DIRS, VALID_EXTENSIONS};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const ARCHEX_DIR: &str = ".archex";

/// One module to seed: name, layer, path pattern. Everything downstream takes
/// this and nothing else, so adding a source of modules later is a change to
/// `seeding_plan` alone.
pub(crate) struct ModuleSpec {
    pub(crate) name: String,
    pub(crate) layer: String,
    pub(crate) path_pattern: String,
}

pub fn run() {
    let project_root = std::env::current_dir().expect("Failed to get current directory");

    // Prompt only when there is an INDEX to overwrite. Once config.toml is
    // committed, `.archex/` always exists -- so prompting on the directory
    // would make `init` unusable in exactly the repos that adopted it.
    let index_exists = Path::new(DB_PATH).exists();
    if index_exists {
        eprint!("An index already exists. Rebuild it? (y/n): ");
        std::io::stderr().flush().unwrap();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).unwrap();
        if !answer.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return;
        }
    }

    fs::create_dir_all(ARCHEX_DIR).expect("Failed to create .archex directory");

    let db = Db::shared(Path::new(DB_PATH)).expect("Failed to open database");
    db.init_schema().expect("Failed to initialize schema");

    eprintln!("Detecting project type...");

    let project_type = detect_project_type(&project_root);

    // A committed config is the architecture. It REPLACES the inferred
    // bootstrap default rather than merging with it -- merging makes a module
    // impossible to delete.
    let modules_seeded = match Config::load(&project_root) {
        Ok(config) => {
            eprintln!(
                "Using committed .archex/config.toml ({} modules).",
                config.modules.len()
            );
            match boundary::install_modules(&db, &config) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("Failed to install modules: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(_) => {
            // Adoption path: hand the human a starting point rather than a
            // silent guess, then immediately index what was drafted so the
            // first `archex check` already reflects their repo.
            match write_draft_config(&project_root) {
                Ok(DraftOutcome::Written(path)) => {
                    eprintln!("Drafted {}", path.display());
                    eprintln!("Read it, edit it, commit it: it is your architecture now.");
                }
                Ok(DraftOutcome::Skipped { .. }) => {}
                Err(e) => eprintln!("Could not draft a config ({}); falling back to inference.", e),
            }
            match Config::load(&project_root) {
                Ok(config) => {
                    eprintln!("Using the drafted config ({} modules).", config.modules.len());
                    match boundary::install_modules(&db, &config) {
                        Ok(n) => n,
                        Err(e) => {
                            eprintln!("Failed to install modules: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(_) => {
                    let plan = seeding_plan(&project_root, &project_type);
                    eprintln!(
                        "Seeding {} inferred module(s) from {}.",
                        plan.len(),
                        project_type.label()
                    );
                    seed_modules(&db, &plan)
                }
            }
        }
    };

    eprintln!("Scanning files...");
    let parser = Parser::new(db.clone());
    let result = parser.scan(&project_root).expect("Failed to scan");

    eprintln!();
    eprintln!("=== Summary ===");
    eprintln!("Modules seeded: {}", modules_seeded);
    eprintln!("Files scanned: {}", result.files_scanned);
    eprintln!("Files mapped: {}", result.files_mapped);
    eprintln!("Files unmapped: {}", result.files_unmapped.len());
    eprintln!("Symbols extracted: {}", result.symbols_extracted);
    eprintln!("Imports extracted: {}", result.imports_extracted);

    if !result.files_unmapped.is_empty() {
        eprintln!("Unmapped files:");
        for f in result.files_unmapped.iter().take(10) {
            eprintln!("  - {}", f.display());
        }
        if result.files_unmapped.len() > 10 {
            eprintln!("  ... and {} more", result.files_unmapped.len() - 10);
        }
    }
}

pub(crate) enum ProjectType {
    NextJs,
    Rust,
    Go,
    Unknown,
}

impl ProjectType {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ProjectType::NextJs => "a Next.js manifest",
            ProjectType::Rust => "a Rust manifest",
            ProjectType::Go => "a Go manifest",
            ProjectType::Unknown => "directory layout",
        }
    }
}

/// The module list to seed. This is the ONE place that decides what a module
/// is, and it is a bootstrap default, not the source of truth.
///
/// Step 3: `.archex/config.toml` becomes authoritative — when it exists,
/// read modules from it and stop inferring. That change lands here and in
/// `seed_modules`; the scanner, the index and the rule engine already take a
/// list of modules and will not care where it came from.
pub(crate) fn seeding_plan(root: &Path, project_type: &ProjectType) -> Vec<ModuleSpec> {
    match project_type {
        // `next` in the manifest used to buy a curated 16-module list with
        // real layers. It does not any more: on LORB, 10 of those 16 matched
        // ZERO files and left 77% of the repo unclassified, and on a plain
        // Node repo it never fired at all. A hardcoded guess about one
        // codebase's layout is not knowledge a manifest can vouch for, so it
        // is gone. config.toml is where those layers are named -- by a human.
        ProjectType::NextJs => generic_plan(root, &[]),
        // No framework convention to lean on. Take the directory names as
        // they are: a module per top-level directory, layer = the name, no
        // rules. archex does not know that `lib` is a service layer, and in
        // Step 2 a wrong layer is not wrong context, it is a false violation
        // from a rule that checks `from_layer`/`to_layer`.
        _ => generic_plan(root, &[]),
    }
}

/// One module per top-level directory that holds indexable files the curated
/// overlay does not already claim. A directory holding only docs or assets
/// gets no module, and neither does one the overlay covers completely --
/// otherwise every Next.js repo grows a second, empty `src` module.
fn generic_plan(root: &Path, curated: &[ModuleSpec]) -> Vec<ModuleSpec> {
    let overlay = compile_patterns(curated);
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut specs: Vec<ModuleSpec> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|entry| !is_skipped_dir(&entry.path()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter(|entry| has_indexable_file(root, &entry.path()))
        .filter(|entry| {
            // Only worth a module if the overlay leaves something behind.
            indexable_files(root, &entry.path())
                .any(|file| !overlay.iter().any(|p| p.matches(&file)))
        })
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            ModuleSpec {
                path_pattern: format!("{}/**", name),
                layer: name.clone(),
                name,
            }
        })
        .collect();

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

fn compile_patterns(specs: &[ModuleSpec]) -> Vec<glob::Pattern> {
    specs
        .iter()
        .filter_map(|spec| glob::Pattern::new(&spec.path_pattern).ok())
        .collect()
}

/// Reuse the scanner's skip list instead of keeping a second one that drifts.
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .map(|n| SKIP_DIRS.contains(&n.to_string_lossy().as_ref()))
        .unwrap_or(false)
}

fn is_indexable(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| VALID_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

/// Every indexable file under `dir`, as a repo-relative forward-slashed path
/// (the form both the scanner and the overlay patterns use), with the same
/// skip list the scanner uses.
fn indexable_files<'a>(root: &'a Path, dir: &'a Path) -> impl Iterator<Item = String> + 'a {
    let prefix = root.to_string_lossy().trim_end_matches('/').to_string();
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_skipped_dir(e.path()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_indexable(e.path()))
        .map(move |e| {
            let path = e.path().to_string_lossy().replace('\\', "/");
            path.strip_prefix(&prefix)
                .map(|rest| rest.trim_start_matches('/').to_string())
                .unwrap_or(path)
        })
}

fn has_indexable_file(root: &Path, dir: &Path) -> bool {
    indexable_files(root, dir).next().is_some()
}

/// Insert whatever the plan says is missing. Idempotent per module, so `init`
/// twice is a no-op. Rules are never seeded: they are human-authored files in
/// `.archex/rules/`, and a rule archex invented for your repo is a rule you
/// did not choose.
pub(crate) fn seed_modules(db: &Db, plan: &[ModuleSpec]) -> usize {
    let mut seeded = 0;
    for spec in plan {
        if db.get_module_id_by_name(&spec.name).ok().flatten().is_some() {
            continue;
        }
        db.insert_module(&spec.name, &spec.layer, &spec.path_pattern)
            .expect("Failed to insert module");
        seeded += 1;
    }

    eprintln!("Seeded {} new modules.", seeded);
    seeded
}



pub(crate) fn detect_project_type(root: &Path) -> ProjectType {
    if root.join("package.json").exists() {
        if let Ok(content) = fs::read_to_string(root.join("package.json")) {
            if content.contains("\"next\"") {
                return ProjectType::NextJs;
            }
        }
    }
    if root.join("Cargo.toml").exists() {
        return ProjectType::Rust;
    }
    if root.join("go.mod").exists() {
        return ProjectType::Go;
    }
    ProjectType::Unknown
}


#[derive(Debug)]
pub enum DraftOutcome {
    Written(PathBuf),
    /// A config was already there. Ours or theirs, we do not touch it.
    Skipped { #[allow(dead_code)] path: PathBuf },
}

/// Read a draft `config.toml` from the workspace layout: one module per
/// workspace package, one layer per module, for a human to edit.
///
/// This is the adoption path, and it is deliberately NOT the generic
/// top-level inference: that produces container modules (`apps`, `packages`)
/// on a monorepo, which are not architecture. A workspace package IS a
/// module someone already decided on.
pub fn draft_config(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut modules: Vec<(String, String)> = Vec::new();

    for pattern in crate::core::resolve::workspace_globs(root) {
        for dir in expand_glob_pub(root, &pattern) {
            let manifest = dir.join("package.json");
            let Ok(body) = fs::read_to_string(&manifest) else {
                continue;
            };
            let Some(pkg) = package_name(&body) else {
                continue;
            };
            let path = crate::core::paths::normalize_in(&dir.to_string_lossy(), root);
            modules.push((short_name(&pkg, &modules), format!("{}/**", path)));
        }
    }

    if modules.is_empty() {
        // Not a workspace: fall back to top-level code directories, so the
        // draft is still worth reading and editing.
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.filter_map(|e| e.ok()) {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || is_skipped_dir(&entry.path()) {
                    continue;
                }
                if has_indexable_file(root, &entry.path()) {
                    let path = format!("{}/**", name);
                    modules.push((name, path));
                }
            }
        }
    }

    modules.sort();
    modules.dedup();

    let mut out = String::new();
    out.push_str("# archex configuration
");
    out.push_str("#
");
    out.push_str("# LAYERS AND MODULE NAMES BELOW ARE A DRAFT. Read them, change
");
    out.push_str("# what is wrong, delete what does not exist. A layer is just a key you
");
    out.push_str("# chose -- archex cannot tell you that `engine` is a domain layer, so
");
    out.push_str("# it will not pretend to.
");
    out.push_str("#
");
    out.push_str("# Every file that belongs to no module is reported as UNCLASSIFIED by
");
    out.push_str("# `archex check`. Uncovered is not the same as clean.

");
    out.push_str("schema_version = 1\n");
    out.push_str("name = \"");
    out.push_str(
        &root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string()),
    );
    out.push_str("\"\n\n");

    let layers: Vec<String> = modules
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    out.push_str("# Layers. Rename these to mean something: ui, service, engine...
");
    out.push_str("layers = [");
    out.push_str(
        &layers
            .iter()
            .map(|l| format!("\"{}\"", l))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("]

");

    out.push_str("# Files matching these are skipped by boundary rules unless a rule sets
");
    out.push_str("# include_tests = true. A rule that flags every test file is a rule that
");
    out.push_str("# gets disabled on day one.
");
    out.push_str("test_globs = [");
    out.push_str(
        &crate::core::config::DEFAULT_TEST_GLOBS
            .iter()
            .map(|g| format!("\"{}\"", g))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("]
");

    for (name, path) in &modules {
        out.push_str("
[[module]]
");
        out.push_str(&format!("name = \"{}\"\n", name));
        out.push_str(&format!("layer = \"{}\"\n", name));
        out.push_str(&format!("path = \"{}\"\n", path));
    }

    Ok(out)
}

pub fn write_draft_config(root: &Path) -> Result<DraftOutcome, Box<dyn std::error::Error>> {
    let path = root.join(".archex/config.toml");
    if path.exists() {
        return Ok(DraftOutcome::Skipped { path });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, draft_config(root)?)?;
    Ok(DraftOutcome::Written(path))
}

/// `@fitness/engine` -> `engine`. A scope is a publishing namespace, not an
/// architecture layer, and a slash in a layer name would collide with every
/// layer-or-module rule. If two packages reduce to the same short name the
/// later one keeps its full name: a draft that silently drops a package is
/// worse than an ugly name a human is about to edit anyway.
fn short_name(pkg: &str, taken: &[(String, String)]) -> String {
    let short = pkg.rsplit('/').next().unwrap_or(pkg).to_string();
    if taken.iter().any(|(name, _)| *name == short) {
        return pkg.to_string();
    }
    short
}

fn package_name(body: &str) -> Option<String> {
    let json = crate::core::resolve::parse_json_loose(body)?;
    json.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn expand_glob_pub(root: &Path, pattern: &str) -> Vec<PathBuf> {
    crate::core::resolve::expand_glob_for(root, pattern)
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    fn workspace(root: &Path) {
        std::fs::create_dir_all(root.join("packages/engine/src")).unwrap();
        std::fs::create_dir_all(root.join("apps/web/app")).unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n  - \"apps/*\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("packages/engine/package.json"),
            "{\"name\":\"@fitness/engine\"}",
        )
        .unwrap();
        std::fs::write(root.join("apps/web/package.json"), "{\"name\":\"web\"}").unwrap();
    }

    #[test]
    fn drafts_one_module_per_workspace_package() {
        let dir = tempfile::tempdir().unwrap();
        workspace(dir.path());
        let draft = draft_config(dir.path()).expect("draft");
        let config = Config::parse_str(&draft).unwrap_or_else(|e| panic!("draft did not parse: {}\n---\n{}", e, draft));

        assert_eq!(config.modules.len(), 2);
        let engine = config.modules.iter().find(|m| m.name == "engine").unwrap();
        assert_eq!(engine.path, "packages/engine/**");
        assert_eq!(config.layers.len(), 2, "one layer per module, pre-filled for editing");
    }

    /// The whole point: a human assigns layers, archex does not guess them.
    #[test]
    fn the_draft_leaves_layers_to_be_edited() {
        let dir = tempfile::tempdir().unwrap();
        workspace(dir.path());
        let draft = draft_config(dir.path()).unwrap();
        assert!(
            draft.contains("# LAYERS AND MODULE NAMES BELOW ARE A DRAFT"),
            "the file must say out loud that it is a draft"
        );
        assert!(draft.contains("test_globs"), "and carry the safe test defaults");
    }

    #[test]
    fn an_existing_config_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        workspace(dir.path());
        let path = dir.path().join(".archex/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mine\nschema_version = 1\n").unwrap();

        let outcome = write_draft_config(dir.path()).expect("handled");
        assert!(matches!(outcome, DraftOutcome::Skipped { .. }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\nschema_version = 1\n");
    }

    #[test]
    fn a_flat_repo_still_gets_a_valid_draft() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.ts"), "export const a = 1;\n").unwrap();
        let draft = draft_config(dir.path()).expect("draft");
        let config = Config::parse_str(&draft).expect("must parse");
        assert_eq!(config.modules.len(), 1);
        assert_eq!(config.modules[0].path, "src/**");
    }

    #[test]
    fn a_repo_with_nothing_in_it_still_gets_a_parseable_draft() {
        let dir = tempfile::tempdir().unwrap();
        let draft = draft_config(dir.path()).expect("draft");
        Config::parse_str(&draft).expect("an empty draft must still be valid");
    }

    #[test]
    fn npm_workspaces_are_read_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("libs/core")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "{\"workspaces\":[\"libs/*\"]}",
        )
        .unwrap();
        std::fs::write(dir.path().join("libs/core/package.json"), "{\"name\":\"core\"}").unwrap();
        let draft = draft_config(dir.path()).unwrap();
        assert!(draft.contains("libs/core/**"), "{}", draft);
    }
}
