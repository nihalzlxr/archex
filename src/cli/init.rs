use crate::core::db::Db;
use crate::core::parser::Parser;
use std::fs;
use std::io::Write;
use std::path::Path;

const ARCHEX_DIR: &str = ".archex";
const DB_PATH: &str = ".archex/db.sqlite";

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

    for (name, layer, pattern) in &modules {
        db.insert_module(name, layer, pattern).expect("Failed to insert module");
    }

    // Get the correct module IDs by name
    let app_id = db.get_module_id_by_name("app").expect("Failed to get app module id");
    let components_id = db.get_module_id_by_name("components").expect("Failed to get components module id");
    let api_id = db.get_module_id_by_name("api").expect("Failed to get api module id");
    let services_id = db.get_module_id_by_name("services").expect("Failed to get services module id");

    // app rules
    if let Some(id) = app_id {
        db.insert_rule(id, "forbidden", "Direct database access in UI layer", Some("from.*@/db|from.*drizzle"))
            .expect("Failed to insert rule");
        db.insert_rule(id, "forbidden", "Direct API calls without hooks, use src/hooks", Some("fetch\\(|axios\\."))
            .expect("Failed to insert rule");
    }

    // components rules
    if let Some(id) = components_id {
        db.insert_rule(id, "forbidden", "No direct server calls from components", Some("from.*@/server"))
            .expect("Failed to insert rule");
        db.insert_rule(id, "warning", "Avoid business logic in components", Some("function |const .* ="))
            .expect("Failed to insert rule");
    }

    // api rules
    if let Some(id) = api_id {
        db.insert_rule(id, "forbidden", "Direct database import in API route", Some("from.*@/db|from.*drizzle"))
            .expect("Failed to insert rule");
        db.insert_rule(id, "forbidden", "Business logic in API route, use services layer", Some("from.*@/services"))
            .expect("Failed to insert rule");
        db.insert_rule(id, "required", "API routes must validate input", Some("zod|yup|joi"))
            .expect("Failed to insert rule");
    }

    // services rules
    if let Some(id) = services_id {
        db.insert_rule(id, "required", "Services must use db layer, not direct SQL", Some("from.*@/db|from.*drizzle"))
            .expect("Failed to insert rule");
    }

    let rule_count = db.get_rule_count().expect("Failed to get rule count");
    eprintln!("Seeded {} modules with {} rules.", modules.len(), rule_count);
    modules.len()
}