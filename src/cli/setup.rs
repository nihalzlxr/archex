use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Where `archex setup --agent ci` fetches the binary from.
pub const DEFAULT_ARCHEX_GIT: &str = "https://github.com/nihalzlxr/archex.git";

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
        "ci" => {
            let root = env::current_dir()?;
            match install_ci_workflow(
                &root,
                &env::var("ARCHEX_GIT").unwrap_or_else(|_| DEFAULT_ARCHEX_GIT.to_string()),
                &env::var("ARCHEX_REF").unwrap_or_else(|_| "main".to_string()),
            )? {
                InstallOutcome::Written(path) => {
                    println!("Wrote {}", path.display());
                    println!("The gate now runs on every pull request.");
                    return Ok(());
                }
                InstallOutcome::Skipped { path } => {
                    println!("{} already exists; leaving it alone.", path.display());
                    return Ok(());
                }
            }
        }
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

        // Add hooks for pre-edit and post-edit
        let hooks = obj.entry("hooks").or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            hooks_obj.insert(
                "pre_edit".to_string(),
                serde_json::json!({
                    "command": [binary_path, "hook", "pre-edit", "{file}"],
                    "cwd": project_root
                }),
            );
            hooks_obj.insert(
                "post_edit".to_string(),
                serde_json::json!({
                    "command": [binary_path, "hook", "post-edit", "{file}"],
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

    // Cursor doesn't have native pre/post edit hooks, but we can add them to the mcp config as tools
    // For now, just note that hooks need manual setup for Cursor
    println!("Note: Cursor requires manual hook setup. Add these to your Cursor settings:");
    println!("  Pre-edit: {} hook pre-edit {{file}}", binary_path);
    println!("  Post-edit: {} hook post-edit {{file}}", binary_path);

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

    // Claude Code uses bash hooks - print instructions
    println!("Note: For Claude Code, add these to your shell config or project .claude/hooks:");
    println!("  Pre-edit: {} hook pre-edit \"$FILE\"", binary_path);
    println!("  Post-edit: {} hook post-edit \"$FILE\"", binary_path);

    Ok(())
}

/// What happened when we tried to install the workflow.
#[derive(Debug, PartialEq)]
pub enum InstallOutcome {
    Written(PathBuf),
    /// A workflow was already there. Ours or theirs, we do not touch it.
    Skipped { path: PathBuf },
}

/// Install the CI gate. The index is derived, so CI rebuilds it every run and
/// stores nothing: no artifacts, no caches, no uploads.
pub fn install_ci_workflow(
    root: &Path,
    archex_git: &str,
    archex_ref: &str,
) -> anyhow::Result<InstallOutcome> {
    let path = root.join(".github/workflows/archex.yml");
    if path.exists() {
        return Ok(InstallOutcome::Skipped { path });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let workflow = format!(
        r#"# Architecture gate. Generated by `archex setup --agent ci`.
#
# The index (.archex/archex.db) is DERIVED: rebuilt on every run from this
# checkout, never cached, never uploaded. Committed state is only
# .archex/config.toml, .archex/rules/*.toml and .archex/baseline.toml.
name: archex

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Keep the derived index out of git
        run: echo ".archex/archex.db*" >> .gitignore

      - name: Build archex
        run: |
          git clone --depth 1 --branch {ref} {git} /tmp/archex-src
          cargo install --path /tmp/archex-src --root /tmp/archex-install

      - name: Build the index
        run: /tmp/archex-install/bin/archex init

      # --strict: a rule that matches nothing is a misconfigured rule, and a
      # misconfigured rule is worse than no rule because it looks like a pass.
      - name: Check the architecture
        run: /tmp/archex-install/bin/archex check --strict
"#,
        ref = archex_ref,
        git = archex_git
    );

    fs::write(&path, workflow)?;
    Ok(InstallOutcome::Written(path))
}

#[cfg(test)]
mod ci_tests {
    use super::*;

    fn install(dir: &Path) -> anyhow::Result<InstallOutcome> {
        install_ci_workflow(dir, "https://github.com/example/archex.git", "v0.1.0")
    }

    #[test]
    fn writes_a_workflow_that_actually_runs_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).expect("installed");

        let path = dir.path().join(".github/workflows/archex.yml");
        let body = std::fs::read_to_string(&path).expect("workflow written");

        // The three things that make it a gate rather than a report.
        assert!(body.contains("archex init"), "must build an index");
        assert!(body.contains("archex check"), "must run the check");
        assert!(
            body.contains("pull_request"),
            "must run on the PR, not only on main"
        );
        // The index is derived and must never be committed back.
        assert!(
            body.contains("archex.db"),
            "must keep the derived cache out of git"
        );
    }

    #[test]
    fn the_index_is_built_ephemerally_and_stored_nowhere() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).expect("installed");
        let body =
            std::fs::read_to_string(dir.path().join(".github/workflows/archex.yml")).unwrap();
        assert!(
            !body.contains("actions/upload-artifact"),
            "the gate stores nothing: no artifacts, no caches, no uploads"
        );
        assert!(
            !body.contains("actions/cache"),
            "the index is rebuilt every run; caching it would be caching a lie"
        );
    }

    #[test]
    fn an_existing_workflow_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".github/workflows/archex.yml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mine, hand-edited\n").unwrap();

        let outcome = install(dir.path()).expect("handled");
        assert!(matches!(outcome, InstallOutcome::Skipped { .. }));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine, hand-edited\n",
            "a hand-edited workflow is the user's, not ours"
        );
    }

    #[test]
    fn installing_twice_does_not_duplicate_or_clobber() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).expect("first");
        let first = std::fs::read_to_string(dir.path().join(".github/workflows/archex.yml")).unwrap();
        install(dir.path()).expect("second");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".github/workflows/archex.yml")).unwrap(),
            first
        );
    }

    #[test]
    fn the_workflow_pins_the_archex_version_and_source() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).expect("installed");
        let body =
            std::fs::read_to_string(dir.path().join(".github/workflows/archex.yml")).unwrap();
        assert!(body.contains("https://github.com/example/archex.git"), "{}", body);
        assert!(body.contains("v0.1.0"), "a moving ref makes CI non-reproducible");
    }

    #[test]
    fn the_gate_runs_in_strict_mode_by_default() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).expect("installed");
        let body =
            std::fs::read_to_string(dir.path().join(".github/workflows/archex.yml")).unwrap();
        assert!(
            body.contains("archex check --strict"),
            "CI should refuse a rule that matches nothing; that is the failure \
             mode nobody notices until it silently stops guarding anything"
        );
    }
}
