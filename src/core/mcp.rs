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
        let api_key = match std::env::var("OPENROUTER_API_KEY") {
            Ok(key) if !key.is_empty() => key,
            _ => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": "OPENROUTER_API_KEY environment variable not set"
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
        let stop_words = ["a","an","the","to","for","in","of","with","and","or","is","it","that","this","on","at","by","from","be","as","are","was","were","will","have","has","had","do","does","did","but","not","we","i","you","they","he","she","its"];
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
                "Module: {} | Layer: {} | Pattern: {}\n  Rules: {}",
                m.name,
                m.layer,
                m.path_pattern,
                m.rules.join("; ")
            )
        }).collect::<Vec<_>>().join("\n\n");

        let system_prompt = format!(
            "You are Archex, a senior software architect. You know this codebase:\n\n{}\n\nGenerate implementation plans as JSON only. No markdown. No explanation. Only valid JSON.",
            context
        );

        let user_prompt = format!(
            "Feature: {}\n\nRespond with this exact JSON structure:\n{{\n  \"feature\": \"...\",\n  \"summary\": \"one line description\",\n  \"modules_involved\": [\"module1\", \"module2\"],\n  \"steps\": [\n    {{\n      \"order\": 1,\n      \"action\": \"CREATE or MODIFY\",\n      \"file_path\": \"exact/path/from/codebase.ts\",\n      \"description\": \"what to implement\",\n      \"pattern_to_follow\": \"existing/similar/file.ts or null\",\n      \"rules\": [\"rule1\", \"rule2\"]\n    }}\n  ],\n  \"security_checklist\": [\n    \"Validate inputs with zod\",\n    \"Check auth before data access\"\n  ],\n  \"estimated_files\": 3\n}}",
            req.feature
        );

        // Call OpenRouter API
        let client = reqwest::Client::new();
        let response = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://archex.dev")
            .header("X-Title", "Archex")
            .json(&serde_json::json!({
                "model": "google/gemma-3-27b-it:free",
                "temperature": 0.3,
                "response_format": { "type": "json_object" },
                "messages": [
                    {"role": "system", "content": system_prompt},
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

                if let Some(content) = json.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|c| c.get("message")).and_then(|m| m.get("content")).and_then(|t| t.as_str()) {
                    // Try to parse as JSON
                    match serde_json::from_str::<serde_json::Value>(content) {
                        Ok(plan) => {
                            return Ok(CallToolResult::success(vec![Content::text(
                                serde_json::to_string(&plan).unwrap()
                            )]));
                        }
                        Err(_) => {
                            // Return with warning
                            return Ok(CallToolResult::success(vec![Content::text(
                                serde_json::to_string(&serde_json::json!({
                                    "warning": "could not parse as JSON",
                                    "raw": content
                                })).unwrap()
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