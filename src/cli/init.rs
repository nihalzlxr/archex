use crate::core::db::{Db, DB_PATH};
use crate::core::parser::Parser;
use std::fs;
use std::io::Write;
use std::path::Path;

const ARCHEX_DIR: &str = ".archex";

pub fn run() {
    let project_root = std::env::current_dir().expect("Failed to get current directory");

    if Path::new(ARCHEX_DIR).exists() {
        eprint!("Already initialized. Reinitialize? (y/n): ");
        std::io::stderr().flush().unwrap();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).unwrap();
        if !answer.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return;
        }
    }

    fs::create_dir_all(ARCHEX_DIR).expect("Failed to create .archex directory");

    let db = Db::open(Path::new(DB_PATH)).expect("Failed to open database");
    db.init_schema().expect("Failed to initialize schema");

    eprintln!("Detecting project type...");

    let project_type = detect_project_type(&project_root);

    let modules_seeded = match project_type {
        ProjectType::NextJs => {
            eprintln!("Next.js project detected.");
            seed_nextjs_modules(&db)
        }
        ProjectType::Rust => {
            eprintln!("Rust project detected.");
            0
        }
        ProjectType::Go => {
            eprintln!("Go project detected.");
            0
        }
        ProjectType::Unknown => {
            eprintln!("Unknown project type.");
            0
        }
    };

    eprintln!("Scanning files...");
    let parser = Parser::new(db);
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

enum ProjectType {
    NextJs,
    Rust,
    Go,
    Unknown,
}

fn detect_project_type(root: &Path) -> ProjectType {
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

fn seed_nextjs_modules(db: &Db) -> usize {
    let modules = vec![
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

    // Idempotent seeding: never duplicate an existing module.
    let mut seeded = 0;
    for (name, layer, pattern) in &modules {
        if db.get_module_id_by_name(name).ok().flatten().is_some() {
            continue;
        }
        db.insert_module(name, layer, pattern)
            .expect("Failed to insert module");
        seeded += 1;
    }

    // Rules: seed only for modules that exist and have no rules yet,
    // so a re-init can never accumulate duplicate rules.
    let rule_specs: &[(&str, &str, &str, Option<&str>)] = &[
        ("app", "forbidden", "Direct database access in UI layer", Some("from.*@/db|from.*drizzle")),
        ("app", "forbidden", "Direct API calls without hooks, use src/hooks", Some("fetch\\(|axios\\.")),
        ("components", "forbidden", "No direct server calls from components", Some("from.*@/server")),
        ("components", "warning", "Avoid business logic in components", Some("function |const .* =")),
        ("api", "forbidden", "Direct database import in API route", Some("from.*@/db|from.*drizzle")),
        ("api", "forbidden", "Business logic in API route, use services layer", Some("from.*@/services")),
        ("api", "required", "API routes must validate input", Some("zod|yup|joi")),
        ("services", "required", "Services must use db layer, not direct SQL", Some("from.*@/db|from.*drizzle")),
    ];

    for &(module_name, rule_type, description, pattern) in rule_specs {
        let module_id = match db.get_module_id_by_name(module_name).ok().flatten() {
            Some(id) => id,
            None => continue,
        };
        let has_rules = db
            .get_rules_for_module(module_id)
            .map(|r| !r.is_empty())
            .unwrap_or(false);
        if has_rules {
            continue;
        }
        db.insert_rule(module_id, rule_type, description, pattern)
            .expect("Failed to insert rule");
    }

    let rule_count = db.get_rule_count().expect("Failed to get rule count");
    eprintln!("Seeded {} new modules; {} rules total.", seeded, rule_count);
    seeded
}