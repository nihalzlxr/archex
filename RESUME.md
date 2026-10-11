# RESUME.md

Handoff for whoever picks archex up next. Read this, then `PLAN.md`.

## What this is

An architecture ledger plus an enforcement gate for AI coding agents. One
Rust binary, local-first, vendor-neutral. It answers two things teams cannot
get today: **REMEMBER** (why the code is built this way, handed to every
agent session) and **BLOCK** (changes that break the architecture, in CI,
with file and line numbers). Steps 0–3 are built. What is missing is
**adoption**, not capability: the forced loop, and drafting so nobody has
to hand-author a rule.

## Where we are

| Step | State |
|---|---|
| 0 — truthful core | ✅ 182 tests, zero warnings |
| 1 — eval harness | ✅ baseline recorded (`EVAL.md`): archex loses to grep, Hit@3 17.6% vs 29.4% |
| 2 — the gate | ✅ exit test passed on **LORB**: 0 false positives, 5 real violations found |
| 2.5 — one truth for rules | ✅ advisory == gate == MCP, `rules` table deleted, import kinds shipped |
| 3 — the ledger | ✅ exit test passed: an agent records a decision, a *new* server process serves it |
| 3.5–4 — adoption chain | 🔴 **this is the work now** — three defects red, then `proposed`=report-only, then the forced loop, then drafting. See PLAN §11 |
| 4 — retrieval surface | ⬜ after the adoption chain. Retrieval is not what the forced loop needs |
| 5 — conditional | ⬜ conditional |

**First run on real repos found the adoption cliff.** On fitness and LORB
the gate and ledger work, but `init` drafts one module per top-level
directory — so on a flat repo there are no boundaries, no boundary rule is
expressible, and `check` prints "0 rule(s) loaded. No violations." A user
who has to hand-author rules before anything happens will not adopt this.
That is why 3.6–3.12 exist and why they precede retrieval.

**241 tests. `cargo test` must stay green. `cargo build` must stay warning-free.**

## Commands

```bash
cargo test                    # 241 tests
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

**Step 3.6 — finish the three defects.** Red tests are committed; the
fixes are not. All three make a number archex prints wrong.

1. **Python and Rust imports are destroyed.** Both extractors build
   `Import { file_path: String::new(), module_id: 0, .. }`. The imports
   table has `FOREIGN KEY(module_id)`, foreign keys are ON, module ids
   start at 1 — so the insert fails for the whole file's batch, the error
   is dropped with `.ok()` (`parser.rs` scan loop), and `init` still counts
   them in "Imports extracted". **Fix:** normalize `file_path` and
   `module_id` on every returned import in
   `extract_symbols_and_imports`, so no collector can forget again.
2. **`.tsx`/`.jsx` parse with the wrong grammar.** `LANGUAGE_TYPESCRIPT`
   has zero JSX node types; `LANGUAGE_TSX` exists and is never used.
   **Fix:** dispatch on the extension. Verify with the three-JSX-function
   fixture in `tests/graph_holes.rs`.
3. **`is_route_file` reads the absolute path**, with `&&` binding tighter
   than `||` so `/app/api/` matches helpers next to route files.
   **Fix:** hand it the repo-relative path and parenthesize the filename
   clause. Unit tests are in `src/core/parser.rs`.

Then, in this order (full table and rationale in PLAN §11):

| # | Slice | Why it is next |
|---|---|---|
| 3.8 | `proposed` = report-only | A drafted rule must not fail CI before anyone agreed to it |
| 3.9 | **The forced loop** — `archex hook pre-edit` / `post-edit` + `setup --agent` wiring | Rules nobody delivers enforce nothing. This is the answer to "the agent forgets". Fail open, once per file per content hash |
| 3.10 | `init --draft` — mined conventions + template library, each citing evidence | Removes the manual work. Blocked on 3.6: a draft on a lying graph is fiction |
| 3.11 | `archex map` — tree, `--diff`, Mermaid projection | Seeing the architecture is what earns trust in a draft |
| 3.12 | `archex review` — batched accept/reject, allowed-crossing records | The learning loop: a rejection is permanent |

Retrieval (Step 4) and language breadth follow, not before: the forced
loop is what makes this product work, and it is unblocked today.

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
src/core/parser.rs     tree-sitter extraction — three defects being fixed (PLAN §17.6)
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