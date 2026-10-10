use crate::core::boundary;
use crate::core::config::Config;
use crate::core::db::{Db, DB_PATH};
use crate::core::parser::{Parser, SKIP_DIRS, VALID_EXTENSIONS};
use std::fs;
use std::io::Write;
use std::path::Path;

const ARCHEX_DIR: &str = ".archex";

/// One module to seed: name, layer, path pattern. Everything downstream takes
/// this and nothing else, so adding a source of modules later is a change to
/// `seeding_plan` alone.
pub(crate) struct ModuleSpec {
    pub(crate) name: String,
    pub(crate) layer: String,
    pub(crate) path_pattern: String,
    pub(crate) rules: Vec<RuleSpec>,
}

pub(crate) struct RuleSpec {
    pub(crate) rule_type: &'static str,
    pub(crate) description: &'static str,
    pub(crate) pattern: Option<&'static str>,
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
            let plan = seeding_plan(&project_root, &project_type);
            eprintln!(
                "No .archex/config.toml yet: seeding {} bootstrap module(s) from {}.",
                plan.len(),
                project_type.label()
            );
            eprintln!(
                "These are a GUESS. Write .archex/config.toml to make the                  architecture yours."
            );
            seed_modules(&db, &plan)
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
        // `next` in the manifest guarantees the src/app route convention, so
        // the curated modules keep their real layers. It guarantees nothing
        // about src/db or src/services: on LORB, 10 of the 16 matched zero
        // files and left 77% of the repo unclassified. So the curated list
        // survives as an overlay on top of the generic plan, not as a
        // replacement for it.
        ProjectType::NextJs => {
            let mut plan = nextjs_plan(root);
            plan.extend(generic_plan(root, &plan));
            plan
        }
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
                rules: Vec::new(),
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

/// Insert whatever the plan says is missing. Idempotent per module and per
/// rule, so `init` twice is a no-op.
pub(crate) fn seed_modules(db: &Db, plan: &[ModuleSpec]) -> usize {
    let mut seeded = 0;
    for spec in plan {
        let module_id = match db.get_module_id_by_name(&spec.name).ok().flatten() {
            Some(id) => id,
            None => {
                db.insert_module(&spec.name, &spec.layer, &spec.path_pattern)
                    .expect("Failed to insert module");
                seeded += 1;
                match db.get_module_id_by_name(&spec.name).ok().flatten() {
                    Some(id) => id,
                    None => continue,
                }
            }
        };

        for rule in &spec.rules {
            let exists = db
                .rule_exists(module_id, rule.rule_type, rule.description, rule.pattern)
                .unwrap_or(false);
            if exists {
                continue;
            }
            db.insert_rule(
                module_id,
                rule.rule_type,
                rule.description,
                rule.pattern,
            )
            .expect("Failed to insert rule");
        }
    }

    let rule_count = db.get_rule_count().expect("Failed to get rule count");
    eprintln!("Seeded {} new modules; {} rules total.", seeded, rule_count);
    seeded
}

/// The curated list is an overlay, not the whole architecture: an entry whose
/// directory holds no indexable file is dropped rather than seeded empty. A
/// module that maps zero files is noise that later reads as "there is an
/// architecture here", and the rules hanging off it can never fire.
fn nextjs_plan(root: &Path) -> Vec<ModuleSpec> {
    let modules: &[(&str, &str, &str)] = &[
        ("app", "ui", "src/app/**"),
        ("components", "ui", "src/components/**"),
        ("api", "api", "src/app/api/**"),
        ("lib", "service", "src/lib/**"),
        ("db", "db", "src/db/**"),
        ("actions", "service", "src/actions/**"),
        ("services", "service", "src/services/**"),
        ("hooks", "ui", "src/hooks/**"),
        ("utils", "service", "src/utils/**"),
        ("types", "types", "src/types/**"),
        ("middleware", "api", "src/middleware/**"),
        ("jobs", "service", "src/jobs/**"),
        ("validations", "service", "src/validations/**"),
        ("models", "db", "src/models/**"),
        ("server", "api", "src/server/**"),
        ("libs", "service", "src/libs/**"),
    ];

    let rules: &[(&str, &[(&str, &str, Option<&str>)])] = &[
        ("app", &[
            ("forbidden", "Direct database access in UI layer", Some("from.*@/db|from.*drizzle")),
            ("forbidden", "Direct API calls without hooks, use src/hooks", Some("fetch\\(|axios\\.")),
        ]),
        ("components", &[
            ("forbidden", "No direct server calls from components", Some("from.*@/server")),
            ("warning", "Avoid business logic in components", Some("function |const .* =")),
        ]),
        ("api", &[
            ("forbidden", "Direct database import in API route", Some("from.*@/db|from.*drizzle")),
            ("forbidden", "Business logic in API route, use services layer", Some("from.*@/services")),
            ("required", "API routes must validate input", Some("zod|yup|joi")),
        ]),
        ("services", &[
            ("required", "Services must use db layer, not direct SQL", Some("from.*@/db|from.*drizzle")),
        ]),
    ];

    modules
        .iter()
        // Existence is checked against the pattern's directory (src/app),
        // not the module name (app).
        .filter(|(_, _, pattern)| has_indexable_file(root, &root.join(pattern.trim_end_matches("/**"))))
        .map(|(name, layer, pattern)| ModuleSpec {
            name: name.to_string(),
            layer: layer.to_string(),
            path_pattern: pattern.to_string(),
            rules: rules
                .iter()
                .find(|(module, _)| module == name)
                .map(|(_, specs)| {
                    specs
                        .iter()
                        .map(|(rule_type, description, pattern)| RuleSpec {
                            rule_type,
                            description,
                            pattern: *pattern,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
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
