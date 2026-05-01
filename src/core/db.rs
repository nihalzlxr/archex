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

        self.create_symbols_table()?;
        self.create_imports_table()?;

        Ok(())
    }

    pub fn create_symbols_table(&self) -> Result<()> {
        self.conn.execute(
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
        
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_module ON symbols(module_id)",
            [],
        )?;

        Ok(())
    }

    pub fn create_imports_table(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS imports (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                imported_from TEXT NOT NULL,
                imported_names TEXT NOT NULL,
                module_id INTEGER NOT NULL,
                FOREIGN KEY(module_id) REFERENCES modules(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_imports_file ON imports(file_path)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_imports_module ON imports(module_id)",
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

    pub fn get_module_for_file(&self, file_path: &str) -> Result<Option<(String, i64)>> {
        let result: Result<(String, i64), _> = self.conn.query_row(
            "SELECT m.name, fm.module_id FROM file_map fm JOIN modules m ON fm.module_id = m.id WHERE fm.file_path = ?1",
            [file_path],
            |row| Ok((row.get(0)?, row.get(1)?))
        );
        
        match result {
            Ok((name, id)) => Ok(Some((name, id))),
            Err(_) => Ok(None),
        }
    }

    pub fn get_rules_for_module(&self, module_id: i64) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(
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

    pub fn get_module_path(&self, module_name: &str) -> Result<Option<String>> {
        let result: Result<String, _> = self.conn.query_row(
            "SELECT path_pattern FROM modules WHERE name = ?1",
            [module_name],
            |row| row.get(0)
        );
        
        match result {
            Ok(pattern) => Ok(Some(pattern)),
            Err(_) => Ok(None),
        }
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

    pub fn insert_symbol(&self, file_path: &str, symbol_type: &str, name: &str, signature: Option<&str>, line_number: i64, module_id: i64, exported: bool) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        let result = tx.execute(
            "INSERT INTO symbols (file_path, symbol_type, name, signature, line_number, module_id, exported) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (file_path, symbol_type, name, signature, line_number, module_id, exported as i32),
        )?;
        tx.commit()?;
        Ok(result as i64)
    }

    pub fn insert_symbols_batch(&self, symbols: &[Symbol]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for symbol in symbols {
            tx.execute(
                "INSERT INTO symbols (file_path, symbol_type, name, signature, line_number, module_id, exported) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (symbol.file_path.as_str(), symbol.symbol_type.as_str(), symbol.name.as_str(), symbol.signature.as_deref(), symbol.line_number, symbol.module_id, symbol.exported as i32),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn insert_import(&self, file_path: &str, imported_from: &str, imported_names: &[String], module_id: i64) -> Result<i64> {
        let names_json = serde_json::to_string(imported_names).unwrap_or_else(|_| "[]".to_string());
        let tx = self.conn.unchecked_transaction()?;
        let result = tx.execute(
            "INSERT INTO imports (file_path, imported_from, imported_names, module_id) VALUES (?1, ?2, ?3, ?4)",
            (file_path, imported_from, names_json.as_str(), module_id),
        )?;
        tx.commit()?;
        Ok(result as i64)
    }

    pub fn insert_imports_batch(&self, imports: &[Import]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for import in imports {
            let names_json = serde_json::to_string(&import.imported_names).unwrap_or_else(|_| "[]".to_string());
            tx.execute(
                "INSERT INTO imports (file_path, imported_from, imported_names, module_id) VALUES (?1, ?2, ?3, ?4)",
                (import.file_path.as_str(), import.imported_from.as_str(), names_json.as_str(), import.module_id),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_symbols_by_module(&self, module_id: i64) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, symbol_type, name, signature, line_number, module_id, exported FROM symbols WHERE module_id = ?1"
        )?;
        let symbols = stmt.query_map([module_id], |row| {
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

    pub fn get_symbols_by_file(&self, file_path: &str) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, symbol_type, name, signature, line_number, module_id, exported FROM symbols WHERE file_path = ?1"
        )?;
        let symbols = stmt.query_map([file_path], |row| {
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
        let query_lower = query.to_lowercase();
        let pattern = format!("%{}%", query_lower);
        
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, imported_from, imported_names, module_id FROM imports WHERE file_path = ?1"
        )?;
        let imports = stmt.query_map([file_path], |row| {
            let imported_names_str: String = row.get(3)?;
            let imported_names: Vec<String> = serde_json::from_str(&imported_names_str).unwrap_or_default();
            Ok(Import {
                id: row.get(0)?,
                file_path: row.get(1)?,
                imported_from: row.get(2)?,
                imported_names,
                module_id: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        Ok(imports)
    }

    pub fn get_symbol_count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
    }

    pub fn get_import_count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM imports", [], |row| row.get(0))
    }

    pub fn clear_file_symbols(&self, file_path: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM symbols WHERE file_path = ?1", [file_path])?;
        tx.execute("DELETE FROM imports WHERE file_path = ?1", [file_path])?;
        tx.commit()?;
        Ok(())
    }

    pub fn search_decisions(&self, keywords: &[String]) -> Result<Vec<(String, String, String)>> {
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

        let mut stmt = self.conn.prepare(&query)?;
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

    pub fn get_forbidden_rules(&self, module_names: &[String]) -> Result<Vec<(String, String, String)>> {
        if module_names.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        
        for name in module_names {
            let query = "SELECT m.name, r.description, r.pattern FROM rules r 
                        JOIN modules m ON r.module_id = m.id 
                        WHERE r.rule_type = 'forbidden' AND m.name = ?1";
            
            let mut stmt = self.conn.prepare(query)?;
            let rows = stmt.query_map([name.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })?;
            
            for row in rows {
                if let Ok(r) = row {
                    results.push(r);
                }
            }
        }

        Ok(results)
    }

    pub fn find_adjacent_modules(&self, module_name: &str) -> Result<Vec<String>> {
        let layer_str: Option<String> = self.conn.query_row(
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
            if let Ok(name) = self.conn.query_row(
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
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolSearchResult {
    pub name: String,
    pub signature: Option<String>,
    pub file_path: String,
    pub module_name: String,
    pub symbol_type: SymbolType,
}