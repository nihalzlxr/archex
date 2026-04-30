# AGENTS.md

## Project

`archex` - Architecture explorer (Rust binary)

## Structure

```
src/
├── main.rs              # CLI routing via clap
├── cli/
│   ├── mod.rs
│   ├── init.rs          # archex init
│   └── serve.rs         # archex serve
└── core/
    ├── mod.rs
    ├── db.rs            # SQLite via rusqlite (bundled)
    ├── parser.rs        # tree-sitter + tree-sitter-typescript
    ├── rule_engine.rs
    └── mcp.rs           # MCP server via rmcp
```

## Commands

```bash
cargo run -- init        # Run archex init
cargo run -- serve       # Run archex serve
cargo build              # Build release
cargo check              # Type check
```

## Database (src/core/db.rs)

DB path: `.archex/db.sqlite` in CWD

### Tables

- `modules` (id, name, layer, path_pattern)
- `rules` (id, module_id, rule_type, description, pattern)
- `file_map` (file_path, module_id, last_parsed)
- `decisions` (id, title, context, decision, created_at)

### API

- `Db::open(path: &Path) -> Result<Db>` - open/create DB
- `Db::init_schema(&self) -> Result<()>` - create tables if not exist
- `Db::insert_module(&self, name, layer, path_pattern) -> Result<i64>`
- `Db::insert_rule(&self, module_id, rule_type, description, pattern) -> Result<()>`
- `Db::upsert_file(&self, file_path, module_id) -> Result<()>`
- `Db::get_context_for_file(&self, file_path) -> Result<Option<FileContext>>`

## Dependencies

- clap (derive) - CLI
- rusqlite (bundled) - SQLite
- rmcp - MCP server
- tree-sitter + tree-sitter-typescript - parsing
- serde + serde_json - serialization
- tokio (full) - async runtime
- anyhow - error handling
- walkdir - filesystem traversal