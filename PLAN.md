# archex v2 — Master Plan

Status: Steps 0–2 shipped · Step 2.5 next · Last updated: 2026-10-10

> Steps 0, 1 and 2 are built, tested and verified on two real repos
> (`project/fitness`, and LORB — the deployment target). The build order and
> exit tests in §11 are unchanged; §12.1 records the decisions taken while
> building, including the two places the frozen plan turned out to be wrong.
> §18 is the verified competitive landscape (researched 2026-10-10); §19
> folds its steals into the remaining steps. **Step 2.5** (§11) is the next
> task: one source of truth for rules, before the ledger. `RESUME.md` is
> the handoff for whoever picks this up next.

---

## 1. The product

**Archex is an architecture ledger plus an enforcement gate for AI coding agents.** One Rust binary, local-first, vendor-neutral. Two halves, one system:

1. **REMEMBER** — why the code is built the way it is, handed to every agent session automatically.
2. **BLOCK** — changes that break the architecture, in CI, with file and line numbers.

A local code graph (who imports whom) feeds both. The graph is the engine, not the product.

**The problem:** every agent session starts as a new hire. It re-learns structure, guesses conventions, re-introduces things you rejected last month. You pay in review time — and review time is the real cost of AI-written code (45% of merged AI PRs need revision; feedback loops are 35.8% of agent conversations). A CLAUDE.md file can't fix this: it's advice, it goes stale, the model ignores it under pressure, and nothing checks it. Archex is the same knowledge made structured, versioned, queryable, injected, and **enforced**.

**Positioning (verified against the landscape, §18):** both halves exist separately in the market — Archcore ships git-native context with no deterministic gate; Mneme ships decisions + literal-text enforcement with no code graph; dependency-cruiser ships a graph gate with no decisions. The open quadrant is **decision-linked enforcement over the resolved import graph, in per-file plain text**. That integration is what archex is. Never claim "no competitors"; claim the quadrant.

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
test_globs = ["**/*.test.ts", "**/test/**"]   # added in Step 2

[[module]]
name = "components"
layer = "ui"
path = "src/components/**"
```

`test_globs` skips those files in boundary rules unless a rule sets
`include_tests = true`. The safe behaviour is the default one: a
hand-written rule must not flag every test file on day one. Measured on
fitness: 34 test-file-to-internals imports against 3 real ones.

Every key is validated. `deny_unknown_fields` everywhere, plus: a top-level
key placed after a `[[module]]` header is caught by name (TOML silently
absorbs it, which would leave a valid config with no layers at all), module
names must be unique, and a module in an undeclared layer is an error rather
than a module that quietly matches nothing.

### rules/<id>.toml — one schema, four engines, `kind` selects

**This section is the authoritative rule schema.** §19 records where each
field came from and what is still deferred; it does not define the schema.

```toml
schema_version = 1
id = "ui-no-db-import"
status = "accepted"      # accepted | proposed | retired
kind = "boundary"        # boundary | pattern | literal | presence
severity = "error"       # error = CI exit 1 | warning = report only

[boundary]
from_layer = "ui"              # a LAYER name or a MODULE name
to_layer = "db"
allow_to = ["packages/engine/src/index.ts"]   # exempt targets
include_tests = false          # opt back into test_globs
exclude = ["src/legacy/**"]    # anything else to skip
reachable = false              # transitive? NOT YET SUPPORTED — see below
message = "UI must not import the db layer. Go through services."

# [pattern]  → scope, mode (forbidden|required|warning), pattern, message
#              pattern is REGEX-ONLY. An invalid regex is a validation error.
# [literal]  → scope, message, literal. Exact, case-sensitive, and
#              identifier-boundary aware: forbidding `legacy_client` must not
#              fire on `legacy_client_v2`.
# [presence] → applies_to, requires, message
```

`allow_to` exists because a monorepo boundary is not a layer pair: reaching
a package through its public entry is legitimate, reaching its internals is
the violation. Without it, "ui must not import db" cannot express the
difference and the rule is either useless or wrong.

`from_layer`/`to_layer` resolve as layer-first, then module name. A name
that is BOTH is only ambiguous when the two readings cover different
modules — `name = "engine", layer = "engine"` is accepted, because both
readings name the same set and cannot change what the rule means.

`pattern` and `literal` are split because one kind carrying two meanings is
a trap: today `pattern` is regex-with-a-substring-fallback, so a typo'd
regex silently degrades into a substring match and keeps passing CI. After
the split, an invalid regex fails validation and `literal` is exact.

**`reachable` is accepted only as `false`.** `reachable = true` must be
REJECTED at validation with "not supported by this build" until the engine
computes transitive closure. A field that is accepted and silently does
nothing is the worst outcome available: a human writes "ui must not import
db, directly or via anything", gets direct-only enforcement, and the rule
reads as if it were transitive. That is the silently-empty-rule failure this
design exists to prevent, arriving through the front door.

Validator rule: `kind` must match exactly one engine table. Rules are
human-authored in v1; agents may only *suggest* one inside a decision.
Rules live in FILES, never in the database: plain text in git is the source
of truth (§2.5), and a rule that exists only in a local index enforces
nothing on anyone else's machine.

**Resolved in Step 2.5:** import classification (§19.4) shipped with the
`imports.kind` column and `include_type_only`, default-skip. The schema
above is now honest about what the engine can do.

### decisions/<ULID>-<slug>.md — markdown + frontmatter

**Frontmatter is TOML, not YAML.** The body is markdown; the `---` header is
parsed as TOML, which is what `config.toml` and every rule file already use:
one parser, one `deny_unknown_fields` discipline, no second dependency. The
YAML sketch below reads the same with `=` and quoted arrays.

```markdown
---
schema_version = 1
id = 01J8K2A7B3C4D5E6F7G8H9J0KM     # ULID — sorts by time, conflict-free
title = "Use Drizzle as the ORM"
status = "proposed"                  # proposed | accepted
supersedes = []                      # ULIDs this replaces
task = "add-billing-webhooks"        # provenance
scope = ["db", "services"]
author = "human"                     # human | agent
recorded_at = "2026-10-08T14:22:00Z"
---
## Context / ## Decision / ## Alternatives rejected
```

Agent writes `status: proposed, author: agent` → merging the PR accepts it. **Superseded is derived, never written:** sync resolves the `supersedes` chain and serves only the heads, so `status = "superseded"` is REJECTED at validation for the same reason `reachable = true` is — a record cannot be trusted to vouch for its own replacement. Cycles and dangling `supersedes` entries are errors, not dropped links.

`scope` holds module or layer names, never paths: scope selects which files
get the decision injected, and an entry that can never match is a decision
nobody is ever told about. `sync` names every scope entry that matches
nothing.

**Still deferred:** `tasks/<id>.md` (the task ledger — parity with
Archcore's plans, not a wedge; it rides with the feature branch and adds
nothing to the exit test).

### decisions, as built (Step 3)

| Piece | Where |
|---|---|
| format + validator | `src/core/decision.rs` — `parse_named`, `validate`, `chain_heads` |
| one writer | `decision::write_new` — renders, validates, refuses to overwrite, then writes. CLI and MCP share it |
| index | `src/core/ledger.rs` — `sync` is hash-driven, prunes deletions, derives heads |
| injection | `get_context` carries ≤10 briefs per file, plus the count of what was left out |
| agent write path | `record_decision` — no `status` and no `author` parameter exist, so an agent cannot accept its own decision |

Files are never modified by archex. `sync` proves it: a test asserts the
committed bytes and the mtime are untouched after a run.

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

Boundary keys are `(rule, from_file, to_file)` and **never** a line number.
A violation that moves down the file because someone added an import above
it is the same violation; keying on lines makes the baseline rot into
per-line bookkeeping within a day. Verified on LORB: a recorded violation
moved from line 1 to line 6 and stayed absorbed.

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

**Honest differentiator notes (from the landscape research, §18):** the task ledger is *parity*, not a wedge — Archcore's `.plan.md` files carry tasks and their review closes them; we keep ours because state continuity feeds the loop, not because it's unique. The real edges are: (a) the integration quadrant itself, (b) one-file-per-record git-native storage (Mneme's corpus is a single `project_memory.json` — the merge-conflict trap this design exists to avoid), (c) the zero-runtime single binary.

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
| `get_context` | keep | keep (serves rules + decisions slice — **shipped**) | keep |
| `check_drift` | keep | keep (advisory pre-edit feedback, same engine as the gate — **shipped**) | keep |
| `record_decision` | — | **new (Step 3) — shipped** | keep |
| `get_module`, `find_similar` | keep | untouched | folded in |
| `create_plan` | broken | untouched (dying) | **removed** |
| `search` / `traverse` / `read` | — | — | **new (Step 4)** |

## 11. Build order

| # | Ship | Exit test — we don't advance without it | Status |
|---|---|---|---|
| **0** | Truthful core: idempotent init, symbol dedup (`clear_file_symbols` wired), shared case-correct regex helper, `exported` + signature extraction fixes, WAL + one shared connection, path normalization, dead code deleted, generic module seeding, **fixture tests + full suite** | `init` twice clean; suite green | ✅ done — 176 tests, zero warnings |
| **1** | **Eval harness**: ~20 real LORB commits, ground truth = files each commit touched; Hit@3, MRR, function accuracy; baselines = plain grep + current archex | Numbers recorded | ✅ done — see below. Run on `project/fitness`; LORB reserved for Step 2 (below) |
| **2** | **The Gate**: file-level import resolution, rules as data, `archex check` + CI job, baseline mode | ≥1 real violation caught on LORB; **zero false positives on a clean tree** | ✅ done — both halves, on LORB |
| **2.5** | **One source of truth for rules.** Gate, `check --file-path` and MCP `check_drift` load `.archex/rules/*.toml` through **one loader and one engine**. `rule add` writes a validated file through the real validator (canonical serialization; flags grow `--from --to --allow-to --severity`, or boundary rules stay CLI-unreachable); `rule remove` **retires** by default (`status = "retired"`), `--purge` deletes; `archex rule migrate` converts DB rules — refusing any whose module has no layer in `config.toml` — then the `rules` table dies **and the `imports.kind` column arrives in the same SCHEMA_VERSION 2→3 bump** (5 kinds: relative | aliased | package | type-only | dynamic; the boundary engine honors `include_type_only`). Legacy DB rules warn once on startup, then migrate | **The per-file advisory result for file X equals the gate's findings filtered to X** — measured on a fixture where X contains at least one violation, *including a type-only cross-boundary import so default-skip is exercised on both paths*, so the assertion cannot pass vacuously. Advisory and gate must never disagree, or agents learn to distrust the advisory | ✅ done — `tests/advisory_matches_gate.rs`, `tests/mcp_agrees_with_gate.rs`, `tests/import_kinds.rs` |
| **3** | **The Ledger**: file formats + validator, `archex sync`, brief injection, `decide` CLI, `record_decision` MCP tool | An agent logs a decision mid-session; the next session starts knowing it | ✅ done — `tests/record_decision.rs` drives the real stdio server: record, kill the process, ask a *new* one for context. Measured on LORB too. 235 tests |
| **4** | Retrieval surface: FTS5 word-split search, `search`/`traverse`/`read` with token budgets, stable ordering | Measurable gain over the Step 1 baseline | ⬜ not started |
| **5** | Conditional: symbol edges, PageRank repo map | Only if Step 4 leaves retrieval as the proven bottleneck | ⬜ not started |

### Measured results

**Step 1 — retrieval (project/fitness, 17 eligible commits).** The numbers
were supposed to be bad. They are:

| system | Hit@1 | Hit@3 | Hit@5 | MRR | avg candidates |
|---|---|---|---|---|---|
| grep | 17.6% | 29.4% | 35.3% | 0.227 | 171.4 |
| archex | 5.9% | 17.6% | 23.5% | 0.110 | 4.4 |

archex loses to plain grep. Three separable causes, each visible per-commit:
`find_similar` caps at 5 **symbols** (not files); symbol-name substring is
the wrong axis; and paths, which carry most of the signal, are barely read.
Every later step has to move this row.

**Step 2 — the gate, on LORB (3666 files).** Import resolution: **10,522 of
10,540 first-party imports resolve (99.8%)**; the 18 misses are CSS, JSON
locales and gitignored generated files. On fitness it is 85/88 (96.6%).

Zero false positives: **218 of 218 cross-module edges go through a package's
public entry**, so the internals rule correctly reports nothing. That is a
real pass, not an empty one — the rule loaded, had 34 from-files, 106
to-files and 19 candidate edges, and examined every one.

At least one real violation: a `domain → routes` rule found **5 genuine
violations in LORB's own code** (features importing `@/app/actions/*`),
none of them introduced by us. 14 candidate edges, 8 exempted as test
files, 5 real. Baseline mode then absorbed those 5 as recorded debt and a
sixth new one failed the build.

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

### 12.1 Decisions taken while building Steps 0–2

The plan above was frozen on 2026-10-08. Four things turned out to be wrong
or incomplete once there was real code to measure. All four were changed
with evidence, not taste.

**1. The curated Next.js seed was deleted, not kept.** The plan said
manifest-detected frameworks keep a curated module list with real layers.
Measured on LORB: **10 of the 16 curated modules matched zero files**, 4 of
the 8 seeded rules could never fire, and 77% of the repo was unclassified.
On fitness it never fired at all (no `next` in the root manifest). A
hardcoded guess about one codebase's layout is not something a manifest can
vouch for. Modules now come from `config.toml`, drafted from the workspace
layout and edited by a human.

**2. Generic seeding could not guess layers, and did not try.** Top-level
directory → module, layer = the directory name, verbatim. The reasoning
that decided it: layers are load-bearing the moment Step 2 landed. A wrong
semantic guess is not merely wrong context, it is a **false violation** from
a rule that checks `from_layer`/`to_layer`. `src/internal/db` mislabelled
`db` plus "ui must not import db" is the cries-wolf death scenario the plan
names. A layer is a stable key; humans give it meaning in `config.toml`.

**3. `config.toml` was pulled forward from Step 3.** Building the boundary
engine against layers we knew were wrong is the "complete the plan before
contact with reality" failure the plan warns about. The pull-forward was
kept tight: the config parser and validator, and rule files. `sync`, the
ledger and `decide` stayed in Step 3.

**4. Unclassified must be visible, always.** `archex check` prints files in
modules vs unclassified, and unclassified imports as their own count.
Deriving the unclassified count from the index reported **zero** uncovered
files in exactly the repos with the most of them (unmapped files never reach
`file_map`), so the scan persists its coverage counters and check reads
those. Unresolved imports are counted too — never violations, never hidden.

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

## 16. Bug triage reference (Step 0 scope) — ALL RESOLVED

Fix what is on the path — do not polish what is being deleted:

- Idempotent init (UNIQUE(name) on modules, no duplicate seeding) — done
- Wire `clear_file_symbols` before batch inserts (no symbol duplication on re-scan) — done
- Case-correct shared regex helper (compile against original content or use `(?i)`) — done
- `exported` detection: walk to parent for `export_statement` / modifiers — done (and it had been returning `false` for **every** TS symbol)
- Signature extraction: use `parameters`/`return_type` fields, cover arrow functions and function expressions — done
- WAL + one shared connection (`OnceCell`) — done (`Db::shared`, one per path)
- Path normalization (absolute → repo-relative) on lookup — done (`core/paths.rs`)
- Delete dead parser functions + unused `reqwest` dependency — done; zero warnings
- Generic module seeding (infer from directories/manifests, not the hardcoded Next.js list) — done
- README/doc corrections (no phantom `refresh`, correct DB path) — done

Bugs found *while* fixing these, which were not on the list:

- `archex rule add` printed the total rule COUNT as the new rule's ID
- `rule add` validated the rule type, then discarded the validated value
- single-quoted import specifiers were dropped from the index entirely
- re-exports (`export … from`), `require()` and dynamic `import()` were not
  edges at all
- `extends` was read from inside `compilerOptions`, dropping every tsconfig chain
- scoped package names contain a slash, so splitting `@fitness/engine` on
  the first slash tore the name in half and lost every scoped workspace link
- `clear_modules` violated FK constraints once Step 0 had actually enabled them
- a database predating schema versioning read as "fresh" and then failed on a missing column
- `init` prompted when `.archex/` existed — which, once `config.toml` is committed, is always

Explicitly **not** fixed: `create_plan` scoring and its hardcoded checklist — they die with the tool surface in Step 4.

## 17. Open items

1. ~~Rules still have two sources of truth~~ — **closed in Step 2.5**. One
   loader (`config::load_rules`), one engine (`boundary::check`) behind the
   gate, `check --file-path` and MCP `check_drift`. The `rules` table is
   gone (SCHEMA_VERSION 3), `archex rule migrate` bridges an old index.
2. ~~`check --file-path` and the pattern/presence engines still use the DB
   rule path~~ — **closed in Step 2.5** for the DB. What remains is that
   only `boundary` rules have a file-based engine at all; `pattern` and
   `presence` are stored, listed, and explicitly reported as *not enforced
   yet* rather than pretending. That engine work is not scheduled: the
   boundary engine is the one that answers the question this tool exists
   for, and it is the one that works.
3. `create_plan` is 172 lines of dead weight scheduled for removal in
   Step 4.
4. **Decisions are indexed but not searchable.** `sync` imports them and
   `get_context` injects by scope; keyword retrieval (`archex decide
   search`, FTS5) is Step 4 work alongside the symbol search surface.
5. **A schema bump empties the source index.** 3 → 4 drops the source
   tables, and `sync` deliberately does not re-parse code (`init` owns
   that). On the first run after a bump, `get_context` answers "not
   mapped — run `archex init`". Loud, but the honest fix is Step 4's
   hash-driven re-parse, not a surprise scan in the MCP server.

## 18. Landscape (verified 2026-10-10)

Researched via GitHub API and primary sources (READMEs, ADRs, rule-engine
docs). All claims below were checked against the actual repos.

| Project | What it is (verified) | State |
|---|---|---|
| **Archcore** (archcore-ai) | Spec-driven dev + git-native context. `.archcore/` docs typed by filename (`.adr.md .rule.md .plan.md .spec.md`). `/plan` (specs+tasks, S–XL sizing), `/document`, `/review` — **bidirectional** (`code-wrong` vs `spec-wrong`). Imports existing CLAUDE.md/AGENTS.md. 8 agent integrations. **No deterministic gate** — review is LLM-driven | 68★, org-backed, installer, active daily |
| **Mneme** (MnemeHQ) | Decision + control layer: ADRs → deterministic guardrails. Retrieval/enforcement explicitly separated (their ADR-017); verdicts *traced to source decisions*; frozen 6-tool MCP, agents can only **propose**, humans accept via CLI; fail-closed authority; 4-tier boundaries (injection → pre-tool hooks → post-mutation audit → CI). **Enforcement is literal text matching only** (ADR-019: exact, case-sensitive, no graph) | 25★, Python, very active, ADR-dogfooded |
| **ArchSteer** | "Living architecture control plane" — derive arch from code, auto-draft ADRs, fitness functions | 5★, stalled since July |
| **dependency-cruiser** | The mature TS/JS graph gate: forbidden/allowed/required rules, `reachable` (transitive), 25+ typed import kinds, `--ignore-known` baseline, `extends` shareable rule sets | ~5k★, category standard |
| **Sheriff, Nx boundaries, import-linter, ArchUnit** | Mature gate category; ArchUnit's `FreezingArchRule` = our baseline mode, proven since 2019 | — |

**Corrections to earlier wedge claims:** decision-linked violations — Mneme
has them (verdicts traced to decisions). Task ledger — Archcore's plans carry
tasks and review closes them; parity, not a wedge. The surviving quadrant:

| | Decisions in git | Deterministic gate | Import-graph rules |
|---|---|---|---|
| Archcore | ✓ | ✗ | ✗ |
| Mneme | ✓ | ✓ | ✗ (literal text only) |
| dep-cruiser & co. | ✗ | ✓ | ✓✓ |
| **archex** | ✓ | ✓ | ✓ |

Pitch line: *dependency-cruiser has the graph but doesn't know why; Mneme
knows why but can't see the graph.* Also structural: Mneme's corpus is a
single `project_memory.json` — the exact merge-conflict trap our
one-file-per-record design exists to avoid.

## 19. Schema and engine refinements (from the landscape, fold into 2.5–4)

Steals with sources. Each is small now, expensive after Step 3+.

1. **`literal` rule kind** (Mneme ADR-019): exact, **case-sensitive**, with
   identifier-boundary matching — forbidding `legacy_client` must not fire on
   `legacy_client_v2`. One semantic per kind: the current `pattern` kind is
   regex-with-substring-fallback — two meanings in one type, and the fallback
   fires silently. Split: `literal` (exact + boundary-aware) and `pattern`
   (regex-only — an invalid regex fails **validation**, never silently
   becomes a substring match).
2. **Rule self-reference exemption** (Mneme): a rule never evaluates against
   the file that declares it. Our rule TOMLs carry pattern text in `message`;
   the engine must not scan `.archex/` anyway — make both explicit in the
   engine spec.
3. **`reachable = false` on `[boundary]`** (dependency-cruiser): the field
   is in the schema (§5), default `false`, so Step 2 semantics are
   unchanged — but **`reachable = true` is rejected at validation until the
   engine exists.** Direct-edge-only is a toy gate (services-layer laundering
   defeats it), so the field is wanted; accepting `true` and ignoring it is
   worse than not having it, because the rule then reads as transitive while
   enforcing nothing transitive. Needs transitive closure over resolved
   edges with cycle guards — a real algorithm, not a flag.
4. **Import classification: 5 kinds** — `relative | aliased | package |
   type-only | dynamic`, everything else unclassified (dependency-cruiser has
   25; we take the 5 that matter). The killer detail: **`import type` edges
   are treated differently by default.** Measured on LORB, type-only imports
   are 713 of 10,638 edges — 6.7% of the graph — and today they are
   indistinguishable from value imports, so any internals rule flags up to
   713 false violations. Type-only imports cannot create runtime coupling,
   which is a fact; that they should therefore be *exempt* is a policy
   choice, because compile-time coupling is still coupling. So it is
   per-rule (`include_type_only`, default `true` = skip, precision-first),
   not a hardcoded exemption — see §5. dependency-cruiser's model is the
   same shape: a kind you can forbid or allow, never a blanket skip.

   **Sequencing decided (2026-10-10): the `kind` column ships in Step 2.5,
   in the same SCHEMA_VERSION 2→3 bump that drops the `rules` table.**
   **(Delivered 2026-10-11: both, in that bump.)**
   (a) §5 already carries `include_type_only` — a field whose default-skip
   behavior cannot be honored without kind data is the same
   accepted-but-does-nothing failure `reachable = true` was rejected for;
   refusing it at validation until Step 3 is absurd for a small parser
   change. (b) Two bumps in two weeks means every active machine rebuilds
   its index twice — bumps carry batches. (c) The gate is live on LORB, and
   6.7% of its edges being indistinguishable from value imports turns wrong
   the moment a rule crosses a type-heavy boundary. **Scope guard** (the
   rule-matching unit's lesson — 2.5 must not balloon): classification by
   syntax only — `import type` → type-only, `import()` → dynamic, then
   relative / aliased / package by specifier shape; one column; the
   boundary engine honoring `include_type_only`; and *nothing else* — no
   new rule kinds over import kind, no transitive work. The
   identical-findings fixture gains a type-only cross-boundary import, so
   default-skip is exercised on both paths. Honesty note: current LORB
   rules measured zero false positives across 218 cross-module edges — the
   713 type-only edges have not crossed a checked boundary *yet*. This is
   completeness for the gate that is coming, not a fix to the one that is
   here.
5. **Future, post-Step-3** (steal list): `archex init --import` — absorb
   existing CLAUDE.md/AGENTS.md/rules into decisions (Archcore proves
   demand); `archex status` coverage report — which decisions are
   *Protected / Rules-only / Guidance* (Mneme's Architecture Audit, an
   honest onboarding surface); the 4-tier boundary vocabulary (injection →
   pre-tool hooks → post-mutation audit → CI) as the target model — we ship
   tiers 1+4, hooks and audit arrive later.
