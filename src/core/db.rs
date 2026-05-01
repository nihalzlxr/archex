use rusqlite::{Connection, Result};
use std::path::Path;
use serde::Serialize;

pub struct Db {
    conn: Connection,
}

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

#[derive(Debug, Serialize)]
pub struct FileContext {
    pub module_name: String,
    pub layer: String,
    pub rules: Vec<Rule>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Ok(Self { conn })
    }

    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS modules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                layer TEXT NOT NULL,
                path_pattern TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
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

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS file_map (
                file_path TEXT PRIMARY KEY,
                module_id INTEGER NOT NULL,
                last_parsed INTEGER NOT NULL,
                FOREIGN KEY(module_id) REFERENCES modules(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS decisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                context TEXT,
                decision TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        Ok(())
    }

    pub fn insert_module(&self, name: &str, layer: &str, path_pattern: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO modules (name, layer, path_pattern) VALUES (?1, ?2, ?3)",
            [name, layer, path_pattern],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_rule(&self, module_id: i64, rule_type: &str, description: &str, pattern: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO rules (module_id, rule_type, description, pattern) VALUES (?1, ?2, ?3, ?4)",
            (module_id, rule_type, description, pattern),
        )?;
        Ok(())
    }

    pub fn get_all_modules(&self) -> Result<Vec<Module>> {
        let mut stmt = self.conn.prepare("SELECT id, name, layer, path_pattern FROM modules")?;
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT OR REPLACE INTO file_map (file_path, module_id, last_parsed) VALUES (?1, ?2, ?3)",
            (file_path, module_id, now),
        )?;
        Ok(())
    }

    pub fn get_context_for_file(&self, file_path: &str) -> Result<Option<FileContext>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.name, m.layer, r.rule_type, r.description, r.pattern
             FROM file_map fm
             JOIN modules m ON fm.module_id = m.id
             LEFT JOIN rules r ON r.module_id = m.id
             WHERE fm.file_path = ?1"
        )?;

        let mut rows = stmt.query([file_path])?;

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

    pub fn get_rule_count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM rules", [], |row| row.get(0))
    }

    pub fn search_files(&self, keyword: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT file_path FROM file_map WHERE file_path LIKE ?1")?;
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
        let result: Result<i64, _> = self.conn.query_row(
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
        let result: Result<(i64, String, String), _> = self.conn.query_row(
            "SELECT id, layer, path_pattern FROM modules WHERE LOWER(name) = LOWER(?1) LIMIT 1",
            [name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        );

        let (module_id, layer, path_pattern) = match result {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        let file_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM file_map WHERE module_id = ?1",
            [module_id],
            |row| row.get(0)
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT file_path FROM file_map WHERE module_id = ?1"
        )?;
        let files: Vec<String> = stmt
            .query_map([module_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare("SELECT DISTINCT name FROM modules ORDER BY name")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    pub fn find_relevant_modules(&self, keywords: &[String]) -> Result<Vec<ModuleContext>> {
        let mut results: Vec<(i64, String, String, String, Vec<String>)> = Vec::new();

        // Get all modules
        let mut stmt = self.conn.prepare("SELECT id, name, layer, path_pattern FROM modules")?;
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
                // Get sample files (up to 5)
                let mut file_stmt = self.conn.prepare(
                    "SELECT file_path FROM file_map WHERE module_id = ?1 LIMIT 5"
                )?;
                let files: Vec<String> = file_stmt
                    .query_map([id], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();

                // Get rules
                let mut rule_stmt = self.conn.prepare(
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
}

#[derive(Debug, Serialize)]
pub struct ModuleContext {
    pub name: String,
    pub layer: String,
    pub path_pattern: String,
    pub rules: Vec<String>,
}