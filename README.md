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
| **Decision ledger** | One markdown file per decision, in git, reviewed like code. An agent records why it did something; the next session starts knowing |
| **Agent write-back** | Agents record what they learn (`record_decision`) as *proposals*; a human accepts them in the PR |
| **Git is the sync layer** | Teammates get new decisions with `git pull`. Zero per-machine setup |
| **Uncovered is not clean** | Files and imports archex cannot classify are counted and reported. A gap never reads as a pass |
| **Task ledger** | *Planned (Step 4)* — stop Friday, resume Monday in any agent |
| **Blast radius** | *Planned (Step 4)* — "if I change this, what breaks?" answered before the refactor |
| **Symbol search** | *Planned (Step 4)* — search `user`, find `getUserById`, word-split over every indexed symbol |
| **Signatures first** | *Planned (Step 4)* — context served as compact signatures with token budgets |

---

## Quick start

```bash
# Install (from source)
cargo install --path .

# In your repo:
archex init       # scan the codebase, draft config.toml if absent, build the index
archex setup      # auto-detect your agent, write its MCP config
archex decide add --title "..." --decision "..." --scope <module> --alternative "..."
archex check      # the gate: whole repo, exit 1 on a new violation
```

Add the gate to CI (`archex setup --agent ci` writes the job):

```yaml
- name: archex gate
  run: |
    curl -sSL https://releases.archex.dev/install.sh | sh   # or: cargo install archex
    archex init          # CI is stateless: build the index from the PR's code
    archex check --strict
```

The first time a violation exists that you accept as existing debt: `archex check --update-baseline` records it, and CI only fails on new violations from then on.

---

## How it works

| Piece | Role |
|---|---|
| **CLI** | `init`, `check`, `sync`, `decide`, `rule`, `serve`, `setup` |
| **MCP server** (stdio) | What agents talk to. Agents pull slices through tools and push learnings via `record_decision`; the server syncs the ledger at startup, so a pulled decision reaches the next session with no command in between |
| **CI job** | The real gate. Builds an ephemeral index from the PR's code, reads committed rules and baseline, returns an exit code. Stores nothing between runs |
| **Local index** | SQLite at `.archex/archex.db` — a derived cache, rebuilt from code + committed files. Never committed |

### What lives in git

```
.archex/
  config.toml                  # modules + layers (reviewed in PRs like code)
  rules/<id>.toml              # one rule per file
  decisions/<ULID>-<slug>.md    # one decision per file
  baseline.toml                # accepted existing violations
  tasks/<id>.md                # planned (Step 4)
```

Everything is plain text: merge-conflict-free by design (one file per record, ULID names), reviewed in pull requests, and readable by humans and agents alike. archex never modifies a committed file — `sync` imports records into a local cache and leaves the originals byte for byte identical.

---

## MCP tools

| Tool | Description |
|---|---|
| `get_context(file_path)` | Module, layer, rules, and relevant decisions for a file — before editing |
| `record_decision(...)` | Agent write path — records a proposal with the alternatives it rejected; a human accepts it in the PR |
| `check_drift(file_path)` | Fast advisory violation check during editing (CI is the guarantee) |
| `get_module`, `find_similar` | Module info; similar-symbol lookup |
| `create_plan` | **Removed in Step 4** — it was keyword matching with hardcoded advice |

## CLI

| Command | Description |
|---|---|
| `archex init` | Scan the codebase, draft `.archex/config.toml` if absent, build the local index |
| `archex check` | The gate (`--file-path` for one file, `--strict`, `--update-baseline`) |
| `archex sync` | Import committed decision records into the local index. Hash-driven, idempotent, never writes to a committed file |
| `archex decide add\|list\|show` | Write and read decisions in `.archex/decisions/` |
| `archex rule list\|add\|remove\|migrate` | The same, for `.archex/rules/` |
| `archex serve` | Start the MCP server (stdio) |
| `archex setup --agent <name>` | Write the MCP config for opencode / Cursor / Claude |
| `archex status` | **Planned (Step 4)** — sync state, rule coverage, ledger coverage |

`sync` imports the **ledger** only. Source files are parsed by `archex
init`; the hash-driven re-parse of changed source files is Step 4 work.
After a schema bump the index is rebuilt empty until `init` runs, and
`get_context` says so rather than reporting a clean run.

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
