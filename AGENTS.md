# AGENTS.md

## Project

`archex` — architecture ledger + enforcement gate for AI coding agents (Rust binary, local-first, MCP server).

**Read [PLAN.md](./PLAN.md) before doing any work.** It is the frozen v2 master plan: principles, file formats, build order with exit tests, and the required development method (TDD). Work proceeds step by step (Step 0 → 5); do not skip exit tests.

## Development method (mandatory)

TDD at every scale — no code without a failing test first:

```bash
cargo test             # run all tests
cargo check            # fast red/green signal
cargo test <name>      # single target
```

- Every bug fix: failing test first, then the fix.
- Parser work: fixture files + snapshot tests (`insta`).
- CLI work: integration tests via `assert_cmd` + `tempfile`.
- DB work: `Connection::open_in_memory()`.

## Structure

```
src/
├── main.rs              # CLI routing via clap (Init, Serve, Setup, Check, Rule, Decision)
├── cli/
│   ├── mod.rs
│   ├── init.rs          # archex init — scan + seed modules (Next.js seeding; generic in Step 0)
│   ├── serve.rs         # archex serve — thin wrapper over core::mcp
│   ├── setup.rs         # archex setup — writes MCP config for opencode/Cursor/Claude
│   ├── check.rs         # archex check — CLI drift check (logic moves to rule_engine in Step 0)
│   ├── rule.rs          # archex rule list/add/remove/test
│   └── decision.rs      # archex decision add/list/search
└── core/
    ├── mod.rs
    ├── db.rs            # SQLite via rusqlite (bundled)
    ├── parser.rs        # tree-sitter: TS/TSX, JS/JSX, Rust, Python — symbols + imports + drift heuristics
    ├── rule_engine.rs   # TODO stub — becomes the single rule engine in Step 0 (3 copies of matching logic exist today: check.rs, rule.rs, mcp.rs)
    └── mcp.rs           # MCP server via rmcp: get_context, get_module, create_plan, find_similar, check_drift
```

## Known issues (tracked in PLAN.md §16 — do not fix ad hoc)

- `init` run twice duplicates modules and symbols (no UNIQUE constraint, `clear_file_symbols` never called)
- Regex rules match lowercased content against original-case patterns
- `exported` detection is wrong for TS (checks prev sibling, should check parent)
- Signature extraction picks up types from the function body
- Only Next.js projects get module seeding; other project types index nothing
- `create_plan` is keyword matching with hardcoded advice — it dies in Step 4; do not invest in it

## Commands

```bash
cargo run -- init                    # Run archex init
cargo run -- serve                   # Run archex serve
cargo run -- setup --agent opencode  # Write MCP config
cargo run -- check --file-path <p>   # Drift check one file
cargo build                          # Build
cargo check                         # Type check
cargo test                          # Run test suite
```

## Database (src/core/db.rs)

DB path: `.archex/archex.db` in CWD (gitignored; rebuilt from code + committed files in v2)

### Tables

- `modules` (id, name, layer, path_pattern)
- `rules` (id, module_id, rule_type, description, pattern)
- `file_map` (file_path, module_id, last_parsed)
- `decisions` (id, title, context, decision, created_at)
- `symbols` (id, file_path, symbol_type, name, signature, line_number, module_id, exported)
- `imports` (id, file_path, imported_from, imported_names, module_id)

### API

- `Db::open(path: &Path) -> Result<Db>` — open/create DB
- `Db::init_schema(&self) -> Result<()>` — create tables if not exist
- `Db::get_context_for_file(&self, file_path) -> Result<Option<FileContext>>`
- `Db::get_module_info(&self, name) -> Result<Option<ModuleInfo>>`
- `Db::search_symbols(&self, query) -> Result<Vec<SymbolSearchResult>>`
- `Db::clear_file_symbols(&self, file_path)` — dead code today; wired in Step 0

## Dependencies

- clap (derive) — CLI
- rusqlite (bundled) — SQLite
- rmcp + schemars — MCP server
- tree-sitter + tree-sitter-{typescript,javascript,rust,python} — parsing
- serde + serde_json — serialization
- tokio (full) — async runtime
- anyhow — error handling
- walkdir, glob — traversal, path patterns
- regex — rule matching
- reqwest — **unused; remove in Step 0**

New dev/test deps planned: `insta` (snapshots), `assert_cmd`, `tempfile`, `proptest`.
