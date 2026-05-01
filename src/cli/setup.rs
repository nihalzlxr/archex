use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(agent_override: Option<String>) -> anyhow::Result<()> {
    let detected_agent = if let Some(agent) = agent_override {
        agent
    } else {
        detect_agent()?
    };

    let binary_path = env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "archex".to_string());

    let project_root = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    match detected_agent.as_str() {
        "opencode" => setup_opencode(&binary_path, &project_root)?,
        "cursor" => setup_cursor(&binary_path, &project_root)?,
        "claude" => setup_claude(&binary_path)?,
        _ => anyhow::bail!("Unknown agent: {}", detected_agent),
    }

    println!("✅ Detected: {}", detected_agent);
    println!("✅ Added archex MCP server");

    println!("\nNext: restart {} and run: archex init", detected_agent);

    Ok(())
}

fn detect_agent() -> anyhow::Result<String> {
    if Path::new("opencode.json").exists() {
        return Ok("opencode".to_string());
    }
    if Path::new(".cursor/mcp.json").exists() {
        return Ok("cursor".to_string());
    }
    if let Some(home) = env::var_os("HOME") {
        let claude_config = PathBuf::from(home)
            .join("Library/Application Support/Claude/claude_desktop_config.json");
        if claude_config.exists() {
            return Ok("claude".to_string());
        }
    }
    anyhow::bail!("Could not detect AI agent. Use --agent flag to specify.")
}

fn setup_opencode(binary_path: &str, project_root: &str) -> anyhow::Result<()> {
    let config_path = Path::new("opencode.json");

    let config = if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::Value::Object(Default::default()))
    } else {
        serde_json::json!({
            "$schema": "https://opencode.ai/config.json"
        })
    };

    let mut config = config;
    if let Some(obj) = config.as_object_mut() {
        let mcp = obj.entry("mcp").or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(mcp_obj) = mcp.as_object_mut() {
            mcp_obj.insert(
                "archex".to_string(),
                serde_json::json!({
                    "type": "local",
                    "enabled": true,
                    "command": [binary_path, "serve"],
                    "cwd": project_root
                }),
            );
        }
    }

    let json = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, json)?;

    Ok(())
}

fn setup_cursor(binary_path: &str, project_root: &str) -> anyhow::Result<()> {
    let config_dir = Path::new(".cursor");
    fs::create_dir_all(config_dir)?;

    let config_path = config_dir.join("mcp.json");

    let config = if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::Value::Array(Default::default()))
    } else {
        serde_json::json!([])
    };

    let mut config = config;
    if let Some(arr) = config.as_array_mut() {
        arr.push(serde_json::json!({
            "name": "archex",
            "command": binary_path,
            "args": ["serve"],
            "env": {
                "cwd": project_root
            }
        }));
    }

    let json = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, json)?;

    Ok(())
}

fn setup_claude(binary_path: &str) -> anyhow::Result<()> {
    let home = env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
    let config_dir = PathBuf::from(home).join("Library/Application Support/Claude");
    fs::create_dir_all(&config_dir)?;

    let config_path = config_dir.join("claude_desktop_config.json");

    let config = if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::Value::Object(Default::default()))
    } else {
        serde_json::json!({ "mcpServers": {} })
    };

    let mut config = config;
    if let Some(obj) = config.as_object_mut() {
        let mcp = obj.entry("mcpServers").or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(mcp_obj) = mcp.as_object_mut() {
            mcp_obj.insert(
                "archex".to_string(),
                serde_json::json!({
                    "command": binary_path,
                    "args": ["serve"]
                }),
            );
        }
    }

    let json = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, json)?;

    Ok(())
}