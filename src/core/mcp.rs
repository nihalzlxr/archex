use rmcp::{ServerHandler, ServiceExt, model::*, schemars, tool, transport::stdio};
use serde::Deserialize;
use crate::core::db::Db;
use std::path::Path;

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

        for m in &modules {
            if let Ok(Some(info)) = db.get_module_info(&m.name) {
                let example_files: Vec<String> = info.files.iter().take(8).cloned().collect();
                similar_files.extend(example_files.clone());

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

        let result = serde_json::json!({
            "feature": req.feature,
            "context_for_agent": {
                "relevant_modules": relevant_modules,
                "similar_existing_files": similar_files.iter().take(10).cloned().collect::<Vec<_>>(),
                "suggested_new_files": [],
                "rules_to_enforce": [
                    "Follow existing patterns in similar_existing_files",
                    "No direct DB queries - use services layer",
                    "Validate all inputs with zod",
                    "Check auth before data access"
                ],
                "security_checklist": [
                    "Input validation with zod",
                    "Auth check before data access",
                    "No hardcoded secrets",
                    "Error boundaries set"
                ]
            },
            "instruction": "You are a senior developer. Use the context above to generate a step-by-step implementation plan with exact file paths, following existing patterns."
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result).unwrap()
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