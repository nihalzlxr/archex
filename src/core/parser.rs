use crate::core::db::{Db, Import, ImportKind, Module, Symbol, SymbolType};
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

        // Persist coverage: `check` runs in CI against a fresh index and must
        // still be able to say how much of the tree it never looked at.
        let _ = self.db.set_scan_stats(files_scanned, files_mapped);

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
                    // `import type { X } from './y'` compiles to nothing, so it
                    // cannot create runtime coupling. Everything else is
                    // classified by the shape of the specifier.
                    let kind = if Self::has_type_marker(node) {
                        ImportKind::TypeOnly
                    } else {
                        Self::classify_specifier(&imported_from)
                    };
                    imports.push(Import {
                        id: 0,
                        file_path: file_path.to_string(),
                        imported_from,
                        imported_names: names,
                        module_id,
                        resolved_to: None,
                        line_number: node.start_position().row as i64 + 1,
                        kind,
                    });
                }
            }
            // `export { x } from './a'` and `export * from './b'` are edges.
            "export_statement" => {
                if let Some(source) = node.child_by_field_name("source") {
                    let imported_from = Self::string_literal_value(&source, content);
                    if !imported_from.is_empty() {
                        let kind = if Self::has_type_marker(node) {
                            ImportKind::TypeOnly
                        } else {
                            Self::classify_specifier(&imported_from)
                        };
                        imports.push(Import {
                            id: 0,
                            file_path: file_path.to_string(),
                            imported_from,
                            imported_names: Vec::new(),
                            module_id,
                            resolved_to: None,
                            line_number: node.start_position().row as i64 + 1,
                            kind,
                        });
                    }
                }
            }
            // require('./x') and await import('./x') are edges too.
            "call_expression" => {
                if let Some(specifier) = Self::require_or_dynamic_import(node, content) {
                    imports.push(Import {
                        id: 0,
                        file_path: file_path.to_string(),
                        imported_from: specifier,
                        imported_names: Vec::new(),
                        module_id,
                        resolved_to: None,
                        line_number: node.start_position().row as i64 + 1,
                        // Deferred is not detached: a dynamic import is a
                        // runtime edge, so it counts like any other.
                        kind: ImportKind::Dynamic,
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

    /// `import type { X } from './y'` and `export type { X } from './y'` put a
    /// `type` keyword directly after the import/export keyword. Checked
    /// against the grammar rather than assumed: tree-sitter-typescript emits
    /// it as a plain `type` node, so looking for an `import_type` node matches
    /// nothing and every type-only import reads as a runtime edge.
    fn has_type_marker(node: &tree_sitter::Node) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "type" | "import_type" | "export_type" => return true,
                // Only the marker immediately follows `import`/`export`;
                // `import { type X } from './y'` is not a type-only edge --
                // it still imports a runtime binding.
                _ => {}
            }
        }
        false
    }

    /// Specifier shape only. Relative and aliased both point at first-party
    /// code; a bare specifier is a package. Nothing here guesses whether the
    /// target is a layer -- that is a module decision, made in config.toml.
    fn classify_specifier(specifier: &str) -> ImportKind {
        if specifier.is_empty() {
            return ImportKind::Unknown;
        }
        if specifier.starts_with("./") || specifier.starts_with("../") {
            return ImportKind::Relative;
        }
        // `@/`, `~/`, `#` and bare absolute-looking prefixes are alias
        // conventions; whether they RESOLVE is the resolver's business.
        if specifier.starts_with('@') || specifier.starts_with('~') || specifier.starts_with('#') {
            return ImportKind::Aliased;
        }
        if specifier.starts_with('/') {
            return ImportKind::Aliased;
        }
        ImportKind::Package
    }

    /// The specifier of an import statement. TypeScript accepts single and
    /// double quotes and both appear in real repos; accepting only one drops
    /// every import written the other way, silently.
    /// The specifier of `require('x')` / `import('x')`, or None for any
    /// other call.
    fn require_or_dynamic_import(node: &tree_sitter::Node, content: &str) -> Option<String> {
        let function = node.child_by_field_name("function")?;
        let name = Self::get_node_text(&function, content);
        if name != "require" && name != "import" {
            return None;
        }
        let args = node.child_by_field_name("arguments")?;
        let first = args.named_child(0)?;
        let specifier = Self::string_literal_value(&first, content);
        if specifier.is_empty() {
            None
        } else {
            Some(specifier)
        }
    }

    /// Unquote a string node. TypeScript accepts single, double and backtick
    /// quotes; accepting only one silently drops imports written the other
    /// way.
    fn string_literal_value(node: &tree_sitter::Node, content: &str) -> String {
        if node.kind() != "string" {
            return String::new();
        }
        let text = node.utf8_text(content.as_bytes()).unwrap_or_default();
        let quote = match text.chars().next() {
            Some(q @ ('"' | '\'' | '`')) => q,
            _ => return String::new(),
        };
        let inner = text.trim_start_matches(quote);
        inner.strip_suffix(quote).unwrap_or(inner).replace("\\'", "'")
    }

    fn extract_ts_import_path(node: &tree_sitter::Node, content: &str) -> String {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                let value = Self::string_literal_value(&child, content);
                if !value.is_empty() {
                    return value;
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
            // Rust: crate-internal paths are first-party; anything else names a
            // crate. Whether the item is a type or a value is not worth a second
            // parse, and wrongly exempting an edge is worse than flagging one.
            let kind = if imported_from.starts_with("crate::")
                || imported_from.starts_with("super::")
                || imported_from.starts_with("self::")
            {
                ImportKind::Relative
            } else {
                ImportKind::Package
            };
            Some(Import {
                id: 0,
                file_path: String::new(),
                imported_from,
                imported_names,
                module_id: 0,
                resolved_to: None,
                line_number: node.start_position().row as i64 + 1,
                kind,
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
            // Python: a leading dot means relative; anything else is a module
            // path, first-party or not -- the resolver decides.
            let kind = if imported_from.starts_with('.') {
                ImportKind::Relative
            } else {
                ImportKind::Package
            };
            Some(Import {
                id: 0,
                file_path: String::new(),
                imported_from,
                imported_names,
                module_id: 0,
                resolved_to: None,
                line_number: node.start_position().row as i64 + 1,
                kind,
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


}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a TypeScript snippet and return (specifier, kind) per import edge.
    fn ts_import_kinds(src: &str) -> Vec<(String, ImportKind)> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        let tree = parser.parse(src, None).unwrap();

        fn walk(
            node: tree_sitter::Node,
            src: &str,
            out: &mut Vec<(String, ImportKind)>,
        ) {
            match node.kind() {
                "import_statement" => {
                    if let Some(spec) = child_string(node, src) {
                        let kind = if Parser::has_type_marker(&node) {
                            ImportKind::TypeOnly
                        } else {
                            Parser::classify_specifier(&spec)
                        };
                        out.push((spec, kind));
                    }
                }
                "export_statement" => {
                    if let Some(source) = node.child_by_field_name("source") {
                        let spec = unquote(
                            source.utf8_text(src.as_bytes()).unwrap_or_default(),
                        );
                        if let Some(spec) = spec {
                            let kind = if Parser::has_type_marker(&node) {
                                ImportKind::TypeOnly
                            } else {
                                Parser::classify_specifier(&spec)
                            };
                            out.push((spec, kind));
                        }
                    }
                }
                "call_expression" => {
                    let text = node.utf8_text(src.as_bytes()).unwrap_or_default().to_string();
                    if text.starts_with("require(") || text.starts_with("import(") {
                        // The specifier is the quoted argument, not the whole call.
                        let arg = text
                            .split_once('(')
                            .and_then(|(_, rest)| rest.rsplit_once(')').map(|(a, _)| a));
                        if let Some(spec) = arg.and_then(unquote) {
                            out.push((spec, ImportKind::Dynamic));
                        }
                    }
                }
                _ => {}
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(child, src, out);
            }
        }

        let mut out = Vec::new();
        walk(tree.root_node(), src, &mut out);
        out
    }

    fn child_string(node: tree_sitter::Node, src: &str) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "string" {
                return unquote(&child.utf8_text(src.as_bytes()).unwrap_or_default());
            }
        }
        None
    }

    fn unquote(text: &str) -> Option<String> {
        let t = text.trim().trim_matches(|c| c == '\'' || c == '"').trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }

    #[test]
    fn classifies_the_five_kinds() {
        let kinds = ts_import_kinds(
            "import { a } from './rel';\n\
             import { b } from '@/aliased';\n\
             import { c } from 'some-pkg';\n\
             import type { D } from '../types';\n\
             export async function go() { return import('./lazy'); }\n",
        );
        assert_eq!(
            kinds,
            vec![
                ("./rel".to_string(), ImportKind::Relative),
                ("@/aliased".to_string(), ImportKind::Aliased),
                ("some-pkg".to_string(), ImportKind::Package),
                ("../types".to_string(), ImportKind::TypeOnly),
                ("./lazy".to_string(), ImportKind::Dynamic),
            ],
        );
    }

    /// The grammar-shape trap: tree-sitter-typescript emits a bare `type`
    /// node, so a detector looking for `import_type` matches nothing and every
    /// type-only import is read as a runtime edge.
    #[test]
    fn type_only_marker_is_a_bare_type_node() {
        assert_eq!(
            ts_import_kinds("import type { Row } from './row';"),
            vec![("./row".to_string(), ImportKind::TypeOnly)]
        );
        assert_eq!(
            ts_import_kinds("import type Row from './row';"),
            vec![("./row".to_string(), ImportKind::TypeOnly)]
        );
        assert_eq!(
            ts_import_kinds("export type { Row } from './row';"),
            vec![("./row".to_string(), ImportKind::TypeOnly)]
        );
    }

    /// `import { type X }` still imports a runtime binding; only a
    /// statement-level `type` marker makes the whole edge type-only.
    #[test]
    fn inline_type_specifier_is_not_a_type_only_edge() {
        let kinds = ts_import_kinds("import { type X, y } from './m';");
        assert_eq!(kinds, vec![("./m".to_string(), ImportKind::Relative)]);
    }

    /// Deferred is not detached: a dynamic import is runtime coupling.
    #[test]
    fn dynamic_imports_are_runtime_edges() {
        assert_eq!(
            ts_import_kinds("const m = await import('./x');"),
            vec![("./x".to_string(), ImportKind::Dynamic)]
        );
        assert_eq!(
            ts_import_kinds("const y = require('./y');"),
            vec![("./y".to_string(), ImportKind::Dynamic)]
        );
    }

    /// Alias conventions that are not `./` and are not package names must not
    /// be classified as packages — they resolve to first-party code.
    #[test]
    fn alias_conventions_classify_as_aliased() {
        for spec in ["@/lib/x", "~/lib/x", "#internal", "/abs/path"] {
            assert_eq!(
                Parser::classify_specifier(spec),
                ImportKind::Aliased,
                "{} should be aliased",
                spec
            );
        }
    }

    /// Defect 3: `is_route_file` was handed the ABSOLUTE path, so a repo
    /// living under any `app/` directory had ordinary files retyped as
    /// routes. The `&&`/`||` precedence made it worse: `/app/api/` matched
    /// on its own, ignoring the filename clause, so every helper under an
    /// api directory became a route.
    #[test]
    fn a_path_above_the_repo_root_cannot_decide_what_a_file_is() {
        // A repo that happens to live under /app/ — the shape that produced
        // bogus route symbols. Nothing in `src/lib` is a route file.
        assert!(
            !Parser::is_route_file(std::path::Path::new(
                "/home/dev/app/lorb/src/lib/page.ts"
            )),
            "a file outside src/app cannot become a route because of where \
             the repository lives"
        );
    }

    #[test]
    fn only_page_and_route_files_are_routes() {
        for path in [
            "src/app/api/route.ts",
            "src/app/api/route.tsx",
            "src/app/dashboard/page.tsx",
            "src/pages/api/users.ts",
        ] {
            assert!(
                Parser::is_route_file(std::path::Path::new(path)),
                "{} is a route file",
                path
            );
        }

        // The precedence bug: `/app/api/` matched on its own, so a plain
        // helper next to the routes was typed as one.
        for path in [
            "src/app/api/handlers.ts",
            "src/app/api/schemas.ts",
            "src/app/api/helpers/thing.ts",
        ] {
            assert!(
                !Parser::is_route_file(std::path::Path::new(path)),
                "{} is a helper, not a route file",
                path
            );
        }
    }
}
