use crate::core::db::{Db, Import, Module, Symbol, SymbolType};
use glob::Pattern;
use std::path::{Path, PathBuf};
use std::fs;
use walkdir::WalkDir;
use serde::Serialize;
use regex::Regex;

pub struct Parser {
    db: Db,
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

const SKIP_DIRS: &[&str] = &["node_modules", ".next", ".git", "dist", "build", "target"];

const VALID_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "rs", "py"];

impl Parser {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn scan(&self, project_root: &Path) -> Result<ScanResult, Box<dyn std::error::Error>> {
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

        let project_root_str = project_root.to_string_lossy().to_lowercase();

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

            let relative_path = if file_path_str.to_lowercase().starts_with(&project_root_str) {
                file_path_str[project_root_str.len()..].trim_start_matches(['/', '\\'])
            } else {
                file_path_str.as_ref()
            }.to_string();

            let relative_path = relative_path.replace('\\', "/");

            if relative_path.is_empty() {
                continue;
            }

            let matched_module = compiled_patterns
                .iter()
                .find(|(_, pattern)| pattern.matches(&relative_path));

            if let Some((module, _)) = matched_module {
                self.db.upsert_file(&relative_path, module.id)?;
                files_mapped += 1;

                if let Ok(content) = fs::read_to_string(file_path) {
                    let (syms, imps) = Self::extract_symbols_and_imports(
                        &relative_path,
                        module.id,
                        file_path,
                        &content,
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
    ) -> (Vec<Symbol>, Vec<Import>) {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match ext {
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
        }
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
                    let full_text = node.utf8_text(content.as_bytes()).unwrap_or_default();
                    let signature = Self::extract_ts_function_signature(&full_text);
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

    fn extract_ts_import_path(node: &tree_sitter::Node, content: &str) -> String {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                let kind = child.kind();
                if kind == "string" {
                    let text = child.utf8_text(content.as_bytes()).unwrap_or_default();
                    if text.starts_with('"') {
                        return text[1..text.len()-1].to_string();
                    }
                }
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

    fn extract_ts_function_signature(node_text: &str) -> String {
        if node_text.is_empty() {
            return "()".to_string();
        }
        
        let params_start = node_text.find('(');
        let params_end = node_text.find(')');
        
        if params_start.is_none() || params_end.is_none() {
            return "()".to_string();
        }
        
        let params_text = &node_text[params_start.unwrap()+1..params_end.unwrap()];
        
        let return_start = node_text.find("): ");
        let return_type = if let Some(pos) = return_start {
            let rt = node_text[pos+3..].trim();
            if rt.is_empty() { "" } else { rt }
        } else {
            ""
        };
        
        let mut param_list = Vec::new();
        let mut depth = 0;
        let mut current = String::new();
        
        for ch in params_text.chars() {
            if ch == '(' { depth += 1; current.push(ch); }
            else if ch == ')' && depth > 0 { depth -= 1; current.push(ch); }
            else if ch == ',' && depth == 0 {
                param_list.push(current.trim().to_string());
                current = String::new();
            } else {
                current.push(ch);
            }
        }
        
        if !current.trim().is_empty() {
            param_list.push(current.trim().to_string());
        }
        
        let formatted: Vec<String> = param_list.into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        
        if formatted.is_empty() {
            if return_type.is_empty() { "()".to_string() } 
            else { format!("() => {}", return_type) }
        } else {
            if return_type.is_empty() { 
                format!("({})", formatted.join(", ")) 
            } else { 
                format!("({}) => {}", formatted.join(", "), return_type) 
            }
        }
    }

    fn get_function_signature(node: &tree_sitter::Node, content: &str) -> String {
        let full_text = node.utf8_text(content.as_bytes()).unwrap_or_default();
        Self::extract_ts_function_signature(&full_text)
    }

    fn extract_ts_import(node: &tree_sitter::Node, content: &str) -> Option<Import> {
        let mut imported_from = String::new();
        let mut imported_names = Vec::new();
        
        fn collect_import_children(node: &tree_sitter::Node, content: &str, names: &mut Vec<String>) {
            let kind = node.kind();
            
            if kind == "string" {
                let text = node.utf8_text(content.as_bytes()).unwrap_or_default();
                if text.starts_with('"') || text.starts_with('\'') {
                    if let Some(parent) = node.parent() {
                        if parent.kind() == "string" && parent.child_count() == 1 {
                            return;
                        }
                    }
                }
            }
            
            if kind == "identifier" || kind == "property_identifier" {
                let text = node.utf8_text(content.as_bytes()).unwrap_or_default();
                if !text.is_empty() && !text.starts_with('"') && !text.starts_with('\'') {
                    names.push(text.to_string());
                }
            } else if kind == "namespace_import" {
                if let Some(name) = node.child_by_field_name("name") {
                    let text = name.utf8_text(content.as_bytes()).unwrap_or_default();
                    if !text.is_empty() {
                        names.push(text.to_string());
                    }
                }
            } else if kind == "named_imports" {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        collect_import_children(&child, content, names);
                    }
                }
            } else if kind == "import_specifier" {
                if let Some(name) = node.child_by_field_name("name") {
                    let text = name.utf8_text(content.as_bytes()).unwrap_or_default();
                    if !text.is_empty() {
                        names.push(text.to_string());
                    }
                }
                if let Some(alias) = node.child_by_field_name("alias") {
                    let text = alias.utf8_text(content.as_bytes()).unwrap_or_default();
                    if !text.is_empty() {
                        names.push(text.to_string());
                    }
                }
            }
            
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    collect_import_children(&child, content, names);
                }
            }
        }
        
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                let kind = child.kind();
                
                if kind == "string" {
                    let text = child.utf8_text(content.as_bytes()).unwrap_or_default();
                    if text.starts_with('"') && !imported_from.is_empty() == false {
                        imported_from = text[1..text.len()-1].to_string();
                    }
                } else if kind == "module" {
                    let text = child.utf8_text(content.as_bytes()).unwrap_or_default();
                    if !text.is_empty() && !text.starts_with('"') {
                        imported_from = text.to_string();
                    }
                } else if kind == "named_imports" {
                    collect_import_children(&child, content, &mut imported_names);
                } else if kind == "namespace_import" {
                    collect_import_children(&child, content, &mut imported_names);
                } else if kind == "default_import" {
                    let text = child.utf8_text(content.as_bytes()).unwrap_or_default();
                    if !text.is_empty() {
                        imported_names.push(text.to_string());
                    }
                }
            }
        }
        
        if imported_from.is_empty() && !imported_names.is_empty() {
            imported_from = "external".to_string();
        }
        
        if !imported_from.is_empty() || !imported_names.is_empty() {
            Some(Import {
                id: 0,
                file_path: String::new(),
                imported_from,
                imported_names,
                module_id: 0,
            })
        } else {
            None
        }
    }

    fn collect_imported_names(node: &tree_sitter::Node, content: &str, names: &mut Vec<String>) {
        let kind = node.kind();
        
        if kind == "identifier" || kind == "property_identifier" {
            names.push(Self::get_node_text(node, content));
            return;
        }
        
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                Self::collect_imported_names(&child, content, names);
            }
        }
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

    fn is_exported(node: &tree_sitter::Node, _content: &str) -> bool {
        if let Some(prev) = node.prev_sibling() {
            let kind = prev.kind();
            kind == "export_statement" || kind == "declare_export_statement"
        } else {
            false
        }
    }

    fn is_python_exported(node: &tree_sitter::Node, _content: &str) -> bool {
        if let Some(prev) = node.prev_sibling() {
            let kind = prev.kind();
            kind == "future_import_statement"
        } else {
            false
        }
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