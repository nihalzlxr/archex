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
        ("app", "ui", "app/**"),
        ("components", "ui", "components/**"),
        ("api", "api", "app/api/**"),
        ("lib", "service", "lib/**"),
        ("db", "db", "db/**"),
        ("actions", "service", "actions/**"),
    ];

    for (name, layer, pattern) in &modules {
        db.insert_module(name, layer, pattern).expect("Failed to insert module");
    }

    db.insert_rule(3, "forbidden", "Direct database import in API route", Some("from.*drizzle|from.*db/"))
        .expect("Failed to insert rule");
    db.insert_rule(2, "forbidden", "Server-side DB access in component", Some("from.*drizzle"))
        .expect("Failed to insert rule");
    db.insert_rule(3, "required", "All routes must use service layer", None)
        .expect("Failed to insert rule");

    eprintln!("Seeded {} modules with default rules.", modules.len());
    modules.len()
}