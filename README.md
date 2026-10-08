# Archex

**The architecture ledger and enforcement gate for AI coding agents.**

Archex sits between your team and your AI coding agents — Claude Code, Cursor, opencode, Windsurf, any MCP client. It remembers what was decided and why, hands that knowledge to every agent session automatically, and blocks changes that break your architecture before they merge.

One Rust binary. Local-first. Vendor-neutral. No accounts, no servers, no API keys.

---

## Why

Every agent session starts as a new hire. It re-learns your structure, guesses conventions from nearby files, and re-introduces things you rejected last month. You pay in review time — and review time is the real cost of AI-assisted development.

A CLAUDE.md file can't fix this: it's advice the model can ignore under pressure, it goes stale, and nothing checks it. Archex is the same knowledge made **structured, versioned, queryable, automatically injected, and enforced**.

---

## Features

| Feature | What you get |
|---|---|
| **Architecture memory** | Layers, boundary rules, and past decisions are injected into every agent session automatically — no re-explaining, no prompt rituals |
| **The gate** | Rules enforced in CI with file, line, and a plain-English fix. Breaking a boundary fails the PR in seconds, not in review |
| **Baseline mode** | Existing violations are recorded once — only *new* violations block. Old debt burns down deliberately |
| **Task ledger** | Stop Friday, resume Monday in any agent — goal, plan, decisions, and where you stopped travel with the branch |
| **Agent write-back** | Agents record what they learn (`record_decision`); merging the PR turns it into team memory |
| **Blast radius** | "If I change this, what breaks?" answered before the refactor, not discovered in review |
| **Symbol search** | Search `user`, find `getUserById` — word-split full-text over every indexed symbol |
| **Git is the sync layer** | Teammates get new rules and decisions with `git pull`. Zero per-machine setup |
| **Signatures first** | Context is served as compact signatures with token budgets — agents read maps, not whole files |

---

## Quick start

```bash
# Install (from source)
cargo install --path .

# In your repo:
archex init       # scan the codebase, build the index
archex setup      # auto-detect your agent, write its MCP config
archex sync       # import rules/decisions from git into the local index
```

Add the gate to CI (GitHub Actions example):

```yaml
- name: archex gate
  run: |
    curl -sSL https://releases.archex.dev/install.sh | sh   # or: cargo install archex
    archex check --ci
```

The first time a violation exists that you accept as existing debt: `archex check --update-baseline` records it, and CI only fails on new violations from then on.

---

## How it works

| Piece | Role |
|---|---|
| **CLI** | `init`, `sync`, `serve`, `check`, `decide`, `status` |
| **MCP server** (stdio) | What agents talk to. A generated brief — layers, boundary rules, active decisions — is delivered to every session automatically. Agents pull detailed slices through tools and push learnings via `record_decision` |
| **CI job** | The real gate. Builds an ephemeral index from the PR's code, reads committed rules and baseline, returns an exit code. Stores nothing between runs |
| **Local index** | SQLite at `.archex/archex.db` — a derived cache, rebuilt from code + committed files. Never committed |

### What lives in git

```
.archex/
  config.toml                  # modules + layers (reviewed in PRs like code)
  rules/<id>.toml              # one rule per file
  decisions/<ULID>-<slug>.md    # one decision per file, append-only
  tasks/<id>.md                # task ledger, travels with the feature branch
  baseline.toml                # accepted existing violations
```

Everything is plain text: merge-conflict-free by design (one file per record, ULID names), reviewed in pull requests, and readable by humans and agents alike.

---

## MCP tools

| Tool | Description |
|---|---|
| `get_context(file_path)` | Module, layer, rules, and relevant decisions for a file — before editing |
| `search(query)` | Ranked symbol search (word-split, camelCase-aware) |
| `traverse(symbols, direction, hops)` | Impact subgraph — who imports, who calls, what breaks |
| `read(symbols)` | Signatures by default, full code on demand |
| `record_decision(...)` | Agent write path — proposals accepted when the PR merges |
| `check_drift(file_path)` | Fast advisory violation check during editing (CI is the guarantee) |

## CLI

| Command | Description |
|---|---|
| `archex init` | Scan the codebase and build the local index |
| `archex sync` | Import changed rules/decisions/tasks from git; re-parse changed files |
| `archex serve` | Start the MCP server (stdio) |
| `archex check` | Run the gate locally (`--update-baseline`, `--ci` for CI mode) |
| `archex decide` | Browse and manage the decision log |
| `archex status` | Sync state, active rules, violations vs baseline |

---

## Requirements

- **No API keys. No accounts. No outbound network calls.** Archex never talks to an LLM and never sends data anywhere — it's the memory and the rulebook; your agents bring the intelligence
- A Rust toolchain to build from source, or a prebuilt release binary
- `git` (the sync layer)
- Any MCP-capable coding agent
- Any CI system for the gate

## Storage & privacy

- The index is a **derived cache** — delete it and `archex sync` rebuilds it from code + committed files
- SQLite and all parsing (tree-sitter: TypeScript/TSX, JavaScript/JSX, Rust, Python) are embedded in the single binary
- The team's architectural memory lives in git, gets code review like code, and works offline

## Works with

Claude Code · Cursor · opencode · Windsurf · Continue · any MCP client

---

## License

MIT
