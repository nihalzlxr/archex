use rmcp::{ServerHandler, ServiceExt, model::*, schemars, tool, transport::stdio};
use serde::Deserialize;
use crate::core::db::{Db, SymbolType};
use crate::core::parser::{Parser, DriftViolation, DriftResult};
use std::path::Path;
use regex::Regex;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetContextRequest {
    #[schemars(description = "Relative file path from project root")]
    pub file_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetModuleRequest {
    #[schemars(description = "Module name e.g. api, services, jobs")]
    pub module_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreatePlanRequest {
    #[schemars(description = "Feature description in plain English")]
    pub feature: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindSimilarRequest {
    #[schemars(description = "Description of what you're looking for (e.g. 'auth function', 'user model')")]
    pub description: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckDriftRequest {
    #[schemars(description = "Relative file path to check for drift")]
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub struct ArchexService;

#[tool(tool_box)]
impl ArchexService {
    #[tool(description = "Get architecture context, module, layer and rules for a file")]
    async fn get_context(
        &self,
        #[tool(aggr)] req: GetContextRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        let db_path = Path::new(".archex/db.sqlite");
        
        let db = match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "found": false,
                        "message": format!("Database not found. Run 'archex init' first. Error: {}", e)
                    })).unwrap()
                )]));
            }
        };
        
        match db.get_context_for_file(&req.file_path) {
            Ok(Some(ctx)) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "found": true,
                    "file": req.file_path,
                    "module": ctx.module_name,
                    "layer": ctx.layer,
                    "rules": ctx.rules
                })).unwrap()
            )])),
            Ok(None) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "found": false,
                    "message": format!("File '{}' not mapped. Run 'archex init' to scan project.", req.file_path)
                })).unwrap()
            )])),
            Err(e) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "found": false,
                    "message": format!("Error: {}", e)
                })).unwrap()
            )]))
        }
    }

    #[tool(description = "Get module info: layer, files, and rules")]
    async fn get_module(
        &self,
        #[tool(aggr)] req: GetModuleRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        let db_path = Path::new(".archex/db.sqlite");
        
        let db = match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "found": false,
                        "message": format!("Database not found. Run 'archex init' first. Error: {}", e)
                    })).unwrap()
                )]));
            }
        };
        
        match db.get_module_info(&req.module_name) {
            Ok(Some(info)) => {
                let rules: Vec<serde_json::Value> = info.rules.iter().map(|r| {
                    serde_json::json!({
                        "type": match r.rule_type {
                            crate::core::db::RuleType::Forbidden => "forbidden",
                            crate::core::db::RuleType::Required => "required",
                            crate::core::db::RuleType::Warning => "warning",
                        },
                        "description": r.description,
                        "pattern": r.pattern
                    })
                }).collect();

                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "found": true,
                        "module": info.name,
                        "layer": info.layer,
                        "path_pattern": info.path_pattern,
                        "file_count": info.file_count,
                        "files": info.files,
                        "rules": rules
                    })).unwrap()
                )]))
            }
            Ok(None) => {
                let all_names = db.get_all_module_names().unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "found": false,
                        "message": format!("Module '{}' not found. Available: {}", req.module_name, all_names.join(", "))
                    })).unwrap()
                )]))
            }
            Err(e) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "found": false,
                    "message": format!("Error: {}", e)
                })).unwrap()
            )]))
        }
    }

    #[tool(description = "Generate implementation plan for a feature using AI")]
    async fn create_plan(
        &self,
        #[tool(aggr)] req: CreatePlanRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        let db_path = Path::new(".archex/db.sqlite");
        let db = match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("Database not found. Run 'archex init' first. Error: {}", e)
                    })).unwrap()
                )]));
            }
        };

        let stop_words = ["a","an","the","to","for","in","of","with","and","or","is","it","that","this","on","at","by","from","be","as","are","was","were","will","have","has","had","do","does","did","but","not","we","i","you","they","he","she","its"];
        let keywords: Vec<String> = req.feature
            .split_whitespace()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty() && !stop_words.iter().any(|w| w == s))
            .collect();

        let modules = match db.find_relevant_modules(&keywords) {
            Ok(m) => m,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("Failed to find modules: {}", e)
                    })).unwrap()
                )]));
            }
        };

        let mut relevant_modules = Vec::new();
        let mut similar_files: Vec<String> = Vec::new();
        let mut module_names: Vec<String> = Vec::new();

        for m in &modules {
            if let Ok(Some(info)) = db.get_module_info(&m.name) {
                let example_files: Vec<String> = info.files.iter().take(8).cloned().collect();
                similar_files.extend(example_files.clone());
                module_names.push(info.name.clone());

                let rule_descriptions: Vec<String> = info.rules.iter().map(|r| r.description.clone()).collect();

                relevant_modules.push(serde_json::json!({
                    "name": info.name,
                    "layer": info.layer,
                    "path_pattern": info.path_pattern,
                    "example_files": example_files,
                    "rules": rule_descriptions
                }));
            }
        }

        for keyword in &keywords {
            if let Ok(files) = db.search_files(keyword) {
                for f in files.iter().take(5) {
                    if !similar_files.contains(f) {
                        similar_files.push(f.clone());
                    }
                }
            }
        }

        let existing_symbols: Vec<serde_json::Value> = {
            let mut symbols = Vec::new();
            for keyword in &keywords {
                if let Ok(results) = db.search_symbols(keyword) {
                    for sr in results.into_iter().take(5) {
                        symbols.push(serde_json::json!({
                            "name": sr.name,
                            "signature": sr.signature,
                            "file_path": sr.file_path,
                            "module": sr.module_name,
                            "symbol_type": match sr.symbol_type {
                                SymbolType::Function => "function",
                                SymbolType::Class => "class",
                                SymbolType::Struct => "struct",
                                SymbolType::Enum => "enum",
                                SymbolType::Interface => "interface",
                                SymbolType::Route => "route",
                            }
                        }));
                    }
                }
            }
            symbols
        };

        let files_to_touch: Vec<serde_json::Value> = {
            let mut files_info = Vec::new();
            for file_path in similar_files.iter().take(10) {
                if let Ok(symbols) = db.get_symbols_by_file(file_path) {
                    let symbol_names: Vec<String> = symbols.iter().map(|s| {
                        let sig = s.signature.as_deref().unwrap_or("");
                        format!("{}{}", s.name, sig)
                    }).collect();
                    
                    files_info.push(serde_json::json!({
                        "file_path": file_path,
                        "symbols": symbol_names
                    }));
                }
            }
            files_info
        };

        let files_to_avoid: Vec<serde_json::Value> = {
            let mut avoid = Vec::new();
            if let Some(first_module) = module_names.first() {
                if let Ok(adjacent) = db.find_adjacent_modules(first_module) {
                    for adj_name in adjacent.iter().take(5) {
                        if let Ok(Some(info)) = db.get_module_info(adj_name) {
                            let forbidden: Vec<String> = info.rules.iter()
                                .filter(|r| matches!(r.rule_type, crate::core::db::RuleType::Forbidden))
                                .map(|r| r.description.clone())
                                .collect();
                            
                            if !forbidden.is_empty() {
                                avoid.push(serde_json::json!({
                                    "module": info.name,
                                    "layer": info.layer,
                                    "forbidden_crosses": forbidden
                                }));
                            }
                        }
                    }
                }
            }
            avoid
        };

        let past_decisions: Vec<serde_json::Value> = {
            if let Ok(decisions) = db.search_decisions(&keywords) {
                decisions.into_iter().map(|(title, context, decision)| {
                    serde_json::json!({
                        "title": title,
                        "context": context,
                        "decision": decision
                    })
                }).collect()
            } else {
                Vec::new()
            }
        };

        let result = serde_json::json!({
            "feature": req.feature,
            "relevant_modules": relevant_modules,
            "existing_symbols": existing_symbols,
            "files_to_touch": files_to_touch,
            "files_to_avoid": files_to_avoid,
            "rules": [
                "Follow existing patterns in similar files",
                "Reuse existing symbols where possible",
                "No direct DB queries - use services layer"
            ],
            "security_checklist": [
                "Input validation with zod",
                "Auth check before data access", 
                "No hardcoded secrets",
                "Error boundaries set"
            ],
            "past_decisions": past_decisions,
            "instruction": "You are a senior developer. Implement this feature by reusing existing_symbols and following patterns in files_to_touch. Avoid files in files_to_avoid. Output a step-by-step plan with exact file paths."
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap()
        )]))
    }

    #[tool(description = "Find similar symbols to prevent reinventing existing code")]
    async fn find_similar(
        &self,
        #[tool(aggr)] req: FindSimilarRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        let db_path = Path::new(".archex/db.sqlite");
        let db = match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "found": false,
                        "message": format!("Database not found. Run 'archex init' first. Error: {}", e)
                    })).unwrap()
                )]));
            }
        };

        let stop_words = ["a","an","the","to","for","in","of","with","and","or","is","it","that","this","on","at","by","from","be","as","are","was","were","will","have","has","had","do","does","did","but","not","we","i","you","they","he","she","its"];
        let keywords: Vec<String> = req.description
            .split_whitespace()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty() && !stop_words.iter().any(|w| w == s))
            .collect();

        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for keyword in &keywords {
            if let Ok(symbols) = db.search_symbols(keyword) {
                for sr in symbols {
                    let key = format!("{}:{}", sr.file_path, sr.name);
                    if !seen.contains(&key) {
                        seen.insert(key);
                        results.push(serde_json::json!({
                            "name": sr.name,
                            "signature": sr.signature,
                            "file_path": sr.file_path,
                            "module_name": sr.module_name,
                            "symbol_type": match sr.symbol_type {
                                SymbolType::Function => "function",
                                SymbolType::Class => "class",
                                SymbolType::Struct => "struct",
                                SymbolType::Enum => "enum",
                                SymbolType::Interface => "interface",
                                SymbolType::Route => "route",
                            }
                        }));
                    }
                }
            }
        }

        let results_len = results.len();
        let results = results.into_iter().take(5).collect::<Vec<_>>();

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&serde_json::json!({
                "found": results_len > 0,
                "count": results_len,
                "matches": results,
                "search_terms": keywords
            })).unwrap()
        )]))
    }

    #[tool(description = "Check a file for architecture drift - pattern violations against module rules")]
    async fn check_drift(
        &self,
        #[tool(aggr)] req: CheckDriftRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        let db_path = Path::new(".archex/db.sqlite");
        let db = match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("Database not found. Run 'archex init' first. Error: {}", e)
                    })).unwrap()
                )]));
            }
        };

        let project_root = Path::new(".");
        
        let full_path = if req.file_path.starts_with('/') || req.file_path.contains(':') {
            Path::new(&req.file_path).to_path_buf()
        } else {
            project_root.join(&req.file_path)
        };

        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("Could not read file: {}", e)
                    })).unwrap()
                )]));
            }
        };

        let module_info = db.get_module_for_file(&req.file_path).ok().and_then(|m| m);
        let module_name = module_info.as_ref().map(|(n, _)| n.clone()).unwrap_or_default();

        let rules = if let Some((_, module_id)) = module_info {
            db.get_rules_for_module(module_id).unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut drift_result = crate::core::parser::Parser::check_drift(&req.file_path, &content);
        drift_result.module = module_name.clone();

        for rule in &rules {
            let rule_type_str = match rule.rule_type {
                crate::core::db::RuleType::Forbidden => "forbidden",
                crate::core::db::RuleType::Required => "required",
                crate::core::db::RuleType::Warning => "warning",
            };

            if let Some(pattern) = &rule.pattern {
                let pattern_lower = pattern.to_lowercase();
                let content_lower = content.to_lowercase();

                match rule.rule_type {
                    crate::core::db::RuleType::Forbidden => {
                        let regex_result = Regex::new(pattern);
                        let matches = if let Ok(re) = regex_result {
                            re.is_match(&content_lower)
                        } else {
                            content_lower.contains(&pattern_lower)
                        };

                        if matches {
                            drift_result.violations.push(
                                crate::core::parser::DriftViolation {
                                    rule_type: rule_type_str.to_string(),
                                    rule_description: rule.description.clone(),
                                    pattern: Some(pattern.clone()),
                                    line_number: None,
                                    suggestion: format!("Remove or refactor matching pattern"),
                                }
                            );
                        }
                    }
                    crate::core::db::RuleType::Required => {
                        let regex_result = Regex::new(pattern);
                        let has_pattern = if let Ok(re) = regex_result {
                            re.is_match(&content_lower)
                        } else {
                            content_lower.contains(&pattern_lower)
                        };

                        if !has_pattern {
                            drift_result.violations.push(
                                crate::core::parser::DriftViolation {
                                    rule_type: rule_type_str.to_string(),
                                    rule_description: rule.description.clone(),
                                    pattern: Some(pattern.clone()),
                                    line_number: None,
                                    suggestion: format!("Add required pattern: {}", pattern),
                                }
                            );
                        }
                    }
                    crate::core::db::RuleType::Warning => {
                        let regex_result = Regex::new(pattern);
                        let matches = if let Ok(re) = regex_result {
                            re.is_match(&content_lower)
                        } else {
                            content_lower.contains(&pattern_lower)
                        };

                        if matches {
                            drift_result.violations.push(
                                crate::core::parser::DriftViolation {
                                    rule_type: rule_type_str.to_string(),
                                    rule_description: rule.description.clone(),
                                    pattern: Some(pattern.clone()),
                                    line_number: None,
                                    suggestion: "Consider refactoring".to_string(),
                                }
                            );
                        }
                    }
                }
            }
        }

        let result_json = serde_json::json!({
            "file_path": drift_result.file_path,
            "module": drift_result.module,
            "violations": drift_result.violations.iter().map(|v| {
                serde_json::json!({
                    "rule_type": v.rule_type,
                    "rule_description": v.rule_description,
                    "pattern": v.pattern,
                    "line_number": v.line_number,
                    "suggestion": v.suggestion
                })
            }).collect::<Vec<_>>(),
            "clean": drift_result.clean
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result_json).unwrap()
        )]))
    }
}

#[tool(tool_box)]
impl ServerHandler for ArchexService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("Archex: get architecture context for files in this project.".to_string()),
        }
    }
}

pub async fn start_server() -> anyhow::Result<()> {
    let service = ArchexService;
    let server = service.serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}