# archex v2 — Master Plan

Status: agreed and frozen · Last updated: 2026-10-08

---

## 1. The product

**Archex is an architecture ledger plus an enforcement gate for AI coding agents.** One Rust binary, local-first, vendor-neutral. Two things teams currently can't get:

1. **REMEMBER** — why the code is built the way it is, handed to every agent session automatically.
2. **BLOCK** — changes that break the architecture, in CI, with file and line numbers.

A local code graph (who imports whom) feeds both. The graph is the engine, not the product.

**The problem:** every agent session starts as a new hire. It re-learns structure, guesses conventions, re-introduces things you rejected last month. You pay in review time — and review time is the real cost of AI-written code (45% of merged AI PRs need revision; feedback loops are 35.8% of agent conversations). A CLAUDE.md file can't fix this: it's advice, it goes stale, the model ignores it under pressure, and nothing checks it. Archex is the same knowledge made structured, versioned, queryable, injected, and **enforced**.

## 2. Locked principles

1. CHECK and REMEMBER are the product. MAP and FIND are substrate.
2. Signatures, not code bodies. (Three independent studies.)
3. Precision over recall. A guard that cries wolf gets disabled on day one.
4. Advice fails under pressure; gates don't. MCP tools are advisory, CI is the guarantee.
5. Plain text in git is the source of truth; the database is a derived cache, never committed.
6. Nothing heavy gets built without the eval harness proving it helps.
7. Success = fewer architecture violations merged + less re-explaining across sessions. Not tokens served.

## 3. The pieces

| Piece | Role |
|---|---|
| **CLI** (`archex`) | `init`, `sync`, `serve`, `check`, `decide`, `status` |
| **MCP server** (stdio) | What agents talk to. `ServerInfo.instructions` = generated brief (layers, boundary rules, active decision titles) — small, it's the only automatic surface. Slices ride on tool calls. `record_decision` is the agent write path |
| **CI job** | The real gate. Ephemeral index from PR code + committed rules + committed baseline → exit code. Stores nothing |
| **Local SQLite** | Derived cache. Never committed, deterministically rebuildable from code + committed files |

## 4. What lives in git

```
.archex/
  config.toml                  # modules + layers (top of file), small, shared
  rules/<id>.toml              # one rule per file, strict TOML, CI-validated
  decisions/<ULID>-<slug>.md    # one decision per file, append-only
  tasks/<id>.md                 # task ledger, travels with the feature branch
  baseline.toml                # recorded existing violations (checked in)
  .gitignore                   # archex.db, cache/, session/
```

**Why this shape:** one file per record + ULIDs = two devs adding decisions on different branches never conflict. Append-only decisions (never edit in place; new ones `supersede`) = history + clean merges. Canonical serialization (sorted keys, fixed line endings) = small readable diffs. The derived index is never shared — it's deterministic from code, so each machine recomputes it.

## 5. File formats

### config.toml

```toml
schema_version = 1
name = "lerniq"
layers = ["ui", "service", "db", "api"]   # top level, BEFORE any table

[[module]]
name = "components"
layer = "ui"
path = "src/components/**"
```

### rules/<id>.toml — one schema, three engines, `kind` selects

```toml
schema_version = 1
id = "ui-no-db-import"
status = "accepted"      # accepted | proposed | retired
kind = "boundary"        # boundary | pattern | presence
severity = "error"       # error = CI exit 1 | warning = report only

[boundary]
from_layer = "ui"
to_layer = "db"
message = "UI must not import the db layer. Go through services."

# [pattern]  → scope, mode (forbidden|required|warning), pattern, message
# [presence] → applies_to, requires, message
```

Validator rule: `kind` must match exactly one engine table. Rules are human-authored in v1; agents may only *suggest* one inside a decision.

### decisions/<ULID>-<slug>.md — markdown + frontmatter

```markdown
---
schema_version: 1
id: 01J8K2A7B3C4D5E6F7G8H9J0K     # ULID — sorts by time, conflict-free
title: Use Drizzle as the ORM
status: proposed                     # proposed | accepted | superseded
supersedes: []                       # ULIDs this replaces
task: add-billing-webhooks           # provenance
scope: [db, services]
author: human                        # human | agent
recorded_at: 2026-10-08T14:22:00Z
---
## Context / ## Decision / ## Alternatives rejected
```

Agent writes `status: proposed, author: agent` → merging the PR accepts it. **Superseded is derived, never written:** sync resolves chains and serves only the head. Files are never modified by archex.

### tasks/<id>.md

Goal, plan, decisions made, where we stopped, next steps. Frontmatter: `id, title, status (active|blocked|done|abandoned), owner, branch, created, updated, decisions: [ULIDs]`. Lives on the feature branch, merges with the PR. **Done is derived:** a task file present on main = done/historical. Friday you stop, Monday any agent picks up from the file. Nothing is lost to compaction.

### baseline.toml — committed, because CI is stateless

```toml
schema_version = 1

[[violation]]            # boundary: stable, churn-proof key
rule = "ui-no-db-import"
from_file = "src/components/Foo.tsx"
to_file = "src/db/client.ts"

[[violation]]            # pattern: file + count, NEVER line numbers
rule = "no-fetch-in-components"
file = "src/components/Bar.tsx"
count = 3
```

`archex check --update-baseline` records existing violations → CI fails only on *new* ones → old debt burns down deliberately.

## 6. The gate

Three engines, one discipline each:

- **boundary** — edge query over resolved imports. The core. "UI must not import db" is a graph question, not a regex over file contents.
- **pattern** — content match with line numbers (uses tree-sitter, not lowercase-contains).
- **presence** — required content (e.g. API routes must import zod).

**Adoption disciplines:**

1. **Precision over recall.** Unresolved import = *unclassified*, never a violation.
2. **Baseline mode.** Old debt doesn't fail CI; new violations do.
3. **Hooks are feedback, CI is the guarantee.** MCP `check_drift` is advisory; the exit code blocks.

## 7. The ledger

Decisions accumulate as the team's shared memory, reviewed in PRs like code, versioned like code, offline, no account. Sync + injection means a teammate's `git pull` gives *their* agents your new decisions automatically. Authority ordering is explicit: authored rules > agent-proposed decisions, always.

## 8. Sync contract

`archex sync` is idempotent and hash-driven:

1. Hash all committed `.archex` files → diff against local manifest.
2. Import only what changed; derive states (chain heads, task done-by-location) into the local DB — **never modify committed files**.
3. Re-parse only source files whose content hash changed.
4. Triggers: git `post-merge`/`post-checkout` hooks (optimization) + lazy check at MCP server start (the guarantee).

Local index keyed on content hashes, not commit SHAs — rebases and force-pushes can't corrupt it.

## 9. The graph substrate

File-level resolved imports first. **The hard part is TypeScript** — `tsconfig` path aliases, barrel re-exports, `index.ts` resolution — the biggest schedule risk in the plan. High-confidence matches only; everything else is unclassified. Symbol-level edges, PageRank, and the repo map are **conditional** (Step 5): built only if the harness proves retrieval is still the bottleneck.

## 10. MCP tool surface evolution

| Tool | Today | Steps 0–3 | Step 4 |
|---|---|---|---|
| `get_context` | keep | keep (serves rules + decisions slice) | keep |
| `check_drift` | keep | keep (advisory pre-edit feedback) | keep |
| `record_decision` | — | **new (Step 3)** | keep |
| `get_module`, `find_similar` | keep | untouched | folded in |
| `create_plan` | broken | untouched (dying) | **removed** |
| `search` / `traverse` / `read` | — | — | **new (Step 4)** |

## 11. Build order

| # | Ship | Exit test — we don't advance without it |
|---|---|---|
| **0** | Truthful core: idempotent init, symbol dedup (`clear_file_symbols` wired), shared case-correct regex helper, `exported` + signature extraction fixes, WAL + one shared connection, path normalization, dead code deleted, generic module seeding, **fixture tests + full suite** | `init` twice clean; suite green |
| **1** | **Eval harness**: ~20 real LORB commits, ground truth = files each commit touched; Hit@3, MRR, function accuracy; baselines = plain grep + current archex | Numbers recorded (they'll be bad — that's the point) |
| **2** | **The Gate**: file-level import resolution, rules as data, `archex check` + CI job, baseline mode | ≥1 real violation caught on LORB; **zero false positives on a clean tree** |
| **3** | **The Ledger**: file formats + validator, `archex sync`, brief injection, `decide` CLI, `record_decision` MCP tool | An agent logs a decision mid-session; the next session starts knowing it |
| **4** | Retrieval surface: FTS5 word-split search, `search`/`traverse`/`read` with token budgets, stable ordering | Measurable gain over the Step 1 baseline |
| **5** | Conditional: symbol edges, PageRank repo map | Only if Step 4 leaves retrieval as the proven bottleneck |

**The real test:** Steps 1–4 deployed on LORB within a few weeks. Ship the slice, learn from real use, then decide on 4–5. Completing the plan before contact with reality is the failure mode.

## 12. Development method: TDD

Rust is arguably the **best** mainstream TDD environment:

- **Zero-config test runner**: `cargo test`. Tests live in-file under `#[cfg(test)] mod tests` or in `tests/` for integration.
- **Honest red phase**: in Rust the "red" can be a compile error — the compiler won't let you fake a green. `cargo check` is a sub-second red/green signal.
- **Fast loop**: `bacon` (or `cargo watch -x test`) re-runs tests on every save.
- **The crates we need are standard**: `insta` (golden/snapshot tests for parser output), `assert_cmd` (CLI integration tests), `tempfile` (fixture repos), `proptest` (tokenizer + path normalization), and rusqlite's `Connection::open_in_memory()` for instant, isolated DB tests.

**We practice TDD at three levels:**

| Level | Tool | What it covers |
|---|---|---|
| Unit | `cargo test` | Every bug in Step 0 gets a **failing test first**, then the fix. Parser: fixture TS/Rust/Python files → insta snapshot of symbols/edges. Import resolution: a fixture matrix of the risky cases (aliases, barrels, index files) — the riskiest code gets the most tests. Rule engines: fixture code → violations; **a clean-tree fixture must produce zero findings** (the false-positive test is a required test) |
| Integration | `assert_cmd` + `tempfile` | `init` twice → clean DB; `check` exit codes; `sync` idempotency (run twice = no changes) |
| Product | The Step 1 harness | The outermost red-green loop: baseline numbers are red, every later step must move them or it doesn't ship |

MCP handlers are pure request-JSON → result-JSON functions — directly unit-testable without a live server.

**Definition of done for every step: failing tests written first, then code, then the step's exit test.**

## 13. Metrics

- **Product** (from Step 2 on): violations caught per PR; repeated architecture violations in merged PRs trending down.
- **Ledger** (from Step 3 on): decisions injected; "decisions used" via a defined proxy (retrievals touching a decision's scope) — define the proxy at Step 3, don't hand-wave it.
- **Retrieval** (from Step 1 on): Hit@3, MRR, function-level accuracy vs. grep baseline.
- **Guard trust**: false-positive rate on clean trees ≈ 0, always.

## 14. Risks

1. **TS import resolution** — biggest schedule risk. Mitigation: high-confidence matches only, unclassified ≠ violation, resolution fixture matrix, start with explicit boundary rules.
2. **Guard credibility** — one false positive on day one and adoption dies. Mitigation: precision-first, baseline mode, required clean-tree tests.
3. **Solo-dev bandwidth** — ship Steps 1–4, learn, then decide. Don't build the five-step plan before first contact.
4. **Staleness** — hash-driven sync + lazy check at serve start.
5. **Contaminated evals** — LORB's own history, never SWE-bench (memorized).

## 15. Non-goals

Embeddings. Vector DBs. Similar-chunk RAG. A better file-finder. Advice-only rules. PageRank-first. A server or account before a paid cloud tier. Agent-writable rules in v1.

## 16. Bug triage reference (Step 0 scope)

Fix what is on the path — do not polish what is being deleted:

- Idempotent init (UNIQUE(name) on modules, no duplicate seeding)
- Wire `clear_file_symbols` before batch inserts (no symbol duplication on re-scan)
- Case-correct shared regex helper (compile against original content or use `(?i)`)
- `exported` detection: walk to parent for `export_statement` / modifiers
- Signature extraction: use `parameters`/`return_type` fields, cover arrow functions and function expressions
- WAL + one shared connection (`OnceCell`)
- Path normalization (absolute → repo-relative) on lookup
- Delete dead parser functions + unused `reqwest` dependency
- Generic module seeding (infer from directories/manifests, not the hardcoded Next.js list)
- README/doc corrections (no phantom `refresh`, correct DB path)

Explicitly **not** fixed: `create_plan` scoring and its hardcoded checklist — they die with the tool surface in Step 4.
