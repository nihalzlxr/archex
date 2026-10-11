# RESUME.md

Handoff for whoever picks archex up next. Read this, then `PLAN.md`.

## What this is

An architecture ledger plus an enforcement gate for AI coding agents. One
Rust binary, local-first, vendor-neutral. It answers two things teams cannot
get today: **REMEMBER** (why the code is built this way, handed to every
agent session) and **BLOCK** (changes that break the architecture, in CI,
with file and line numbers). Steps 0–3 are built; retrieval (Step 4) is not.

## Where we are

| Step | State |
|---|---|
| 0 — truthful core | ✅ 182 tests, zero warnings |
| 1 — eval harness | ✅ baseline recorded (`EVAL.md`): archex loses to grep, Hit@3 17.6% vs 29.4% |
| 2 — the gate | ✅ exit test passed on **LORB**: 0 false positives, 5 real violations found |
| 2.5 — one truth for rules | ✅ advisory == gate == MCP, `rules` table deleted, import kinds shipped |
| 3 — the ledger | ✅ exit test passed: an agent records a decision, a *new* server process serves it |
| 4 — retrieval surface | ⬜ not started |
| 5 — conditional | ⬜ conditional |

**235 tests. `cargo test` must stay green. `cargo build` must stay warning-free.**

## Commands

```bash
cargo test                    # 235 tests
cargo build                   # zero warnings is the bar, not a nice-to-have
cargo run -- init             # draft .archex/config.toml if absent, then index
cargo run -- check            # the gate: whole repo, exit 1 on a new violation
cargo run -- check --file-path src/foo.ts   # advisory: same engine, filtered
cargo run -- check --strict   # also fails when a rule matches nothing
cargo run -- check --update-baseline        # record current violations as debt
cargo run -- sync             # import committed decisions into the local index
cargo run -- decide add --title "..." --decision "..." --scope db --alternative "..."
cargo run -- rule add --id ui-no-db --kind boundary --from ui --to db --message "..."
cargo run -- setup --agent ci # write the GitHub Actions gate
```

Eval harness (needs a repo clone; refuses to run on a branch — it checks
out commits):

```bash
ARCHEX_EVAL_REPO=/tmp/fitness-eval ARCHEX_EVAL_N=20 \
  cargo test eval -- --ignored --nocapture
```

## The next task, in full

**Step 4 — the retrieval surface.** The ledger now knows *why*; Step 1
measured that archex loses to `grep` at finding *where* (Hit@3 17.6% vs
29.4%). That gap is the whole remaining product.

Build order, smallest first:

1. **`decide search` over the ledger index.** Decisions are indexed but not
   searchable; scope injection only answers "what applies to this file".
   FTS5 over the decisions table, the same token discipline Step 1 used for
   symbols (word-split, camelCase-aware) — the harness already exists in
   `core::eval`, so the numbers are comparable rather than invented.
2. **`search` / `traverse` / `read` MCP tools** (PLAN §10, Step 4 column),
   with **token budgets** and **stable ordering**. Stable means: the same
   query against the same index returns the same order, every time. An agent
   that gets a different list twice stops trusting the second answer.
3. **Re-run the Step 1 harness.** The exit test is "measurable gain over
   the Step 1 baseline", and that means re-running `ARCHEX_EVAL_REPO` on the
   same commits and recording the new Hit@3/MRR in `EVAL.md` — including
   the case where it does *not* improve. Three causes are already named in
   PLAN §11 "Measured results": the 5-symbol cap, symbol-substring matching
   instead of path matching, and paths carrying most of the signal while
   being barely read. Fix those three before building anything clever.
4. **Delete `create_plan`.** 172 lines of keyword matching with hardcoded
   advice (PLAN §17 item 3). It is scheduled for removal and every step
   after this one is easier without it.
5. **Optional, cheap, high value:** `archex status` — sync state, rule
   coverage, ledger coverage. Mneme's Architecture Audit is the model:
   which decisions are *Protected / Rules-only / Guidance*. Onboarding
   surface, and the honest answer to "is this thing actually working".

### Two known gaps that Step 4 should close, not work around

- **A schema bump empties the source index.** `sync` deliberately imports
  the ledger only; source files are parsed by `init`. On the first run after
  a bump, `get_context` reports "not mapped — run `archex init`". The fix
  is hash-driven re-parse of changed source files (PLAN §8 step 3), which is
  the other half of the sync contract.
- **`get_context` serves no decisions for an unmapped file**, because
  injection matches on module and an unmapped file has no module. The
  module can be derived from `config.toml` path patterns without an index —
  worth it only if it stays honest about which files it actually classified.

## Hard-won rules — do not undo these

- **Committed files are the record; the index is a cache.** A test asserts
  `sync` leaves the decision bytes *and* the mtime untouched. Same rule as
  Step 2.5 for rules: a record in a local database enforces nothing on
  anyone else's machine.
- **Derived, never written.** `superseded` comes from the chain, not the
  file — `status = "superseded"` is rejected at validation for the same
  reason `reachable = true` is. A field that asserts a fact the tool
  computes is a field that can lie.
- **Unresolved ≠ violation, and unclassified must be visible.** Same
  discipline as the gate, now applied to scope: a decision scoped to a
  module that does not exist is named by `sync`, not silently never
  injected.
- **Agents propose, humans accept — by absence, not by check.**
  `record_decision` has no `status` and no `author` parameter, so there is
  nothing to push on. A field that exists and is refused is still a field
  an agent is invited to try.
- **One writer per format.** `decision::write_new` renders, validates, and
  only then writes; the CLI and the MCP tool both go through it. Two
  renderers drift, and the second one is the one that writes bad records.
- **Cap what you inject and count what you dropped.** `get_context` sends
  ≤10 briefs *and* `decisions_omitted`. A silent truncation reads to an
  agent as "that was all of them".
- **Unresolved ≠ violation.** An unresolved import is *unclassified*, never
  an accusation. The gate counts them and stays silent.
- **Test files are exempt by default** (`test_globs` in config). A rule
  that flags every test file is a rule that gets disabled on day one.
- **`config.toml` replaces seeding, it does not merge.** Otherwise a module
  can never be deleted. `init` never overwrites it.
- **No rule may match nothing silently.** Validation errors on an unknown
  layer/module reference; a rule whose side has no files is suspicious and
  fails under `--strict`. `pattern`/`presence` rules are stored and
  explicitly reported as *not enforced yet*.
- **Layers are keys a human chose.** archex does not guess that `engine` is
  a domain layer.
- **The index is derived.** Never committed, rebuilt on schema-version
  mismatch. No migrations: a migration is code that runs once, on machines
  nobody has, to fix a shape nobody uses.

## Traps that cost real time

- **JSON-RPC here is line-delimited.** A pretty-printed `arguments` payload
  in a test is two broken messages, not a formatted one. All test arguments
  must be single-line.
- **The MCP server must not print to stdout.** stdout is the protocol
  stream; that is why `core::ledger::sync` returns a report and `cli/sync`
  prints it.
- **JSON-RPC `clientInfo` needs `version`**, and the server exits on stdin
  EOF before dispatching the tool call. `tests/common::call_mcp_tool` keeps
  stdin open on purpose.
- **tree-sitter-typescript emits a bare `type` node** for `import type`,
  not `import_type`. The first implementation looked for `import_type`,
  matched nothing, and read every type-only import as a runtime edge —
  exactly the false positive the column exists to remove. Pinned by tests.
- **`archex init` prompts when an index exists.** In scripts, pipe `y`.

## Environment notes

- **LORB is the user's live project.** Never edit it, never push, never
  create a branch or worktree in it. Clone instead:
  `git clone --local /Users/nihal/work/lorb /tmp/lorb-gate`. Ask before
  doing anything at all inside `/Users/nihal/work/lorb`.
- Eval corpora live in `/tmp` and are disposable: `/tmp/fitness-eval`
  (757 commits, pnpm monorepo) and `/tmp/lorb-gate` (3666 files, Next.js).
  Both are detached clones; `git checkout --detach <sha>` freely.
- **The eval harness moves the clone it runs against** — it checks out each
  sampled commit's parent. Leave a clone checked out at HEAD when you are
  done with it, or the next `init` will index a tree from three years ago.
- `/tmp/lorb-gate/.archex/decisions/` holds a real decision used to verify
  Step 3 end to end. It is untracked and disposable.
- `resolve.rs` once vanished from the working tree between two commands and
  was recovered from git. If a source file goes missing, `git checkout HEAD
  -- <file>` before anything else.

## What is where

```
src/core/decision.rs   ledger format: parse, validate, chain heads, one writer
src/core/ledger.rs     sync (hash-driven, prunes, derives heads), the brief
src/core/config.rs     config.toml + rules/*.toml parsers and validators
src/core/db.rs         SQLite index, SCHEMA_VERSION 4
src/core/resolve.rs    import resolution (the gate's foundation)
src/core/boundary.rs   the boundary engine
src/core/baseline.rs   baseline.toml
src/core/parser.rs     tree-sitter extraction: symbols, imports, line numbers
src/core/eval.rs       Step 1 harness (#[ignore]d)
src/cli/decide.rs      the decision ledger CLI
src/cli/rule.rs        the rule CLI
src/cli/sync.rs        prints what sync did
src/cli/init.rs        seeding, the config drafter
src/cli/check.rs       the gate (run_gate) and the per-file advisory path
EVAL.md                Step 1 baseline numbers and how to reproduce
```