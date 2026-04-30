use crate::core::db::{Db, RuleType};
use rmcp::{ServerHandler, ServiceExt, tool, Error as McpError};
use rmcp::model::{CallToolResult, Content};
use serde_json::{json, Value};
use std::path::Path;

const DB_PATH: &str = ".archex/db.sqlite";

#[derive(Clone)]
pub struct ArchexService;

#[rmcp::tool(tool_box)]
impl ArchexService {
    #[tool(description = "Get architecture context for a file")]
    async fn get_context(
        &self,
        #[tool(param)]
        #[schemars(description = "Relative file path from project root")]
        file_path: String,
    ) -> Result<CallToolResult, McpError> {
        let db = match Db::open(Path::new(DB_PATH)) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&json!({
                        "found": false,
                        "message": format!("Failed to open database: {}", e)
                    })).unwrap()
                )]));
            }
        };

        match db.get_context_for_file(&file_path) {
            Ok(Some(ctx)) => {
                let rules: Vec<Value> = ctx
                    .rules
                    .iter()
                    .map(|r| {
                        json!({
                            "rule_type": match r.rule_type {
                                RuleType::Forbidden => "forbidden",
                                RuleType::Required => "required",
                                RuleType::Warning => "warning",
                            },
                            "description": r.description,
                            "pattern": r.pattern
                        })
                    })
                    .collect();

                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&json!({
                        "found": true,
                        "file": file_path,
                        "module": ctx.module_name,
                        "layer": ctx.layer,
                        "rules": rules
                    })).unwrap()
                )]))
            }
            Ok(None) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&json!({
                    "found": false,
                    "message": "File not mapped. Run archex init."
                })).unwrap()
            )])),
            Err(e) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&json!({
                    "found": false,
                    "message": format!("Database error: {}", e)
                })).unwrap()
            )])),
        }
    }
}

#[rmcp::tool(tool_box)]
impl ServerHandler for ArchexService {}

pub async fn start_server() -> Result<(), Box<dyn std::error::Error>> {
    let service = ArchexService.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}