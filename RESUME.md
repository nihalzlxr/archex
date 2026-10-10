# RESUME.md

Handoff for whoever picks archex up next. Read this, then `PLAN.md`.

## What this is

An architecture ledger plus an enforcement gate for AI coding agents. One
Rust binary, local-first, vendor-neutral. It answers two things teams cannot
get today: **REMEMBER** (why the code is built this way, handed to every
agent session) and **BLOCK** (changes that break the architecture, in CI,
with file and line numbers). Steps 0–2 are built; the ledger (Step 3) is not.

## Where we are

| Step | State |
|---|---|
| 0 — truthful core | ✅ 176 tests, zero warnings, 60 commits |
| 1 — eval harness | ✅ baseline recorded (`EVAL.md`): archex loses to grep, Hit@3 17.6% vs 29.4% |
| 2 — the gate | ✅ exit test passed on **LORB**: 0 false positives, 5 real violations found |
| 3 — the ledger | ⬜ not started |
| 4 — retrieval surface | ⬜ not started |
| 5 — conditional | ⬜ conditional |

## Commands

```bash
cargo test                    # 176 tests, must stay green
cargo build                   # zero warnings is the bar, not a nice-to-have
cargo run -- init             # draft .archex/config.toml if absent, then index
cargo run -- check            # the gate: whole repo, exit 1 on a new violation
cargo run -- check --strict   # also fails when a rule matches nothing
cargo run -- check --update-baseline   # record current violations as debt
cargo run -- setup --agent ci # write the GitHub Actions gate
```

Eval harness (needs a repo clone; refuses to run on a branch — it checks
out commits):

```bash
ARCHEX_EVAL_REPO=/tmp/fitness-eval ARCHEX_EVAL_N=20 \
  cargo test eval -- --ignored --nocapture
```

## The next task, in full

**Rules have two sources of truth and that must be fixed before Step 3.**
The gate reads `.archex/rules/*.toml`; `archex rule add`/`remove`, MCP
`check_drift` and `check --file-path` still read DB rules. A rule added
through the CLI is not committed and is not enforced by the gate.

Approved approach — rules in files everywhere. Why the alternatives fail:
rules in the DB cannot survive "committed plain text is the source of
truth"; loading both (option 3) is exactly the failure the ledger exists to
prevent; leaving it (option 2) is a trap where `rule add` appears to work
and enforces nothing.

1. **One loader, every path.** Gate, `check --file-path` and MCP
   `check_drift` load through the same function and run the same engine. Add
   a test that runs one fixture through the CI path and the per-file
   advisory path and asserts identical findings. Advisory and gate must
   never disagree, or agents learn to distrust the advisory.
2. **`rule add` writes a file through the real validator.** Slug id, refuse
   to overwrite, full validation before writing, canonical serialization
   (sorted keys, fixed line endings) so diffs stay small. An invalid rule
   must fail at the CLI, not later in CI.
   **Note:** today's flags are `--module --rule-type --description
   --pattern` and cannot express a boundary rule at all. The CLI has to
   grow `from`/`to`/`allow-to`/`severity`, or the migration is half-done and
   the headline feature of Step 2 stays CLI-unreachable.
3. **`rule remove` retires by default.** Set `status = "retired"`; the engine
   already skips retired rules. Add `--purge` for real deletion. Decisions
   will reference rule ids, so deleting files creates dangling references.
   This is a human editing a committed file, not sync mutating anything, so
   it does not conflict with append-only.
4. **Legacy DB rules: warn once, then migrate.** On startup, if the DB has
   rules with no matching file, warn ("N legacy DB rules are ignored; run
   `archex rule migrate`") and provide that command to write them as files.
   Then drop the `rules` table at the next schema-version bump —
   rebuild-on-mismatch already handles it.
   **Note:** a DB rule is keyed by module *name*. `migrate` should REFUSE a
   rule whose module has no layer in `config.toml`, with a clear message,
   rather than invent one.

After that, Step 3 can start; the ledger file formats reuse the same loader
and validator.

## Hard-won rules — do not undo these

- **Unresolved ≠ violation.** An unresolved import is *unclassified*, never
  an accusation. The gate counts them and stays silent. Guessing here is how
  a guard cries wolf and gets disabled.
- **Unclassified must be visible.** `check` prints files-in-modules vs
  unclassified and unclassified imports. Uncovered code must never read as
  clean code. The counters come from the scan's persisted stats, not from
  the index — unmapped files never reach `file_map`.
- **Test files are exempt by default** (`test_globs` in config). On fitness,
  34 test-file-to-internals imports against 3 real ones. A rule that flags
  every test file is a rule that gets disabled on day one.
- **`config.toml` replaces seeding, it does not merge.** Otherwise a module
  can never be deleted. `init` never overwrites it.
- **No rule may match nothing silently.** Validation errors on an unknown
  layer/module reference; a rule whose side has no files is reported as
  suspicious and fails under `--strict`.
- **Layers are keys a human chose.** archex does not guess that `engine` is
  a domain layer. It cannot keep that claim, and Step 2 turns wrong layers
  into false violations.
- **The baseline is keyed without line numbers.** A violation that moves
  down the file is the same violation.
- **The index is derived.** Never committed, rebuilt on schema-version
  mismatch, never cached in CI. No migrations: a migration is code that runs
  once, on machines nobody has, to fix a shape nobody uses.

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
- `resolve.rs` once vanished from the working tree between two commands and
  was recovered from git. If a source file goes missing, `git checkout HEAD
  -- <file>` before anything else.

## What is where

```
src/core/resolve.rs    import resolution (the gate's foundation; 32 tests)
src/core/boundary.rs   the boundary engine (25 tests)
src/core/config.rs     config.toml + rules/*.toml parsers and validators
src/core/baseline.rs   baseline.toml
src/core/parser.rs     tree-sitter extraction: symbols, imports, line numbers
src/core/eval.rs       Step 1 harness (#[ignore]d)
src/cli/init.rs        seeding, the config drafter
src/cli/check.rs       the gate (run_gate) and the per-file advisory path
EVAL.md                Step 1 baseline numbers and how to reproduce
```