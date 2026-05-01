# Archex

A local-first MCP server that acts as the **senior developer brain** behind any AI coding agent.

It knows your codebase. When your AI agent needs to build something, Archex gives it a complete, architecture-aware plan — the right files, the right patterns, the rules to follow, the security checklist — before a single line is written.

---

## The Problem

When a developer uses an AI coding agent on a real production codebase, the agent has no awareness of:
- What already exists and where
- How the project is structured
- What patterns and rules to follow
- What other modules are affected by a change

The result: blind code generation that breaks architecture, duplicates logic, skips security, and creates hours of rework.

---

## How It Works

```
Developer: "add fee payment reminders"
       ↓
Archex (MCP tool: create_plan)
  - scans existing codebase
  - finds relevant modules: jobs, services, notifications, db
  - identifies existing patterns to reuse
  - generates step-by-step plan with exact file paths
  - attaches rules: no DB in jobs, validate inputs, use existing notification service
       ↓
AI Agent (opencode / Claude Code / Cursor)
  - receives structured plan
  - executes with full context
  - no guessing, no drift
```

---

## Quick Start

```bash
# Build
cargo build --release

# Initialize - scan your codebase and build context DB
./target/release/archex init

# Start MCP server (for use with AI agents)
./target/release/archex serve
```

---

## MCP Tools

### 1. `get_context(file_path)`
Returns module, layer, and rules for a specific file. Used by AI before editing.

### 2. `get_module(module_name)`
Returns all files in a module, their purpose, key exports, and inter-module dependencies.

### 3. `create_plan(feature_description)`
Takes a plain English feature request. Returns:
- Which modules are involved
- Exact file paths to create or modify
- Existing patterns to follow
- Security checklist
- Step-by-step implementation order
- Rules that apply

---

## Integration

Add to your `opencode.json`:

```json
{
  "mcp": {
    "archex": {
      "type": "local",
      "enabled": true,
      "command": ["/path/to/archex", "serve"],
      "cwd": "/your/project",
      "env": {
        "OPENROUTER_API_KEY": "your_key_here"
      }
    }
  }
}
```

---

## Commands

| Command | Description |
|---------|-------------|
| `archex init` | Parse codebase, build context DB |
| `archex refresh` | Re-sync after large changes |
| `archex serve` | Start MCP server (stdio) |

---

## What It Stores

SQLite database (`.archex/db.sqlite`):

| Table | Description |
|-------|-------------|
| `modules` | name, layer, path pattern |
| `rules` | forbidden/required/warning per module |
| `file_map` | file path → module mapping |
| `decisions` | architectural decisions log |

---

## Tech Stack

| Component | Technology |
|-----------|-----------|
| Language | Rust |
| MCP server | rmcp |
| Storage | SQLite |
| Code parsing | tree-sitter |

---

## License

MIT