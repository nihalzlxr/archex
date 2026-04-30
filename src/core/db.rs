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
            "SELECT m.name, m.layer
             FROM file_map f
             JOIN modules m ON f.module_id = m.id
             WHERE f.file_path = ?1"
        )?;

        let module: Option<(String, String)> = stmt
            .query_row([file_path], |row| Ok((row.get(0)?, row.get(1)?)))
            .ok();

        let Some((module_name, layer)) = module else {
            return Ok(None);
        };

        let mut stmt = self.conn.prepare(
            "SELECT id, rule_type, description, pattern
             FROM rules
             WHERE module_id = (SELECT module_id FROM file_map WHERE file_path = ?1)"
        )?;

        let rules: Vec<Rule> = stmt
            .query_map([file_path], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    rule_type: RuleType::from(row.get::<_, String>(1)?),
                    description: row.get(2)?,
                    pattern: row.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Some(FileContext {
            module_name,
            layer,
            rules,
        }))
    }
}