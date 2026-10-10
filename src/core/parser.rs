use crate::core::db::{Db, Import, Module, Symbol, SymbolType};
use crate::core::paths;
use crate::core::resolve::Resolver;
use glob::Pattern;
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::Arc;
use walkdir::WalkDir;
use serde::Serialize;

pub struct Parser {
    db: Arc<Db>,
}

#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub files_scanned: usize,
    pub files_mapped: usize,
    pub files_unmapped: Vec<PathBuf>,
    pub symbols_extracted: usize,
    pub imports_extracted: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftViolation {
    pub rule_type: String,
    pub rule_description: String,
    pub pattern: Option<String>,
    pub line_number: Option<i64>,
    pub suggestion: String,
}

#[derive(Debug, Serialize)]
pub struct DriftResult {
    pub file_path: String,
    pub module: String,
    pub violations: Vec<DriftViolation>,
    pub clean: bool,
}

impl DriftResult {
    /// Recompute `clean` from the current violations. Must be called after
    /// pushing violations onto an existing result — `clean` is a derived
    /// value and is stale the moment the violation list changes.
    pub fn recompute_clean(&mut self) {
        self.clean = self
            .violations
            .iter()
            .all(|v| v.rule_type != "forbidden");
    }
}

pub const SKIP_DIRS: &[&str] = &["node_modules", ".next", ".git", "dist", "build", "target"];

pub const VALID_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "rs", "py"];

impl Parser {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    pub fn scan(&self, project_root: &Path) -> Result<ScanResult, Box<dyn std::error::Error>> {
        // One resolver for the whole scan: tsconfig chains and workspace
        // packages are repo-level facts, not per-file ones.
        let resolver = Resolver::new(project_root).ok();

        let modules = self.db.get_all_modules().map_err(|e| format!("DB error: {}", e))?;

        let mut compiled_patterns: Vec<(Module, Pattern)> = modules
            .into_iter()
            .filter_map(|m| {
                Pattern::new(&m.path_pattern)
                    .ok()
                    .map(|p| (m, p))
            })
            .collect();

        compiled_patterns.sort_by(|a, b| b.1.as_str().len().cmp(&a.1.as_str().len()));

        let mut files_scanned = 0;
        let mut files_mapped = 0;
        let mut files_unmapped = Vec::new();
        let mut symbols_extracted = 0;
        let mut imports_extracted = 0;

        let total_entries: usize = WalkDir::new(project_root)
            .into_iter()
            .filter_entry(|e| !Self::should_skip(e))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| Self::has_valid_extension(e.path()))
            .count();

        eprintln!("Scanning {} files...", total_entries);

        for entry in WalkDir::new(project_root)
            .into_iter()
            .filter_entry(|e| !Self::should_skip(e))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| Self::has_valid_extension(e.path()))
        {
            files_scanned += 1;

            let file_path = entry.path();
            let file_path_str = file_path.to_string_lossy();

            // Same normalizer the DB uses on lookup: writer and reader agree
            // by construction instead of by two hand-rolled versions.
            let relative_path = paths::normalize_in(&file_path_str, &project_root);

            if relative_path.is_empty() {
                continue;
            }

            let matched_module = compiled_patterns
                .iter()
                .find(|(_, pattern)| pattern.matches(&relative_path));

            if let Some((module, _)) = matched_module {
                self.db.upsert_file(&relative_path, module.id)?;
                // Replace any previous index rows for this file — a re-scan
                // must never duplicate symbols or imports.
                self.db.clear_file_symbols(&relative_path)?;
                files_mapped += 1;

                if let Ok(content) = fs::read_to_string(file_path) {
                    let (syms, imps) = Self::extract_symbols_and_imports(
                        &relative_path,
                        module.id,
                        file_path,
                        &content,
                        resolver.as_ref(),
                    );
                    
                    if !syms.is_empty() {
                        self.db.insert_symbols_batch(&syms).ok();
                        symbols_extracted += syms.len();
                    }
                    
                    if !imps.is_empty() {
                        self.db.insert_imports_batch(&imps).ok();
                        imports_extracted += imps.len();
                    }
                }
            } else {
                files_unmapped.push(PathBuf::from(&relative_path));
            }

            if files_scanned % 50 == 0 {
                eprintln!("Processed {}/{} files", files_scanned, total_entries);
            }
        }

        eprintln!("Scan complete: {} scanned, {} mapped, {} unmapped",
            files_scanned, files_mapped, files_unmapped.len());

        Ok(ScanResult {
            files_scanned,
            files_mapped,
            files_unmapped,
            symbols_extracted,
            imports_extracted,
        })
    }

    fn should_skip(entry: &walkdir::DirEntry) -> bool {
        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy();
            SKIP_DIRS.contains(&name.as_ref())
        } else {
            false
        }
    }

    fn has_valid_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| VALID_EXTENSIONS.contains(&ext))
            .unwrap_or(false)
    }

    fn extract_symbols_and_imports(
        file_path: &str,
        module_id: i64,
        path: &Path,
        content: &str,
        resolver: Option<&Resolver>,
    ) -> (Vec<Symbol>, Vec<Import>) {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let (symbols, mut imports) = match ext {
            "ts" | "tsx" | "js" | "jsx" => {
                Self::extract_ts_symbols(file_path, module_id, content, path)
            }
            "rs" => {
                Self::extract_rust_symbols(file_path, module_id, content)
            }
            "py" => {
                Self::extract_python_symbols(file_path, module_id, content)
            }
            _ => (Vec::new(), Vec::new()),
        };

        // Resolve after extraction rather than threading a resolver through
        // every collector: the specifier is all that is needed, and one pass
        // is easier to keep honest than resolution in six places.
        if let Some(resolver) = resolver {
            for import in imports.iter_mut() {
                import.resolved_to = resolver.resolve(file_path, &import.imported_from);
            }
        }
        (symbols, imports)
    }

    fn extract_ts_symbols(
        file_path: &str,
        module_id: i64,
        content: &str,
        path: &Path,
    ) -> (Vec<Symbol>, Vec<Import>) {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();

        let is_route_file = Self::is_route_file(path);

        let mut parser = tree_sitter::Parser::new();
        let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT;
        
        if parser.set_language(&language.into()).is_ok() {
            if let Some(tree) = parser.parse(content, None) {
                let root_node = tree.root_node();
                Self::collect_ts_symbols(&root_node, content, file_path, module_id, is_route_file, &mut symbols, &mut imports);
            }
        }

        (symbols, imports)
    }

    fn collect_ts_symbols(
        node: &tree_sitter::Node,
        content: &str,
        file_path: &str,
        module_id: i64,
        is_route_file: bool,
        symbols: &mut Vec<Symbol>,
        imports: &mut Vec<Import>,
    ) {
        let kind = node.kind();
        
        match kind {
            "function_declaration" | "method_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    
                    let signature = Self::ts_function_signature(&name, node, content);

                    let line = node.start_position().row as i64 + 1;
                    
                    let symbol_type = if is_route_file { "route" } else { "function" };
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::from(symbol_type),
                        name,
                        signature: Some(signature),
                        line_number: line,
                        module_id,
                        exported: Self::is_exported(node, content),
                    });
                }
            }
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Class,
                        name,
                        signature: None,
                        line_number: line,
                        module_id,
                        exported: Self::is_exported(node, content),
                    });
                }
            }
            "interface_declaration" | "type_alias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    let symbol_type = if kind == "interface_declaration" { "interface" } else { "struct" };
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::from(symbol_type),
                        name,
                        signature: None,
                        line_number: line,
                        module_id,
                        exported: Self::is_exported(node, content),
                    });
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Enum,
                        name,
                        signature: None,
                        line_number: line,
                        module_id,
                        exported: Self::is_exported(node, content),
                    });
                }
            }
"lexical_declaration" | "variable_declaration" => {
                for i in 0..node.child_count() {
                    let Some(declarator) = node.child(i) else { continue };
                    if declarator.kind() != "variable_declarator" { continue; }

                    let (Some(name_node), Some(value)) = (
                        declarator.child_by_field_name("name"),
                        declarator.child_by_field_name("value"),
                    ) else { continue };

                    if !matches!(
                        value.kind(),
                        "arrow_function" | "function_expression" | "function" | "generator_function"
                    ) {
                        continue;
                    }

                    let name = Self::get_node_text(&name_node, content);
                    let signature = Self::ts_function_signature(&name, &value, content);
                    let line = declarator.start_position().row as i64 + 1;

                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: if is_route_file { SymbolType::Route } else { SymbolType::Function },
                        name,
                        signature: Some(signature),
                        line_number: line,
                        module_id,
                        exported: Self::is_exported(node, content),
                    });
                }
            }
            "import_statement" => {
                let imported_from = Self::extract_ts_import_path(node, content);
                let names = Self::extract_ts_imported_names(node, content);
                
                if !imported_from.is_empty() || !names.is_empty() {
                    imports.push(Import {
                        id: 0,
                        file_path: file_path.to_string(),
                        imported_from,
                        imported_names: names,
                        module_id,
                        resolved_to: None,
                    });
                }
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                Self::collect_ts_symbols(&child, content, file_path, module_id, is_route_file, symbols, imports);
            }
        }
    }

    /// The specifier of an import statement. TypeScript accepts single and
    /// double quotes and both appear in real repos; accepting only one drops
    /// every import written the other way, silently.
    fn extract_ts_import_path(node: &tree_sitter::Node, content: &str) -> String {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() != "string" {
                    continue;
                }
                let text = child.utf8_text(content.as_bytes()).unwrap_or_default();
                let quote = match text.chars().next() {
                    Some(q @ ('"' | '\'' | '`')) => q,
                    _ => continue,
                };
                let inner = text.trim_start_matches(quote);
                let inner = inner.strip_suffix(quote).unwrap_or(inner);
                return inner.replace("\\'", "'").replace("\\\\", "\\");
            }
        }
        String::new()
    }

    fn extract_ts_imported_names(node: &tree_sitter::Node, content: &str) -> Vec<String> {
        let mut names = Vec::new();
        
        fn visit(node: &tree_sitter::Node, content: &str, names: &mut Vec<String>) {
            let kind = node.kind();
            
            if kind == "identifier" || kind == "property_identifier" {
                let text = node.utf8_text(content.as_bytes()).unwrap_or_default();
                if !text.is_empty() && !text.starts_with('"') && !text.starts_with('\'') {
                    names.push(text.to_string());
                }
            }
            
            if kind == "import_specifier" {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        let child_kind = child.kind();
                        if child_kind == "identifier" || child_kind == "property_identifier" {
                            let text = child.utf8_text(content.as_bytes()).unwrap_or_default();
                            if !text.is_empty() {
                                names.push(text.to_string());
                            }
                        }
                    }
                }
            }
            
            if kind == "namespace_import" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let text = name_node.utf8_text(content.as_bytes()).unwrap_or_default();
                    if !text.is_empty() {
                        names.push(text.to_string());
                    }
                }
            }
            
if kind == "default_import" {
                let text = node.utf8_text(content.as_bytes()).unwrap_or_default();
                if !text.is_empty() {
                    names.push(text.trim().to_string());
                }
            }
            
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    let child_ref = &child;
                    visit(child_ref, content, names);
                }
            }
        }
        
        let node_ref = node;
        visit(node_ref, content, &mut names);
        names
    }

    fn extract_rust_symbols(
        file_path: &str,
        module_id: i64,
        content: &str,
    ) -> (Vec<Symbol>, Vec<Import>) {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();

        let mut parser = tree_sitter::Parser::new();
        let language = tree_sitter_rust::LANGUAGE;
        
        if parser.set_language(&language.into()).is_ok() {
            if let Some(tree) = parser.parse(content, None) {
                let root_node = tree.root_node();
                Self::collect_rust_symbols(&root_node, content, file_path, module_id, &mut symbols, &mut imports);
            }
        }

        (symbols, imports)
    }

    fn collect_rust_symbols(
        node: &tree_sitter::Node,
        content: &str,
        file_path: &str,
        module_id: i64,
        symbols: &mut Vec<Symbol>,
        imports: &mut Vec<Import>,
    ) {
        let kind = node.kind();
        
        match kind {
            "function_item" | "method_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let signature = Self::get_rust_signature(node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Function,
                        name,
                        signature: Some(signature),
                        line_number: line,
                        module_id,
                        exported: false,
                    });
                }
            }
            "struct_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Struct,
                        name,
                        signature: None,
                        line_number: line,
                        module_id,
                        exported: false,
                    });
                }
            }
            "enum_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Enum,
                        name,
                        signature: None,
                        line_number: line,
                        module_id,
                        exported: false,
                    });
                }
            }
            "use_declaration" => {
                if let Some(import) = Self::extract_rust_import(node, content) {
                    imports.push(import);
                }
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                Self::collect_rust_symbols(&child, content, file_path, module_id, symbols, imports);
            }
        }
    }

    fn extract_rust_import(node: &tree_sitter::Node, content: &str) -> Option<Import> {
        let mut imported_from = String::new();
        let mut imported_names = Vec::new();
        
        if let Some(tree) = node.child_by_field_name("tree") {
            let text = Self::get_node_text(&tree, content);
            if let Some(path) = text.split_whitespace().last() {
                imported_from = path.to_string();
                
                if text.contains("::") {
                    imported_names = text.split("::")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty() && s != "use" && s != "crate" && s != "self" && s != "super")
                        .collect();
                }
            }
        }
        
        if !imported_from.is_empty() {
            Some(Import {
                id: 0,
                file_path: String::new(),
                imported_from,
                imported_names,
                module_id: 0,
                        resolved_to: None,
            })
        } else {
            None
        }
    }

    fn extract_python_symbols(
        file_path: &str,
        module_id: i64,
        content: &str,
    ) -> (Vec<Symbol>, Vec<Import>) {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();

        let mut parser = tree_sitter::Parser::new();
        let language = tree_sitter_python::LANGUAGE;
        
        if parser.set_language(&language.into()).is_ok() {
            if let Some(tree) = parser.parse(content, None) {
                let root_node = tree.root_node();
                Self::collect_python_symbols(&root_node, content, file_path, module_id, &mut symbols, &mut imports);
            }
        }

        (symbols, imports)
    }

    fn collect_python_symbols(
        node: &tree_sitter::Node,
        content: &str,
        file_path: &str,
        module_id: i64,
        symbols: &mut Vec<Symbol>,
        imports: &mut Vec<Import>,
    ) {
        let kind = node.kind();
        
        match kind {
            "function_definition" | "async_function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let signature = Self::get_python_signature(node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Function,
                        name,
                        signature: Some(signature),
                        line_number: line,
                        module_id,
                        exported: Self::is_python_exported(node, content),
                    });
                }
            }
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::get_node_text(&name_node, content);
                    let line = node.start_position().row as i64 + 1;
                    
                    symbols.push(Symbol {
                        id: 0,
                        file_path: file_path.to_string(),
                        symbol_type: SymbolType::Class,
                        name,
                        signature: None,
                        line_number: line,
                        module_id,
                        exported: Self::is_python_exported(node, content),
                    });
                }
            }
            "import_from_statement" | "import_statement" => {
                if let Some(import) = Self::extract_python_import(node, content) {
                    imports.push(import);
                }
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                Self::collect_python_symbols(&child, content, file_path, module_id, symbols, imports);
            }
        }
    }

    fn extract_python_import(node: &tree_sitter::Node, content: &str) -> Option<Import> {
        let mut imported_from = String::new();
        let mut imported_names = Vec::new();
        
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                let kind = child.kind();
                
                if kind == "module_name" {
                    imported_from = Self::get_node_text(&child, content);
                } else if kind == "dotted_name" {
                    if imported_from.is_empty() {
                        imported_from = Self::get_node_text(&child, content);
                    }
                } else if kind == "alias" {
                    if let Some(name) = child.child_by_field_name("name") {
                        imported_names.push(Self::get_node_text(&name, content));
                    }
                } else if kind == "wildcard_import" {
                    imported_names.push("*".to_string());
                }
            }
        }
        
        if !imported_from.is_empty() {
            Some(Import {
                id: 0,
                file_path: String::new(),
                imported_from,
                imported_names,
                module_id: 0,
                resolved_to: None,
            })
        } else {
            None
        }
    }

    fn get_node_text(node: &tree_sitter::Node, content: &str) -> String {
        node.utf8_text(content.as_bytes())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    fn get_rust_signature(node: &tree_sitter::Node, content: &str) -> String {
        let mut parts = Vec::new();
        
        if let Some(params) = node.child_by_field_name("parameters") {
            let text = Self::get_node_text(&params, content);
            let params_str: Vec<&str> = text.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            for p in params_str {
                if let Some(name) = p.split(':').next() {
                    parts.push(name.trim().to_string());
                }
            }
        }
        
        format!("({})", parts.join(", "))
    }

    fn get_python_signature(node: &tree_sitter::Node, content: &str) -> String {
        let mut parts = Vec::new();
        
        if let Some(params) = node.child_by_field_name("parameters") {
            let text = Self::get_node_text(&params, content);
            let params_str: Vec<&str> = text.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            for p in params_str {
                if p.contains(':') {
                    parts.push(p.split(':').next().unwrap_or(p).trim().to_string());
                } else {
                    parts.push(p.to_string());
                }
            }
        }
        
        format!("({})", parts.join(", "))
    }

    fn is_route_file(path: &Path) -> bool {
        let path_str = path.to_string_lossy().to_lowercase();
        path_str.contains("/app/api/") || 
        path_str.contains("/pages/api/") || 
        path_str.contains("/app/") && (
            path_str.contains("page.ts") || 
            path_str.contains("page.tsx") ||
            path_str.contains("route.ts") ||
            path_str.contains("route.tsx")
        )
    }

    /// Build `name(params): return_type` from tree-sitter fields only.
    /// Field lookups, never subtree search: a body-local `: SomeType` must
    /// never be mistaken for the declared return type.
    fn ts_function_signature(name: &str, node: &tree_sitter::Node, content: &str) -> String {
        let params_text = node
            .child_by_field_name("parameters")
            .map(|p| Self::get_node_text(&p, content))
            .unwrap_or_default();
        let params_text = params_text.trim();
        let params = params_text
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(params_text);

        let mut return_type = node
            .child_by_field_name("return_type")
            .map(|r| Self::get_node_text(&r, content))
            .map(|t| t.trim().trim_start_matches(':').trim().to_string())
            .unwrap_or_default();

        if return_type.is_empty() {
            if let Some(body) = node.child_by_field_name("body") {
                return_type = Self::literal_type(&body).to_string();
            }
        }

        if return_type.is_empty() {
            format!("{}({})", name, params)
        } else {
            format!("{}({}): {}", name, params, return_type)
        }
    }

    /// Return type of a body that is a bare literal — the only case where we
    /// can state the type without reading a type annotation. Empty otherwise:
    /// precision over recall.
    fn literal_type(body: &tree_sitter::Node) -> &'static str {
        match body.kind() {
            "number" => "number",
            "string" | "template_string" => "string",
            "true" | "false" => "boolean",
            "null" => "null",
            "undefined" => "undefined",
            _ => "",
        }
    }

    /// A declaration is exported when it is wrapped in an `export_statement`,
    /// i.e. its PARENT is the export — `export function f() {}` parses as an
    /// export_statement whose `declaration` child is the function, never as a
    /// sibling. Also honours explicit `export { a, b }` clauses.
    fn is_exported(node: &tree_sitter::Node, content: &str) -> bool {
        let Some(parent) = node.parent() else { return false };
        if parent.kind() != "export_statement" && parent.kind() != "declare_export_statement" {
            return false;
        }
        // `export { a, b }` — the declaration is NOT a child, so match by name.
        let Some(clause) = Self::find_export_clause(&parent) else {
            return true;
        };
        let Some(name_node) = node.child_by_field_name("name") else {
            return true;
        };
        let name = Self::get_node_text(&name_node, content);
        Self::export_clause_names(&clause, content).iter().any(|n| *n == name)
    }

    fn find_export_clause<'a>(stmt: &'a tree_sitter::Node) -> Option<tree_sitter::Node<'a>> {
        (0..stmt.named_child_count())
            .filter_map(|i| stmt.named_child(i))
            .find(|c| matches!(c.kind(), "export_clause" | "named_exports"))
    }

    fn export_clause_names(clause: &tree_sitter::Node, content: &str) -> Vec<String> {
        let mut names = Vec::new();
        for i in 0..clause.named_child_count() {
            let Some(child) = clause.named_child(i) else { continue };
            if matches!(child.kind(), "identifier" | "string" | "export_specifier") {
                let text = Self::get_node_text(&child, content);
                let name = text.rsplit(" as ").next().unwrap_or(&text).trim().to_string();
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Python has no `export`: a top-level name not starting with `_` is
    /// public. Nesting (methods, closures) or a leading underscore makes it
    /// private. Decorators wrap the definition in `decorated_definition`.
    fn is_python_exported(node: &tree_sitter::Node, content: &str) -> bool {
        let Some(name_node) = node.child_by_field_name("name") else { return false };
        let name = Self::get_node_text(&name_node, content);
        if name.starts_with('_') {
            return false;
        }
        let mut current = *node;
        while let Some(parent) = current.parent() {
            match parent.kind() {
                "module" => return true,
                "decorated_definition" => current = parent,
                _ => return false,
            }
        }
        false
    }

    pub fn check_drift(file_path: &str, content: &str) -> DriftResult {
        let mut violations = Vec::new();
        
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        
        let path_lower = file_path.to_lowercase();
        let mut all_imports = Vec::new();
        let mut all_symbols = Vec::new();
        
        match ext {
            "ts" | "tsx" | "js" | "jsx" => {
                let (syms, imps) = Self::extract_ts_symbols(file_path, 0, content, Path::new(file_path));
                all_symbols = syms.into_iter().map(|s| (s.name, s.signature.unwrap_or_default())).collect();
                all_imports = imps.into_iter().map(|i| i.imported_from).collect();
            }
            "rs" => {
                let (syms, imps) = Self::extract_rust_symbols(file_path, 0, content);
                all_symbols = syms.into_iter().map(|s| (s.name, s.signature.unwrap_or_default())).collect();
                all_imports = imps.into_iter().map(|i| i.imported_from).collect();
            }
            "py" => {
                let (syms, imps) = Self::extract_python_symbols(file_path, 0, content);
                all_symbols = syms.into_iter().map(|s| (s.name, s.signature.unwrap_or_default())).collect();
                all_imports = imps.into_iter().map(|i| i.imported_from).collect();
            }
            _ => {}
        }
        
        let is_route_file = path_lower.contains("/app/api/") || path_lower.contains("/pages/api/");
        
        if is_route_file {
            violations.push(DriftViolation {
                rule_type: "route".to_string(),
                rule_description: "API route detected".to_string(),
                pattern: None,
                line_number: None,
                suggestion: "Ensure this route follows API conventions".to_string(),
            });
        }
        
        for import_from in &all_imports {
            let import_lower = import_from.to_lowercase();
            if import_lower.contains("@/db") || import_lower.contains("drizzle") || import_lower.contains("prisma") {
                violations.push(DriftViolation {
                    rule_type: "forbidden".to_string(),
                    rule_description: "Direct database access in API layer".to_string(),
                    pattern: Some(import_from.clone()),
                    line_number: None,
                    suggestion: "Use services layer for database access".to_string(),
                });
            }
        }
        
        for (name, sig) in &all_symbols {
            let combined = format!("{} {}", name, sig);
            
            if combined.contains("fetch(") || combined.contains("axios.") {
                violations.push(DriftViolation {
                    rule_type: "forbidden".to_string(),
                    rule_description: "Direct API calls in non-service layer".to_string(),
                    pattern: Some(combined.clone()),
                    line_number: None,
                    suggestion: "Use hooks or services for API calls".to_string(),
                });
            }
            
            if combined.contains("from.*@/server") || combined.contains("from '@/server") {
                violations.push(DriftViolation {
                    rule_type: "forbidden".to_string(),
                    rule_description: "Server calls from components".to_string(),
                    pattern: Some(combined.clone()),
                    line_number: None,
                    suggestion: "Move business logic to services layer".to_string(),
                });
            }
        }
        
        let has_db_import = all_imports.iter().any(|i| i.contains("@/db") || i.contains("drizzle") || i.contains("prisma"));
        let has_zod = all_symbols.iter().any(|(n, _)| n.contains("zod") || content.contains("z.object") || content.contains("zod."));
        let has_validation = has_zod || content.contains("yup") || content.contains("joi");
        
        if path_lower.contains("/api/") && !has_validation && path_lower.ends_with(".ts") {
            violations.push(DriftViolation {
                rule_type: "required".to_string(),
                rule_description: "API routes must validate input".to_string(),
                pattern: Some("zod|yup|joi".to_string()),
                line_number: None,
                suggestion: "Add input validation with zod".to_string(),
            });
        }
        
        if path_lower.contains("/services/") && !has_db_import && path_lower.ends_with(".ts") {
            violations.push(DriftViolation {
                rule_type: "required".to_string(),
                rule_description: "Services must use db layer".to_string(),
                pattern: Some("from.*@/db".to_string()),
                line_number: None,
                suggestion: "Import db layer for data access".to_string(),
            });
        }
        
        let is_ui_layer = path_lower.contains("/components/") || path_lower.contains("/app/") || path_lower.contains("/pages/");
        let has_direct_sql = content.to_lowercase().contains("execute(") || content.to_lowercase().contains("query(");
        
        if is_ui_layer && has_direct_sql {
            violations.push(DriftViolation {
                rule_type: "forbidden".to_string(),
                rule_description: "Direct database access in UI layer".to_string(),
                pattern: Some("execute|query".to_string()),
                line_number: None,
                suggestion: "Use services layer for database queries".to_string(),
            });
        }
        
        let clean = violations.iter().all(|v| v.rule_type != "forbidden");
        
        DriftResult {
            file_path: file_path.to_string(),
            module: String::new(),
            violations,
            clean,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn ts(content: &str) -> Vec<Symbol> {
        Parser::extract_ts_symbols("src/a.ts", 1, content, Path::new("src/a.ts")).0
    }

    fn py(content: &str) -> Vec<Symbol> {
        Parser::extract_python_symbols("a.py", 1, content).0
    }

    fn get<'a>(syms: &'a [Symbol], name: &str) -> &'a Symbol {
        syms.iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("symbol `{}` not extracted. got: {:?}", name, syms.iter().map(|s| s.name.clone()).collect::<Vec<_>>()))
    }

    fn sig(syms: &[Symbol], name: &str) -> String {
        get(syms, name).signature.clone().unwrap_or_default()
    }

    // --- exported: TS ---

    #[test]
    fn ts_export_function_is_exported() {
        let s = ts("export function alpha() {}\n");
        assert!(get(&s, "alpha").exported);
    }

    #[test]
    fn ts_plain_function_is_not_exported() {
        let s = ts("function alpha() {}\n");
        assert!(!get(&s, "alpha").exported);
    }

    #[test]
    fn ts_export_default_function_is_exported() {
        let s = ts("export default function alpha() {}\n");
        assert!(get(&s, "alpha").exported);
    }

    #[test]
    fn ts_export_class_interface_type_enum_are_exported() {
        let s = ts(
            "export class A {}\nexport interface B {}\nexport type C = number;\nexport enum D { X }\n",
        );
        for n in ["A", "B", "C", "D"] {
            assert!(get(&s, n).exported, "{} should be exported", n);
        }
    }

    #[test]
    fn ts_non_exported_class_interface_enum_are_not_exported() {
        let s = ts("class A {}\ninterface B {}\nenum D { X }\ntype C = number;\n");
        for n in ["A", "B", "C", "D"] {
            assert!(!get(&s, n).exported, "{} should not be exported", n);
        }
    }

    // --- import line numbers: the boundary engine reports a line or nothing ---

    fn imports_of(content: &str) -> Vec<(String, i64)> {
        let imports = Parser::extract_ts_symbols("src/a.ts", 1, content, Path::new("src/a.ts")).1;
        imports
            .into_iter()
            .map(|i| (i.imported_from, i.line_number))
            .collect()
    }

    #[test]
    fn static_import_records_its_line() {
        let found = imports_of(
            "import { a } from './a';\n\nimport { b } from './b';\n",
        );
        assert_eq!(found, vec![("./a".to_string(), 1), ("./b".to_string(), 3)]);
    }

    #[test]
    fn side_effect_import_records_its_line() {
        let found = imports_of("// header\nimport './polyfill';\n");
        assert_eq!(found, vec![("./polyfill".to_string(), 2)]);
    }

    #[test]
    fn export_from_records_its_line() {
        let found = imports_of("const a = 1;\nexport { a } from './a';\nexport * from './b';\n");
        assert_eq!(
            found,
            vec![("./a".to_string(), 2), ("./b".to_string(), 3)],
            "re-exports are edges too"
        );
    }

    #[test]
    fn require_call_records_its_line() {
        let found = imports_of("const x = require('./x');\n");
        assert_eq!(found, vec![("./x".to_string(), 1)]);
    }

    #[test]
    fn dynamic_import_records_its_line() {
        let found = imports_of("async function f() {\n  const m = await import('./lazy');\n}\n");
        assert_eq!(found, vec![("./lazy".to_string(), 2)]);
    }

    #[test]
    fn python_import_records_its_line() {
        let imports = Parser::extract_python_symbols("a.py", 1, "import os\nfrom x import y\n").1;
        let lines: Vec<i64> = imports.iter().map(|i| i.line_number).collect();
        assert_eq!(lines, vec![1, 2]);
    }

    // --- signatures: TS ---

    #[test]
    fn ts_signature_ignores_types_from_function_body() {
        let s = ts(
            "export function alpha(a: string): Promise<void> {\n  const x: SecretType = a as any;\n  return;\n}\n",
        );
        assert_eq!(sig(&s, "alpha"), "alpha(a: string): Promise<void>");
    }

    #[test]
    fn ts_signature_without_return_type() {
        let s = ts("export function alpha(a: string, b: number) {\n  const x: Inner = 1;\n}\n");
        assert_eq!(sig(&s, "alpha"), "alpha(a: string, b: number)");
    }

    #[test]
    fn ts_arrow_function_symbol_and_signature() {
        let s = ts("export const alpha = (a: string): Promise<void> => {\n  const y: Inner = 1;\n};\n");
        assert_eq!(sig(&s, "alpha"), "alpha(a: string): Promise<void>");
        assert!(get(&s, "alpha").exported);
    }

    #[test]
    fn ts_arrow_function_without_params() {
        let s = ts("export const alpha = () => 1;\n");
        assert_eq!(sig(&s, "alpha"), "alpha(): number");
        assert!(get(&s, "alpha").exported);
    }

    #[test]
    fn ts_non_exported_arrow_function() {
        let s = ts("const alpha = () => 1;\n");
        assert!(!get(&s, "alpha").exported);
    }

    #[test]
    fn ts_function_expression_variable() {
        let s = ts("export const alpha = function (a: number): string { return ''; };\n");
        assert_eq!(sig(&s, "alpha"), "alpha(a: number): string");
    }

    // --- exported: Python ---

    #[test]
    fn python_module_level_def_is_exported() {
        let s = py("def alpha():\n    pass\n");
        assert!(get(&s, "alpha").exported);
    }

    #[test]
    fn python_underscore_def_is_private() {
        let s = py("def _alpha():\n    pass\n");
        assert!(!get(&s, "_alpha").exported);
    }

    #[test]
    fn python_nested_def_is_not_exported() {
        let s = py("def outer():\n    def alpha():\n        pass\n");
        assert!(!get(&s, "alpha").exported);
        assert!(get(&s, "outer").exported);
    }

    #[test]
    fn python_class_method_is_not_exported() {
        let s = py("class A:\n    def alpha(self):\n        pass\n");
        assert!(!get(&s, "alpha").exported);
        assert!(get(&s, "A").exported);
    }

    #[test]
    fn python_decorated_def_is_exported() {
        let s = py("@deco\ndef alpha():\n    pass\n");
        assert!(get(&s, "alpha").exported);
    }
}
