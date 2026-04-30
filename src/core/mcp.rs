use crate::core::db::{Db, RuleType};
use std::future::Future;
use rmcp::{
    handler::server::tool::ToolRouter,
    model::{CallToolResult, Content, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_router, ErrorData as McpError, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use std::path::Path;

const DB_PATH: &str = ".archex/db.sqlite";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetContextInput {
    pub file_path: String,
}

#[derive(Debug, Serialize)]
pub struct GetContextOutput {
    pub found: bool,
    pub message: Option<String>,
    pub file: Option<String>,
    pub module: Option<String>,
    pub layer: Option<String>,
    pub rules: Option<Vec<RuleOutput>>,
}

#[derive(Debug, Serialize)]
pub struct RuleOutput {
    pub rule_type: String,
    pub description: String,
    pub pattern: Option<String>,
}

#[derive(Clone)]
pub struct ArchexService {
    tool_router: ToolRouter<Self>,
}

impl ServerHandler for ArchexService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            instructions: Some(
                "Archex provides context about file module mappings. Use get_context with a file path to see which module a file belongs to and its rules.".to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tool_router]
impl ArchexService {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get context for a file: its module, layer, and rules")]
    async fn get_context(
        &self,
        params: rmcp::handler::server::tool::Parameters<GetContextInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let db = match Db::open(Path::new(DB_PATH)) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&GetContextOutput {
                        found: false,
                        message: Some(format!("Failed to open database: {}", e)),
                        file: None,
                        module: None,
                        layer: None,
                        rules: None,
                    })
                    .unwrap(),
                )]));
            }
        };

        match db.get_context_for_file(&input.file_path) {
            Ok(Some(ctx)) => {
                let rules: Vec<RuleOutput> = ctx
                    .rules
                    .iter()
                    .map(|r| RuleOutput {
                        rule_type: match r.rule_type {
                            RuleType::Forbidden => "forbidden".to_string(),
                            RuleType::Required => "required".to_string(),
                            RuleType::Warning => "warning".to_string(),
                        },
                        description: r.description.clone(),
                        pattern: r.pattern.clone(),
                    })
                    .collect();

                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&GetContextOutput {
                        found: true,
                        message: None,
                        file: Some(input.file_path),
                        module: Some(ctx.module_name),
                        layer: Some(ctx.layer),
                        rules: Some(rules),
                    })
                    .unwrap(),
                )]))
            }
            Ok(None) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&GetContextOutput {
                    found: false,
                    message: Some("File not mapped. Run archex init.".to_string()),
                    file: None,
                    module: None,
                    layer: None,
                    rules: None,
                })
                .unwrap(),
            )])),
            Err(e) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&GetContextOutput {
                    found: false,
                    message: Some(format!("Database error: {}", e)),
                    file: None,
                    module: None,
                    layer: None,
                    rules: None,
                })
                .unwrap(),
            )])),
        }
    }
}

pub async fn start_server() -> Result<(), Box<dyn std::error::Error>> {
    let service = ArchexService::new().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}