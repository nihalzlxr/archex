use rusqlite::{Connection, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use serde::Serialize;

use crate::core::paths;

pub const DB_PATH: &str = ".archex/archex.db";

/// Bump when the shape of the index changes. On mismatch the cache is dropped
/// and rebuilt from code -- it is derived, never the source of truth, so a
/// migration would be code that only ever runs once, on machines nobody has.
pub const SCHEMA_VERSION: i64 = 2;

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

/// Every table the index owns. Dropped wholesale on a version mismatch.
const OWNED_TABLES: &[&str] = &[
    "symbols",
    "imports",
    "file_map",
    "rules",
    "modules",
    "decisions",
];

/// One SQLite connection per DB file, behind a mutex. WAL lets readers run
/// while a writer holds the write lock, so a single shared connection is both
/// faster and safer than the five the MCP server used to open per process.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    /// Repo root this DB belongs to: every file path is normalized against it,
    /// so an absolute path from an agent resolves to the same row a scan wrote.
    root: PathBuf,
}

static SHARED: OnceLock<Mutex<HashMap<PathBuf, Arc<Db>>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub id: i64,
    pub rule_type: RuleType,
    pub description: String,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum RuleType {
    Forbidden,
    Required,
    Warning,
}

impl From<String> for RuleType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "forbidden" => RuleType::Forbidden,
            "required" => RuleType::Required,
            "warning" => RuleType::Warning,
            _ => RuleType::Warning,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Module {
    pub id: i64,
    pub name: String,
    pub layer: String,
    pub path_pattern: String,
}

#[derive(Debug, Serialize)]
pub struct ModuleInfo {
    pub name: String,
    pub layer: String,
    pub path_pattern: String,
    pub file_count: i64,
    pub files: Vec<String>,
    pub rules: Vec<Rule>,
}

/// One import, reduced to what a boundary rule needs to judge it.
#[derive(Debug, Clone)]
pub struct ImportEdge {
    pub imported_from: String,
    pub resolved_to: Option<String>,
    pub line_number: i64,
}

#[derive(Debug, Serialize)]
pub struct FileContext {
    pub module_name: String,
    pub layer: String,
    pub rules: Vec<Rule>,
}

impl Db {
    /// Borrow the connection. A poisoned lock means some other thread panicked
    /// mid-statement; the data is still consistent (SQLite rolled back), so
    /// recover rather than take the whole MCP server down with it.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn configure(conn: &Connection) -> Result<()> {
        // busy_timeout must come first: it governs how long a writer waits for
        // a lock instead of failing immediately.
        conn.pragma_update(None, "busy_timeout", 5000i64)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // The schema declares FOREIGN KEYs; SQLite ignores them unless asked.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(())
    }

    /// The repo root a DB belongs to: two levels up from `<root>/.archex/x.db`.
    fn root_for(path: &Path) -> PathBuf {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        absolute
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(cwd)
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let conn = Connection::open(path)?;
        Self::configure(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            root: Self::root_for(path),
        })
    }

    /// The process-wide connection for `path`. Five MCP tools each opened their
    /// own; they now share one, which is what makes WAL worth having.
    pub fn shared(path: &Path) -> Result<Arc<Self>> {
        let key = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
        let registry = SHARED.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = map.get(&key) {
            return Ok(existing.clone());
        }
        let db = Arc::new(Self::open(path)?);
        map.insert(key, db.clone());
        Ok(db)
    }

    /// Repo-relative canonical form of `file_path`, identical to what every
    /// writer stores — so an agent's absolute path finds the row a scan wrote.
    fn normalize(&self, file_path: &str) -> String {
        paths::normalize_in(file_path, &self.root)
    }

    /// SQL predicate matching a stored path. On a case-insensitive filesystem
    /// (`SRC/A.ts` and `src/a.ts` are one file there) an exact `=` would report
    /// "no context" for a file that is indexed. On a case-sensitive one the two
    /// really are different files, so the fallback stays off.
    fn path_match(column: &str) -> String {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            format!("LOWER({}) = LOWER(?1)", column)
        } else {
            format!("{} = ?1", column)
        }
    }

    pub fn init_schema(&self) -> Result<()> {
        let conn = self.lock();
        self.rebuild_if_stale(&conn)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS modules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                layer TEXT NOT NULL,
                path_pattern TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                module_id INTEGER NOT NULL,
                rule_type TEXT NOT NULL CHECK(rule_type IN ('forbidden', 'required', 'warning')),
                description TEXT NOT NULL,
                pattern TEXT,
                FOREIGN KEY(module_id) REFERENCES modules(id)
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_map (
                file_path TEXT PRIMARY KEY,
                module_id INTEGER NOT NULL,
                last_parsed INTEGER NOT NULL,
                FOREIGN KEY(module_id) REFERENCES modules(id)
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS decisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                context TEXT,
                decision TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        Self::create_symbols_table(&conn)?;
        Self::create_imports_table(&conn)?;

        Ok(())
    }

    /// Drop everything if the on-disk version is not ours. A fresh database
    /// has no meta table at all, which is not a mismatch -- there is nothing
    /// to drop.
    fn rebuild_if_stale(&self, conn: &Connection) -> Result<()> {
        let recorded: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .ok();

        // No meta table but archex tables present means a database written
        // before versioning existed: its shape is whatever it was, so treat
        // it as stale. Neither table means a fresh database, nothing to drop.
        let pre_versioning = recorded.is_none() && table_exists(conn, "modules");

        let stale = pre_versioning
            || matches!(&recorded, Some(value) if value.parse::<i64>().ok() != Some(SCHEMA_VERSION));

        if stale {
            for table in OWNED_TABLES {
                conn.execute(&format!("DROP TABLE IF EXISTS {}", table), [])?;
            }
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
            [SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn create_symbols_table(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS symbols (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                symbol_type TEXT NOT NULL,
                name TEXT NOT NULL,
                signature TEXT,
                line_number INTEGER NOT NULL,
                module_id INTEGER NOT NULL,
                exported INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(module_id) REFERENCES modules(id)
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_module ON symbols(module_id)",
            [],
        )?;

        Ok(())
    }

    pub fn create_imports_table(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS imports (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                imported_from TEXT NOT NULL,
                imported_names TEXT NOT NULL,
                module_id INTEGER NOT NULL,
                resolved_to TEXT,
                line_number INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(module_id) REFERENCES modules(id)
            )",
            [],
        )?;

        // The DB is a derived cache, but a stale one should still open.
        let _ = conn.execute("ALTER TABLE imports ADD COLUMN resolved_to TEXT", []);

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_imports_file ON imports(file_path)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_imports_module ON imports(module_id)",
            [],
        )?;

        Ok(())
    }

    pub fn insert_module(&self, name: &str, layer: &str, path_pattern: &str) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO modules (name, layer, path_pattern) VALUES (?1, ?2, ?3)",
            [name, layer, path_pattern],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Returns the new rule's id: callers print it, and ids are NOT the row
    /// count once anything has been deleted.
    pub fn insert_rule(&self, module_id: i64, rule_type: &str, description: &str, pattern: Option<&str>) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO rules (module_id, rule_type, description, pattern) VALUES (?1, ?2, ?3, ?4)",
            (module_id, rule_type, description, pattern),
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_all_modules(&self) -> Result<Vec<Module>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT id, name, layer, path_pattern FROM modules")?;
        let modules = stmt.query_map([], |row| {
            Ok(Module {
                id: row.get(0)?,
                name: row.get(1)?,
                layer: row.get(2)?,
                path_pattern: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        Ok(modules)
    }

    pub fn upsert_file(&self, file_path: &str, module_id: i64) -> Result<()> {
        let conn = self.lock();
        let file_path = self.normalize(file_path);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT OR REPLACE INTO file_map (file_path, module_id, last_parsed) VALUES (?1, ?2, ?3)",
            (&file_path, module_id, now),
        )?;
        Ok(())
    }

    pub fn get_context_for_file(&self, file_path: &str) -> Result<Option<FileContext>> {
        let conn = self.lock();
        let file_path = self.normalize(file_path);
        let sql = "SELECT m.name, m.layer, r.rule_type, r.description, r.pattern
             FROM file_map fm
             JOIN modules m ON fm.module_id = m.id
             LEFT JOIN rules r ON r.module_id = m.id
             WHERE ";
        let mut stmt = conn.prepare(&format!(
            "{}{}",
            sql,
            Self::path_match("fm.file_path")
        ))?;

        let mut rows = stmt.query([&file_path])?;

        let mut module_name: Option<String> = None;
        let mut layer: Option<String> = None;
        let mut rules: Vec<Rule> = Vec::new();

        while let Some(row) = rows.next()? {
            if module_name.is_none() {
                module_name = Some(row.get(0)?);
                layer = Some(row.get(1)?);
            }
            
            let rule_type: Option<String> = row.get(2).ok();
            let description: Option<String> = row.get(3).ok();
            let pattern: Option<String> = row.get(4).ok();

            if let (Some(rt), Some(desc)) = (rule_type, description) {
                rules.push(Rule {
                    id: rules.len() as i64 + 1,
                    rule_type: RuleType::from(rt),
                    description: desc,
                    pattern,
                });
            }
        }

        let (module_name, layer) = match (module_name, layer) {
            (Some(name), Some(layer)) => (name, layer),
            _ => return Ok(None),
        };

        Ok(Some(FileContext {
            module_name,
            layer,
            rules,
        }))
    }

    /// The module a file belongs to, or None when it is unclassified.
    pub fn module_of_file(&self, file_path: &str) -> Option<String> {
        self.get_module_for_file(file_path)
            .ok()
            .flatten()
            .map(|(name, _)| name)
    }

    /// Resolved edges out of one file: target plus the line to jump to.
    pub fn imports_of_file(&self, file_path: &str) -> Result<Vec<ImportEdge>> {
        Ok(self
            .get_imports_by_file(file_path)?
            .into_iter()
            .map(|i| ImportEdge {
                imported_from: i.imported_from,
                resolved_to: i.resolved_to,
                line_number: i.line_number,
            })
            .collect())
    }

    /// How many indexed files belong to a module. Used to tell "this layer
    /// is empty" (probably a typo) from "these layers simply have no edges
    /// between them" (a clean result).
    pub fn files_in_module(&self, module_name: &str) -> Result<usize> {
        let Some(id) = self.get_module_id_by_name(module_name)? else {
            return Ok(0);
        };
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM file_map WHERE module_id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    /// Scanned-but-unmapped files are invisible in file_map by construction,
    /// so the scan records the counters where `check` can see them. A file in
    /// no module is not "clean", it is uncovered, and the output says so.
    pub fn set_scan_stats(&self, scanned: usize, mapped: usize) -> Result<()> {
        let conn = self.lock();
        conn.execute("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)", [])?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('files_scanned', ?1)",
            [scanned.to_string()],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('files_mapped', ?1)",
            [mapped.to_string()],
        )?;
        Ok(())
    }

    pub fn scan_stats(&self) -> (usize, usize) {
        let conn = self.lock();
        let read = |key: &str| -> usize {
            conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
        };
        (read("files_scanned"), read("files_mapped"))
    }

    /// Modules come from the committed config when it exists, so seeding
    /// output can be replaced outright -- otherwise a module could never be
    /// deleted.
    /// Replacing the module set invalidates every row that points at one.
    /// With foreign keys actually ENABLED (they are, since WAL), deleting
    /// modules out from under their rows is a constraint violation, not a
    /// silent success.
    pub fn clear_modules(&self) -> Result<()> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        for table in ["file_map", "rules", "symbols", "imports", "modules"] {
            tx.execute(&format!("DELETE FROM {}", table), [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_module_for_file(&self, file_path: &str) -> Result<Option<(String, i64)>> {
        let conn = self.lock();
        let file_path = self.normalize(file_path);
        let result: Result<(String, i64), _> = conn.query_row(
            "SELECT m.name, fm.module_id FROM file_map fm JOIN modules m ON fm.module_id = m.id WHERE fm.file_path = ?1",
            [&file_path],
            |row| Ok((row.get(0)?, row.get(1)?))
        );
        
        match result {
            Ok((name, id)) => Ok(Some((name, id))),
            Err(_) => Ok(None),
        }
    }

    pub fn get_rules_for_module(&self, module_id: i64) -> Result<Vec<Rule>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, rule_type, description, pattern FROM rules WHERE module_id = ?1"
        )?;
        
        let rules = stmt.query_map([module_id], |row| {
            Ok(Rule {
                id: row.get(0)?,
                rule_type: RuleType::from(row.get::<_, String>(1)?),
                description: row.get(2)?,
                pattern: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        
        Ok(rules)
    }

    pub fn get_rule_count(&self) -> Result<i64> {
        let conn = self.lock();
        conn.query_row("SELECT COUNT(*) FROM rules", [], |row| row.get(0))
    }

    pub fn rule_exists(&self, module_id: i64, rule_type: &str, description: &str, pattern: Option<&str>) -> Result<bool> {
        let conn = self.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rules
             WHERE module_id = ?1 AND rule_type = ?2 AND description = ?3
               AND ((?4 IS NULL AND pattern IS NULL) OR pattern = ?4)",
            (module_id, rule_type, description, pattern),
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn delete_rule(&self, rule_id: i64) -> Result<()> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM rules WHERE id = ?1", [rule_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_decision(&self, title: &str, context: Option<&str>, decision: &str) -> Result<i64> {
        let conn = self.lock();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        
        conn.execute(
            "INSERT INTO decisions (title, context, decision, created_at) VALUES (?1, ?2, ?3, ?4)",
            (title, context, decision, now),
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_decisions(&self, limit: usize) -> Result<Vec<(i64, String, Option<String>, String, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, context, decision, created_at FROM decisions ORDER BY created_at DESC LIMIT ?1"
        )?;
        
        let rows = stmt.query_map([limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
        
        Ok(rows)
    }

    pub fn search_decisions_db(&self, query: &str) -> Result<Vec<(i64, String, Option<String>, String, i64)>> {
        let conn = self.lock();
        let pattern = format!("%{}%", query.to_lowercase());
        
        let mut stmt = conn.prepare(
            "SELECT id, title, context, decision, created_at FROM decisions 
             WHERE LOWER(title) LIKE ?1 OR LOWER(context) LIKE ?1 OR LOWER(decision) LIKE ?1
             ORDER BY created_at DESC LIMIT 20"
        )?;
        
        let rows = stmt.query_map([&pattern], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
        
        Ok(rows)
    }

    /// In-memory database for tests. No journal for `:memory:`.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        })
    }

    /// Every file this index knows about, repo-relative. The retrieval
    /// universe: any eval or ranking must score against this same set, or it
    /// is measuring coverage instead of ranking.
    #[allow(dead_code)] // Step 1 eval harness
    pub fn indexed_files(&self) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT file_path FROM file_map ORDER BY file_path")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn search_files(&self, keyword: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT file_path FROM file_map WHERE file_path LIKE ?1")?;
        let pattern = format!("%{}%", keyword);
        let rows = stmt.query_map([pattern], |row| row.get(0))?;
        let mut files = Vec::new();
        for row in rows {
            if let Ok(f) = row {
                files.push(f);
            }
        }
        Ok(files)
    }

    pub fn get_module_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        let conn = self.lock();
        let result: Result<i64, _> = conn.query_row(
            "SELECT id FROM modules WHERE LOWER(name) = LOWER(?1) LIMIT 1",
            [name],
            |row| row.get(0)
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(_) => Ok(None),
        }
    }

    pub fn get_module_info(&self, name: &str) -> Result<Option<ModuleInfo>> {
        let conn = self.lock();
        let result: Result<(i64, String, String), _> = conn.query_row(
            "SELECT id, layer, path_pattern FROM modules WHERE LOWER(name) = LOWER(?1) LIMIT 1",
            [name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        );

        let (module_id, layer, path_pattern) = match result {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        let file_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM file_map WHERE module_id = ?1",
            [module_id],
            |row| row.get(0)
        )?;

        let mut stmt = conn.prepare(
            "SELECT file_path FROM file_map WHERE module_id = ?1"
        )?;
        let files: Vec<String> = stmt
            .query_map([module_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt = conn.prepare(
            "SELECT id, rule_type, description, pattern FROM rules WHERE module_id = ?1"
        )?;
        let rules: Vec<Rule> = stmt
            .query_map([module_id], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    rule_type: RuleType::from(row.get::<_, String>(1)?),
                    description: row.get(2)?,
                    pattern: row.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Some(ModuleInfo {
            name: name.to_string(),
            layer,
            path_pattern,
            file_count,
            files,
            rules,
        }))
    }

    pub fn get_all_module_names(&self) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT DISTINCT name FROM modules ORDER BY name")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    pub fn find_relevant_modules(&self, keywords: &[String]) -> Result<Vec<ModuleContext>> {
        let conn = self.lock();
        let mut results: Vec<(i64, String, String, String, Vec<String>)> = Vec::new();

        // Get all modules
        let mut stmt = conn.prepare("SELECT id, name, layer, path_pattern FROM modules")?;
        let modules: Vec<(i64, String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (id, name, layer, pattern) in modules {
            let mut score = 0;
            let name_lower = name.to_lowercase();
            
            for kw in keywords {
                let kw_lower = kw.to_lowercase();
                if name_lower.contains(&kw_lower) {
                    score += 3;
                }
            }

            if score > 0 {
                // Get rules
                let mut rule_stmt = conn.prepare(
                    "SELECT rule_type, description, pattern FROM rules WHERE module_id = ?1"
                )?;
                let rules: Vec<String> = rule_stmt
                    .query_map([id], |row| {
                        let rt: String = row.get(0)?;
                        let desc: String = row.get(1)?;
                        Ok(format!("[{}] {}", rt, desc))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();

                results.push((score, name, layer, pattern, rules));
            }
        }

        // Sort by score descending and take top 4
        results.sort_by(|a, b| b.0.cmp(&a.0));
        results.truncate(4);

        let context: Vec<ModuleContext> = results
            .into_iter()
            .map(|(_, name, layer, pattern, rules)| ModuleContext {
                name,
                layer,
                path_pattern: pattern,
                rules,
            })
            .collect();

        Ok(context)
    }

    pub fn insert_symbols_batch(&self, symbols: &[Symbol]) -> Result<()> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        for symbol in symbols {
            tx.execute(
                "INSERT INTO symbols (file_path, symbol_type, name, signature, line_number, module_id, exported) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (symbol.file_path.as_str(), symbol.symbol_type.as_str(), symbol.name.as_str(), symbol.signature.as_deref(), symbol.line_number, symbol.module_id, symbol.exported as i32),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn insert_imports_batch(&self, imports: &[Import]) -> Result<()> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction()?;
        for import in imports {
            let names_json = serde_json::to_string(&import.imported_names).unwrap_or_else(|_| "[]".to_string());
            tx.execute(
                "INSERT INTO imports (file_path, imported_from, imported_names, module_id, resolved_to, line_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (import.file_path.as_str(), import.imported_from.as_str(), names_json.as_str(), import.module_id, import.resolved_to.as_deref(), import.line_number),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_symbols_by_file(&self, file_path: &str) -> Result<Vec<Symbol>> {
        let conn = self.lock();
        let file_path = self.normalize(file_path);
        let mut stmt = conn.prepare(&format!(
            "SELECT id, file_path, symbol_type, name, signature, line_number, module_id, exported FROM symbols WHERE {}",
            Self::path_match("file_path")
        ))?;
        let symbols = stmt.query_map([&file_path], |row| {
            let symbol_type_str: String = row.get(2)?;
            Ok(Symbol {
                id: row.get(0)?,
                file_path: row.get(1)?,
                symbol_type: SymbolType::from(symbol_type_str),
                name: row.get(3)?,
                signature: row.get(4)?,
                line_number: row.get(5)?,
                module_id: row.get(6)?,
                exported: row.get::<_, i32>(7)? != 0,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        Ok(symbols)
    }

    pub fn search_symbols(&self, query: &str) -> Result<Vec<SymbolSearchResult>> {
        let conn = self.lock();
        let query_lower = query.to_lowercase();
        let pattern = format!("%{}%", query_lower);
        
        let mut stmt = conn.prepare(
            "SELECT s.name, s.signature, s.file_path, m.name, s.symbol_type
             FROM symbols s
             JOIN modules m ON s.module_id = m.id
             WHERE LOWER(s.name) LIKE ?1
             ORDER BY 
                 CASE WHEN LOWER(s.name) = ?2 THEN 0
                      WHEN LOWER(s.name) LIKE ?2 || '%' THEN 1
                      ELSE 2 END,
                 s.name
             LIMIT 50"
        )?;
        
        let results = stmt.query_map([&pattern, &query_lower], |row| {
            let symbol_type_str: String = row.get(4)?;
            Ok(SymbolSearchResult {
                name: row.get(0)?,
                signature: row.get(1)?,
                file_path: row.get(2)?,
                module_name: row.get(3)?,
                symbol_type: SymbolType::from(symbol_type_str),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        
        Ok(results)
    }

    pub fn get_imports_by_file(&self, file_path: &str) -> Result<Vec<Import>> {
        let conn = self.lock();
        let file_path = self.normalize(file_path);
        let mut stmt = conn.prepare(&format!(
            "SELECT id, file_path, imported_from, imported_names, module_id, resolved_to, line_number FROM imports WHERE {}",
            Self::path_match("file_path")
        ))?;
        let imports = stmt.query_map([&file_path], |row| {
            let imported_names_str: String = row.get(3)?;
            let imported_names: Vec<String> = serde_json::from_str(&imported_names_str).unwrap_or_default();
            Ok(Import {
                id: row.get(0)?,
                file_path: row.get(1)?,
                imported_from: row.get(2)?,
                imported_names,
                module_id: row.get(4)?,
                resolved_to: row.get(5)?,
                line_number: row.get(6)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        Ok(imports)
    }

    pub fn clear_file_symbols(&self, file_path: &str) -> Result<()> {
        let conn = self.lock();
        let file_path = self.normalize(file_path);
        let tx = conn.unchecked_transaction()?;
        let symbols_sql = format!("DELETE FROM symbols WHERE {}", Self::path_match("file_path"));
        let imports_sql = format!("DELETE FROM imports WHERE {}", Self::path_match("file_path"));
        tx.execute(&symbols_sql, [&file_path])?;
        tx.execute(&imports_sql, [&file_path])?;
        tx.commit()?;
        Ok(())
    }

    pub fn search_decisions(&self, keywords: &[String]) -> Result<Vec<(String, String, String)>> {
        let conn = self.lock();
        if keywords.is_empty() {
            return Ok(Vec::new());
        }

        let pattern = keywords.iter()
            .map(|k| format!("%{}%", k.to_lowercase()))
            .collect::<Vec<_>>();
        
        let conditions: Vec<String> = (0..keywords.len()).map(|_| "LOWER(title) LIKE ? OR LOWER(context) LIKE ?".to_string()).collect();
        let query = format!(
            "SELECT title, context, decision FROM decisions WHERE {} ORDER BY created_at DESC LIMIT 10",
            conditions.join(" OR ")
        );

        let mut stmt = conn.prepare(&query)?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        for k in &pattern {
            params.push(Box::new(k.clone()));
            params.push(Box::new(k.clone()));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let results = stmt.query_map(params_refs.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

        Ok(results)
    }

    pub fn find_adjacent_modules(&self, module_name: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let layer_str: Option<String> = conn.query_row(
            "SELECT layer FROM modules WHERE name = ?1",
            [module_name],
            |row| row.get(0)
        ).ok();

        if layer_str.is_none() {
            return Ok(Vec::new());
        }

        let layer_str = layer_str.unwrap();
        let layer_type = match layer_str.as_str() {
            "ui" => vec!["api", "service", "db"],
            "api" => vec!["ui", "service"],
            "service" => vec!["ui"],
            "db" => vec!["ui", "api"],
            _ => vec![],
        };

        if layer_type.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        for lt in layer_type {
            if let Ok(name) = conn.query_row(
                "SELECT name FROM modules WHERE layer = ?1",
                [lt],
                |row| row.get(0)
            ) {
                if !results.contains(&name) {
                    results.push(name);
                }
            }
        }

        Ok(results)
    }
}

#[derive(Debug, Serialize)]
pub struct ModuleContext {
    pub name: String,
    pub layer: String,
    pub path_pattern: String,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub id: i64,
    pub file_path: String,
    pub symbol_type: SymbolType,
    pub name: String,
    pub signature: Option<String>,
    pub line_number: i64,
    pub module_id: i64,
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum SymbolType {
    Function,
    Class,
    Struct,
    Enum,
    Interface,
    Route,
}

impl From<String> for SymbolType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "function" => SymbolType::Function,
            "class" => SymbolType::Class,
            "struct" => SymbolType::Struct,
            "enum" => SymbolType::Enum,
            "interface" => SymbolType::Interface,
            "route" => SymbolType::Route,
            _ => SymbolType::Function,
        }
    }
}

impl From<&str> for SymbolType {
    fn from(s: &str) -> Self {
        match s {
            "function" => SymbolType::Function,
            "class" => SymbolType::Class,
            "struct" => SymbolType::Struct,
            "enum" => SymbolType::Enum,
            "interface" => SymbolType::Interface,
            "route" => SymbolType::Route,
            _ => SymbolType::Function,
        }
    }
}

impl SymbolType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolType::Function => "function",
            SymbolType::Class => "class",
            SymbolType::Struct => "struct",
            SymbolType::Enum => "enum",
            SymbolType::Interface => "interface",
            SymbolType::Route => "route",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Import {
    pub id: i64,
    pub file_path: String,
    pub imported_from: String,
    pub imported_names: Vec<String>,
    pub module_id: i64,
    /// Repo-relative target when the specifier resolved, `None` when it did
    /// not. `None` means UNCLASSIFIED -- it is never a violation.
    pub resolved_to: Option<String>,
    /// 1-based line of the import statement. A violation with a file and no
    /// line is a finding nobody can act on.
    pub line_number: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolSearchResult {
    pub name: String,
    pub signature: Option<String>,
    pub file_path: String,
    pub module_name: String,
    pub symbol_type: SymbolType,
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    fn mem_db() -> Db {
        let db = Db::open_in_memory().expect("open in-memory");
        db.init_schema().expect("init schema");
        db
    }

    fn seeded() -> Db {
        let db = mem_db();
        let module_id = db
            .insert_module("components", "ui", "src/components/**")
            .expect("insert module");
        db.insert_rule(
            module_id,
            "forbidden",
            "ui must not import db",
            Some("from ['\"]@/db"),
        )
        .expect("insert rule");
        db.upsert_file("src/components/Button.tsx", module_id)
            .expect("upsert file");
        db.insert_symbols_batch(&[Symbol {
            id: 0,
            file_path: "src/components/Button.tsx".to_string(),
            symbol_type: SymbolType::Function,
            name: "Button".to_string(),
            signature: Some("Button(): string".to_string()),
            line_number: 1,
            module_id,
            exported: true,
        }])
        .expect("insert symbols");
        db.insert_imports_batch(&[Import {
            id: 0,
            file_path: "src/components/Button.tsx".to_string(),
            imported_from: "@/db".to_string(),
            imported_names: vec!["db".to_string()],
            module_id,
            resolved_to: None,
            line_number: 1,
        }])
        .expect("insert imports");
        db
    }

    fn abs(cwd_relative: &str) -> String {
        std::env::current_dir()
            .unwrap()
            .join(cwd_relative)
            .to_string_lossy()
            .to_string()
    }

    fn pragma(db: &Db, name: &str) -> String {
        let conn = db.lock();
        // Some pragmas answer INTEGER, some TEXT -- take the raw Value.
        let value: rusqlite::types::Value = conn
            .query_row(&format!("PRAGMA {}", name), [], |r| r.get(0))
            .unwrap();
        match value {
            rusqlite::types::Value::Text(t) => t,
            rusqlite::types::Value::Integer(i) => i.to_string(),
            other => format!("{:?}", other),
        }
    }

    // --- path normalization on lookup ---

    #[test]
    fn context_lookup_accepts_absolute_path() {
        let db = seeded();
        assert!(db
            .get_context_for_file(&abs("src/components/Button.tsx"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn context_lookup_accepts_dot_slash_prefix() {
        let db = seeded();
        assert!(db
            .get_context_for_file("./src/components/Button.tsx")
            .unwrap()
            .is_some());
    }

    #[test]
    fn context_lookup_accepts_backslashes() {
        let db = seeded();
        assert!(db
            .get_context_for_file("src\\components\\Button.tsx")
            .unwrap()
            .is_some());
    }

    #[test]
    fn context_lookup_accepts_parent_segments() {
        let db = seeded();
        assert!(db
            .get_context_for_file("src/db/../components/Button.tsx")
            .unwrap()
            .is_some());
    }

    #[test]
    fn module_for_file_accepts_absolute_path() {
        let db = seeded();
        let hit = db
            .get_module_for_file(&abs("src/components/Button.tsx"))
            .unwrap();
        assert_eq!(hit.map(|(name, _)| name), Some("components".to_string()));
    }

    #[test]
    fn symbols_by_file_accept_absolute_path() {
        let db = seeded();
        let syms = db
            .get_symbols_by_file(&abs("src/components/Button.tsx"))
            .unwrap();
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "Button");
    }

    #[test]
    fn imports_by_file_accept_absolute_path() {
        let db = seeded();
        let imports = db
            .get_imports_by_file(&abs("src/components/Button.tsx"))
            .unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].imported_from, "@/db");
    }

    #[test]
    fn symbols_by_file_returns_empty_for_unindexed_file() {
        let db = seeded();
        assert!(db
            .get_symbols_by_file("src/components/Nope.tsx")
            .unwrap()
            .is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn context_lookup_is_case_insensitive_on_case_insensitive_fs() {
        let db = seeded();
        assert!(db
            .get_context_for_file("SRC/Components/Button.tsx")
            .unwrap()
            .is_some());
    }

    #[test]
    fn upsert_and_clear_use_the_same_normalized_path() {
        let db = mem_db();
        let module_id = db.insert_module("components", "ui", "src/components/**").unwrap();
        db.upsert_file("./src/components/A.tsx", module_id).unwrap();
        db.insert_symbols_batch(&[Symbol {
            id: 0,
            file_path: "./src/components/A.tsx".to_string(),
            symbol_type: SymbolType::Function,
            name: "a".to_string(),
            signature: None,
            line_number: 1,
            module_id,
            exported: false,
        }])
        .unwrap();
        db.clear_file_symbols("src/components/A.tsx").unwrap();
        assert!(db.get_symbols_by_file("src/components/A.tsx").unwrap().is_empty());
        assert!(db
            .get_module_for_file("src/components/A.tsx")
            .unwrap()
            .is_some());
    }

    // --- WAL + pragmas ---

    #[test]
    fn open_enables_wal_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".archex").join("archex.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Db::open(&path).unwrap();
        assert_eq!(pragma(&db, "journal_mode").to_lowercase(), "wal");
    }

    #[test]
    fn open_enables_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".archex").join("archex.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Db::open(&path).unwrap();
        assert_eq!(pragma(&db, "foreign_keys"), "1");
    }

    #[test]
    fn open_sets_a_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".archex").join("archex.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Db::open(&path).unwrap();
        let conn = db.lock();
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert!(timeout > 0);
    }

    // --- shared connection ---

    #[test]
    fn shared_returns_one_instance_per_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.db");
        let b = dir.path().join("b.db");
        let one = Db::shared(&a).unwrap();
        let again = Db::shared(&a).unwrap();
        let other = Db::shared(&b).unwrap();
        assert!(Arc::ptr_eq(&one, &again), "same path must reuse one connection");
        assert!(!Arc::ptr_eq(&one, &other), "different paths must not share");
    }

    #[test]
    fn shared_instance_survives_writes_from_both_handles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");
        let one = Db::shared(&path).unwrap();
        let again = Db::shared(&path).unwrap();
        one.init_schema().unwrap();
        again.init_schema().unwrap();
        one.insert_module("components", "ui", "src/components/**").unwrap();
        assert_eq!(again.get_rule_count().unwrap(), 0);
        assert_eq!(again.get_all_modules().unwrap().len(), 1);
    }

    #[test]
    fn db_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Db>();
    }
}


#[cfg(test)]
mod schema_tests {
    use super::*;
    use std::fs;

    /// The DB is a derived cache, never the source of truth, so a schema
    /// change drops and rebuilds instead of writing a migration. A migration
    /// would be code that only ever runs once, on machines nobody has.
    #[test]
    fn schema_version_mismatch_rebuilds_instead_of_migrating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archex.db");

        // A database written by an older archex.
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE modules (id INTEGER PRIMARY KEY, name TEXT);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO modules (id, name) VALUES (1, 'legacy');
                 INSERT INTO meta (key, value) VALUES ('schema_version', '1');",
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        db.init_schema().unwrap();

        let legacy: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM modules WHERE name = 'legacy'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(legacy, 0, "a stale schema must be dropped, not migrated");

        // And the rebuilt DB is actually usable.
        db.insert_module("web", "ui", "apps/web/**").unwrap();
        assert_eq!(db.get_all_modules().unwrap().len(), 1);
    }

    #[test]
    fn matching_schema_version_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archex.db");

        let db = Db::open(&path).unwrap();
        db.init_schema().unwrap();
        db.insert_module("web", "ui", "apps/web/**").unwrap();
        db.init_schema().unwrap();

        assert_eq!(
            db.get_all_modules().unwrap().len(),
            1,
            "re-running init_schema must never drop a current database"
        );
    }

    #[test]
    fn schema_version_is_recorded() {
        let db = Db::open_in_memory().unwrap();
        db.init_schema().unwrap();
        let conn = db.lock();
        let value: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn a_corrupt_version_is_treated_as_a_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archex.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE modules (id INTEGER PRIMARY KEY, name TEXT);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO modules (id, name) VALUES (1, 'legacy');
                 INSERT INTO meta (key, value) VALUES ('schema_version', 'garbage');",
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        db.init_schema().unwrap();
        assert_eq!(db.get_all_modules().unwrap().len(), 0);
    }
}
