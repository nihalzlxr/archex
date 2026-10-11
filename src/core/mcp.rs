use rmcp::{ServerHandler, ServiceExt, model::*, schemars, tool, transport::stdio};
use serde::Deserialize;
use crate::core::db::{Db, SymbolType, DB_PATH};
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

/// The committed rules that govern this file, read from `.archex/rules/`.
///
/// The `rules` table died with SCHEMA_VERSION 3 — a rule that exists only in
/// a local index enforces nothing on anyone else's machine — but this caller
/// was left joining the empty table and answering `rules: []`. An empty array
/// reads to an agent as "nothing applies to this file", which is the one
/// answer that must never be returned by accident.
///
/// A rule governs a file when the file's module (or layer) is on its FROM
/// side: a rule about "ui must not import db" describes what ui files may not
/// do, so it is context for ui files and nothing for a db file.
fn rules_for_file(
    root: &Path,
    file_path: &str,
    module: &str,
    layer: &str,
) -> Vec<serde_json::Value> {
    use crate::core::config::{Kind, Status};

    // An unreadable rule file is reported by the gate and by check_drift.
    // Here it must not take the whole context call down: an agent gets the
    // module, the layer and the decisions even when the rules are broken.
    let rules = crate::core::config::load_rules(root).unwrap_or_default();
    let mut brief = Vec::new();
    for rule in &rules {
        if rule.status == Status::Retired {
            continue;
        }

        let governs = match (&rule.boundary, &rule.pattern, &rule.presence) {
            (Some(boundary), _, _) => {
                boundary.from_layer == module || boundary.from_layer == layer
            }
            // Content rules are scoped by glob against the file path.
            (None, Some(pattern), _) => {
                crate::core::boundary::matches_any(file_path, &[pattern.scope.clone()])
            }
            (None, None, Some(presence)) => {
                crate::core::boundary::matches_any(file_path, &[presence.applies_to.clone()])
            }
            (None, None, None) => false,
        };
        if !governs {
            continue;
        }

        brief.push(serde_json::json!({
            "id": rule.id,
            "kind": match rule.kind {
                Kind::Boundary => "boundary",
                Kind::Pattern => "pattern",
                Kind::Presence => "presence",
            },
            "status": match rule.status {
                Status::Accepted => "accepted",
                Status::Proposed => "proposed",
                Status::Retired => "retired",
            },
            // Only an accepted boundary rule is actually judged today. A
            // proposal shown as a rule would read as enforced and is not.
            "enforced": rule.boundary.is_some() && rule.status == Status::Accepted,
            "message": rule
                .boundary
                .as_ref().map(|b| b.message.clone())
                .or_else(|| rule.pattern.as_ref().map(|p| p.message.clone()))
                .or_else(|| rule.presence.as_ref().map(|p| p.message.clone()))
                .unwrap_or_default(),
        }));
    }
    brief
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordDecisionRequest {
    #[schemars(description = "One sentence: what was decided")]
    pub title: String,
    #[schemars(description = "The decision itself, in one or two sentences")]
    pub decision: String,
    #[schemars(description = "Why this came up — what forced the choice")]
    #[serde(default)]
    pub context: Option<String>,
    #[schemars(description = "Options that were rejected and why. Strongly recommended: it is what stops the same question being reopened next month")]
    #[serde(default)]
    pub alternatives: Vec<String>,
    #[schemars(description = "Module or layer names this applies to, e.g. [\"db\"]. Omit only if the decision is genuinely global")]
    #[serde(default)]
    pub scope: Vec<String>,
    #[schemars(description = "Provenance: the task or branch this came out of")]
    #[serde(default)]
    pub task: Option<String>,
    #[schemars(description = "ULID of the decision this replaces")]
    #[serde(default)]
    pub supersedes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArchexService;

/// How many decisions ride along in one context. A brief that pushes out
/// the module's own rules has cost more than it gave; the count of what
/// was left out ships with it, so a short list never reads as a complete
/// one.
const MAX_DECISIONS_IN_CONTEXT: usize = 10;

/// Characters per decision body in the brief.
const DECISION_SUMMARY_LIMIT: usize = 600;

/// The decisions whose scope covers this file's module or layer, newest
/// first, already trimmed to a brief — plus how many were left out, so a
/// capped list never reads as a complete one.
fn decisions_for(db: &Db, module: &str, layer: &str) -> (Vec<serde_json::Value>, usize) {
    let mut records = match db.decisions_for_scope(module, layer) {
        Ok(records) => records,
        // An unreadable index is reported through `ledger_error`; here it
        // must not become an exception that takes the tool down.
        Err(_) => Vec::new(),
    };
    let omitted = records.len().saturating_sub(MAX_DECISIONS_IN_CONTEXT);
    records.truncate(MAX_DECISIONS_IN_CONTEXT);
    let brief = records
        .iter()
        .map(|record| {
            serde_json::json!({
                "id": record.id,
                "title": record.title,
                "status": record.status,
                "author": record.author,
                "recorded_at": record.recorded_at,
                "summary": crate::core::ledger::summary(record, DECISION_SUMMARY_LIMIT),
            })
        })
        .collect();
    (brief, omitted)
}

#[tool(tool_box)]
impl ArchexService {
    #[tool(description = "Get architecture context, module, layer and rules for a file")]
    async fn get_context(
        &self,
        #[tool(aggr)] req: GetContextRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        let db_path = Path::new(DB_PATH);
        // Rules live in committed files, not in the index, so the context
        // answer reads them the same way the gate does.
        let root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let rel = crate::core::paths::normalize_in(&req.file_path, &root);

        let db = match Db::shared(db_path) {
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
            Ok(Some(ctx)) => {
                let (decisions, omitted) = decisions_for(&db, &ctx.module_name, &ctx.layer);
                let omitted_note = if omitted > 0 {
                    Some(format!(
                        "{} more decision{} apply to this file — \
                         `archex decide list` shows every current one.",
                        omitted,
                        if omitted == 1 { "" } else { "s" }
                    ))
                } else {
                    None
                };
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "found": true,
                        "file": req.file_path,
                        "module": ctx.module_name,
                        "layer": ctx.layer,
                        "rules": rules_for_file(&root, &rel, &ctx.module_name, &ctx.layer),
                        "decisions": decisions,
                        "decisions_omitted": omitted,
                        "decisions_omitted_note": omitted_note,
                        "ledger_error": LEDGER_ERROR.get(),
                    })).unwrap()
                )]))
            }
            Ok(None) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "found": false,
                    "message": format!("File '{}' not mapped. Run 'archex init' to scan project.", req.file_path),
                    "decisions": [],
                    "ledger_error": LEDGER_ERROR.get(),
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
        let db_path = Path::new(DB_PATH);
        
        let db = match Db::shared(db_path) {
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
        let db_path = Path::new(DB_PATH);
        let db = match Db::shared(db_path) {
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
        let db_path = Path::new(DB_PATH);
        let db = match Db::shared(db_path) {
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

    #[tool(description = "Check a file for architecture violations. Same engine and same rules as the CI gate, filtered to this file -- if this says clean, CI agrees. Use it before finishing an edit.")]
    async fn check_drift(
        &self,
        #[tool(aggr)] req: CheckDriftRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        use crate::core::config;
        use std::path::Path;

        let root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let db_path = Path::new(DB_PATH);
        let db = match Db::shared(db_path) {
            Ok(db) => db,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("Database not found. Run 'archex init' first. Error: {}", e)
                    })).unwrap()
                )]));
            }
        };

        let rel = crate::core::paths::normalize_in(&req.file_path, &root);
        if rel.is_empty() || crate::core::paths::looks_absolute(&rel) {
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "clean": true,
                    "file_path": req.file_path,
                    "note": "outside the project - no architectural context"
                })).unwrap()
            )]));
        }

        if let Err(e) = db.init_schema() {
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "error": format!("cannot read the index: {}", e)
                })).unwrap()
            )]));
        }

        crate::core::rule_engine::warn_legacy_db_rules(&db);

        let (config, rules) = match config::Config::load(&root).and_then(|c| {
            let rules = config::load_rules(&root)?;
            Ok((c, rules))
        }) {
            Ok(pair) => pair,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("{}", e),
                        "hint": "Run 'archex init' to draft a committed architecture."
                    })).unwrap()
                )]));
            }
        };

        let report = match crate::core::boundary::check(&db, &config, &rules) {
            Ok(report) => report,
            Err(e) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("{}", e)
                    })).unwrap()
                )]));
            }
        };

        // Exactly what `archex check --file-path` reports: the gate's own
        // findings, filtered to this file. An agent told "clean" here and
        // "failed" by CI learns to ignore the tool, which costs more than a
        // noisy one ever would.
        let findings: Vec<&crate::core::boundary::Finding> = report
            .findings
            .iter()
            .filter(|f| f.from_file == rel)
            .collect();

        let module = db
            .get_module_for_file(&rel)
            .ok()
            .and_then(|m| m)
            .map(|(name, _)| name)
            .unwrap_or_default();

        let errors = findings
            .iter()
            .filter(|f| f.severity == config::Severity::Error)
            .count();

        let result_json = serde_json::json!({
            "file_path": rel,
            "module": module,
            "clean": errors == 0,
            "violations": findings.iter().map(|f| serde_json::json!({
                "rule_id": f.rule_id,
                "severity": match f.severity {
                    config::Severity::Error => "error",
                    config::Severity::Warning => "warning",
                },
                "line_number": f.line_number,
                "imported_from": f.imported_from,
                "to_file": f.to_file,
                "message": f.message,
            })).collect::<Vec<_>>(),
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&result_json).unwrap()
        )]))
    }
/// Record why the code is the way it is, so the next session starts
    /// knowing it. Writes a proposed, agent-authored record and nothing
    /// else: there is deliberately no status or author parameter, because an
    /// agent cannot accept its own decision.
    #[tool(description = "Record an architectural decision you made while working, with the alternatives you rejected. Writes .archex/decisions/<ULID>-<slug>.md as status: proposed, author: agent — a human accepts it in review. Scope it with module or layer names so it reaches the files it concerns. Use it when you chose an approach, not for routine changes.")]
    async fn record_decision(
        &self,
        #[tool(aggr)] req: RecordDecisionRequest,
    ) -> Result<CallToolResult, rmcp::Error> {
        use crate::core::decision::{self, Author, DecisionStatus, NewDecision};

        let root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());

        let supersedes = match &req.supersedes {
            Some(id) if !decision::is_ulid(id) => {
                return Ok(tool_error(format!(
                    "`{}` is not a ULID. `archex decide list` shows them.",
                    id
                )));
            }
            Some(id) => vec![id.clone()],
            None => Vec::new(),
        };

        let new = NewDecision {
            id: decision::new_ulid(),
            title: req.title.clone(),
            decision: req.decision.clone(),
            context: req.context.clone(),
            alternatives: req.alternatives.clone(),
            scope: req.scope.clone(),
            task: req.task.clone(),
            // Fixed, not a parameter: agents propose, humans accept.
            status: DecisionStatus::Proposed,
            author: Author::Agent,
            supersedes,
            recorded_at: decision::now_rfc3339(),
        };

        match decision::write_new(&root, &new) {
            Ok(path) => Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&serde_json::json!({
                    "recorded": true,
                    "id": new.id,
                    "file": path,
                    "title": new.title,
                    "status": "proposed",
                    "author": "agent",
                    "next": "Commit it and open a PR. A human accepts it there; until then \
                             it is served to agents marked as proposed.",
                }))
                .unwrap(),
            )])),
            Err(error) => Ok(tool_error(format!("{}", error))),
        }
    }
}

/// A failure the agent can read and act on, without the tool call itself
/// looking like a transport error.
fn tool_error(message: String) -> CallToolResult {
    CallToolResult::success(vec![Content::text(
        serde_json::to_string(&serde_json::json!({ "error": message })).unwrap(),
    )])
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
    // The ledger is synced when the server starts, not when someone
    // remembers to: a teammate's `git pull` has to reach this session's
    // agents with no command in between. A failure is remembered and
    // reported by `get_context` rather than swallowed — a stale ledger
    // served silently is worse than no ledger.
    sync_ledger();

    let service = ArchexService;
    let server = service.serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}

/// The last startup sync failed. `get_context` reports it, because serving
/// a stale ledger without saying so is the failure this exists to prevent.
static LEDGER_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn sync_ledger() {
    let root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    if let Err(error) = crate::core::ledger::sync(&root) {
        eprintln!("archex: ledger sync failed: {}", error);
        let _ = LEDGER_ERROR.set(error);
    }
}