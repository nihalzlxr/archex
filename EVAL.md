# Step 1 eval baseline

Measured 2026-10-10 against `project/fitness` (`nihalzlxr/fitness`, 757 commits),
evaluated in a local clone at `/tmp/fitness-eval`. No branch or worktree is
created in the source repo.

## Method

- **Ground truth**: the files a real commit touched (indexable extensions only).
  No merges, no reverts, ≤10 files, strided across history — the newest commits
  in this corpus are all one feature area, so taking the newest N measures one
  feature, not retrieval.
- **Query**: the commit message alone. That is what an agent would have.
  Conventional-commit type words are dropped; the scope is kept.
- **Tree**: each sample is evaluated against its **parent** commit — the state
  the developer was looking at when they wrote it.
- **Candidate universe**: identical for both systems (the files archex indexed
  for that tree), so the numbers compare *ranking*, not coverage.
- **grep**: keyword hits in the path weighted 30, body hits capped at 10/term.
- **archex**: today's `find_similar`, faithfully — tokenize, per keyword
  `LIKE %kw%` over symbol names, dedup by (file, name), take the first 5.

17 eligible commits out of a 400-commit pool.

## Numbers

| system | Hit@1 | Hit@3 | Hit@5 | MRR | avg candidates |
|---|---|---|---|---|---|
| grep | 17.6% | **29.4%** | 35.3% | 0.227 | 171.4 |
| archex | 5.9% | **17.6%** | 23.5% | 0.110 | 4.4 |

archex loses to plain grep by 11.8 points of Hit@3 and by 2× on MRR.

## Why it loses

1. **The cap.** `find_similar` counts 5 *symbols*, not files, then stops.
   grep offers 171 candidates. Most misses are the answer not being in a
   5-symbol window.
2. **Symbol-name substring is the wrong axis.** Token "validation" matches
   `substitution`/`session` symbols and lands `seed-validation.ts` at rank 5 by
   accident. `undo-tag.tsx` is missed outright because no symbol is named
   `undo-tag`.
3. **Paths carry the signal and are barely read.** For
   `web: fix post-login redirect /today -> /dashboard, update old nav links`,
   truth is `nav.tsx`; archex returns five unrelated `route.ts` files.

## Seeding note

fitness seeds 2 modules: `apps`, `packages` — both containers. Generic
top-level seeding is honest but too coarse for a pnpm monorepo, so the module
axis carries no retrieval signal here. This is a Step 3 `config.toml` problem,
not a Step 2 patch.

## Reproduce

```bash
git clone --local <repo> /tmp/<name>-eval && git -C /tmp/<name>-eval checkout --detach
ARCHEX_EVAL_REPO=/tmp/<name>-eval ARCHEX_EVAL_N=20 \
  cargo test eval -- --ignored --nocapture
```

The harness refuses to run against anything that is not a detached worktree or
clone — it checks out commits and must never move a branch in a working copy.