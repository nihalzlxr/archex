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
        let api_key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(key) if !key.is_empty() => key,
            _ => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": "ANTHROPIC_API_KEY environment variable not set"
                    })).unwrap()
                )]));
            }
        };

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

        // Extract keywords from feature
        let stop_words = ["a", "an", "the", "to", "for", "in", "of", "with", "and", "or", "is", "are", "be", "have", "has", "that", "this", "it"];
        let keywords: Vec<String> = req.feature
            .split_whitespace()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty() && !stop_words.iter().any(|w| w == s))
            .collect();

        // Find relevant modules
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

        // Build context string
        let context = modules.iter().map(|m| {
            format!(
                "Module: {} (layer: {}, pattern: {})\n  Rules: {}\n",
                m.name,
                m.layer,
                m.path_pattern,
                m.rules.join("; ")
            )
        }).collect::<Vec<_>>().join("\n");

        let system_prompt = format!(
            "You are Archex, a senior software architect. You know this codebase structure:\n\n{}\n\nYou must generate a precise implementation plan for the requested feature. Always output valid JSON only, no markdown, no explanation.",
            context
        );

        let user_prompt = format!(
            "Feature request: {}\n\nGenerate a plan with this exact JSON structure:\n{{\n  \"feature\": \"...\",\n  \"summary\": \"one line description\",\n  \"modules_involved\": [\"api\", \"services\", \"jobs\"],\n  \"steps\": [\n    {{\n      \"order\": 1,\n      \"action\": \"CREATE|MODIFY|DELETE\",\n      \"file_path\": \"src/services/fee-reminder.ts\",\n      \"description\": \"what to implement in this file\",\n      \"pattern_to_follow\": \"src/services/fee.ts\",\n      \"rules\": [\"no direct DB access\", \"validate inputs with zod\"]\n    }}\n  ],\n  \"security_checklist\": [\n    \"Validate all inputs with zod\",\n    \"Check authentication before data access\",\n    \"No secrets hardcoded\"\n  ],\n  \"estimated_files\": 4\n}}",
            req.feature
        );

        // Call Anthropic API
        let client = reqwest::Client::new();
        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": "claude-sonnet-4-20250514",
                "max_tokens": 4000,
                "system": system_prompt,
                "messages": [
                    {"role": "user", "content": user_prompt}
                ]
            }))
            .send()
            .await;

        match response {
            Ok(resp) => {
                let json: serde_json::Value = match resp.json().await {
                    Ok(j) => j,
                    Err(e) => {
                        return Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string(&serde_json::json!({
                                "error": format!("Failed to parse API response: {}", e)
                            })).unwrap()
                        )]));
                    }
                };

                if let Some(content) = json.get("content").and_then(|c| c.as_array()).and_then(|a| a.first()) {
                    if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                        // Try to extract valid JSON from the response
                        if let Ok(plan) = serde_json::from_str::<serde_json::Value>(text) {
                            return Ok(CallToolResult::success(vec![Content::text(
                                serde_json::to_string(&plan).unwrap()
                            )]));
                        }
                    }
                }

                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&json).unwrap()
                )]))
            }
            Err(e) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "error": format!("API call failed: {}", e)
                })).unwrap()
            )]))
        }
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