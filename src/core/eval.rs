//! Step 1 eval harness.
//!
//! Ground truth is the set of files a real commit touched. The question is
//! narrow: given the commit message an agent would have, does the retrieval
//! surface rank those files near the top?
//!
//! Two systems, same candidate universe (the files archex indexed for that
//! commit's tree), so the numbers compare retrieval quality rather than
//! coverage differences:
//!
//! - `grep`: plain keyword search over paths and file contents. What a dev
//!   reaches for first, and the number archex has to beat.
//! - `archex`: today's `find_similar` -- tokenize, per keyword `LIKE %kw%`
//!   on symbol name, dedup by (file, name), take the first 5. Not a strawman:
//!   this is the best retrieval the MCP surface currently offers.
//!
//! `#[ignore]`d on purpose: it needs a real repo and minutes, not seconds.
//! Run it with `cargo test eval -- --ignored --nocapture`.
//!
//! SAFETY: the harness checks out commits. It refuses to run against
//! anything that is not a registered git worktree with a detached HEAD, so it
//! cannot move a branch in someone's working checkout.

#[cfg(test)]
mod tests {
    use crate::core::db::Db;
    use crate::core::parser::Parser;
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::path::PathBuf;
    use std::process::Command;

    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "to", "for", "in", "of", "with", "and", "or", "is", "it", "that",
        "this", "on", "at", "by", "from", "be", "as", "are", "was", "were", "will", "have",
        "has", "had", "do", "does", "did", "but", "not", "we", "i", "you", "they", "he",
        "she", "its", "instead", "into", "onto", "when", "then", "than", "so", "if", "no",
    ];
    /// Conventional-commit type words: true of every commit in a repo that
    /// uses them, so they carry no retrieval signal.
    const TYPE_WORDS: &[&str] = &[
        "feat", "fix", "chore", "refactor", "test", "docs", "style", "perf", "build", "ci",
        "revert", "ui", "api", "wip",
    ];

    fn repo() -> PathBuf {
        std::env::var("ARCHEX_EVAL_REPO")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/lorb-eval"))
    }

    fn git(args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo())
            .args(args)
            .output()
            .expect("git must run");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// For probes whose exit code IS the answer (`symbolic-ref -q HEAD`
    /// exits 1 on a detached HEAD, which is the state we want).
    fn git_allow_failure(args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo())
            .args(args)
            .output()
            .expect("git must run");
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// Refuse to run anywhere that could move a branch. A working checkout on
    /// a branch is exactly what we must never check commits out in.
    fn assert_safe_target() {
        let path = repo();
        assert!(
            path.exists(),
            "eval repo {} does not exist (set ARCHEX_EVAL_REPO)",
            path.display()
        );
        let head = git_allow_failure(&["symbolic-ref", "-q", "HEAD"]);
        assert!(
            head.trim().is_empty(),
            "{} is on a branch ({}) -- the harness checks out commits and must \
             run against a detached worktree, never a working checkout",
            path.display(),
            head.trim()
        );
        // Compare canonical paths: git reports /private/tmp/... for a
        // worktree asked for as /tmp/...
        let here = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let registered = git(&["worktree", "list", "--porcelain"])
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p)))
            .any(|p| p == here);
        assert!(
            registered,
            "{} is not a registered worktree of the repo",
            path.display()
        );
    }

    fn indexable(path: &str) -> bool {
        matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str()),
            Some("ts" | "tsx" | "js" | "jsx" | "rs" | "py")
        )
    }

    struct Sample {
        sha: String,
        subject: String,
        truth: BTreeSet<String>,
    }

    /// Commits an agent could plausibly have been asked to make: not merges,
    /// not reverts, small enough that the file set is a real answer, and
    /// touching code archex can index at all.
    fn eligible_samples(pool: usize, stride: usize) -> Vec<Sample> {
        let log = git(&["log", &format!("-{}", pool), "--format=%H%x1f%P%x1f%s", "--no-merges"]);
        let mut eligible: Vec<Sample> = Vec::new();

        for line in log.lines() {
            let fields: Vec<&str> = line.split('\x1f').collect();
            if fields.len() != 3 {
                continue;
            }
            let sha = fields[0].to_string();
            let subject = fields[2].to_string();
            if subject.to_lowercase().starts_with("revert") {
                continue;
            }
            let files = git(&["show", "--format=", "--name-only", &sha]);
            let touched: Vec<String> = files
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect();
            if touched.is_empty() || touched.len() > 10 {
                continue;
            }
            let truth: BTreeSet<String> = touched
                .iter()
                .filter(|p| indexable(p))
                .cloned()
                .collect();
            if truth.is_empty() {
                continue;
            }
            eligible.push(Sample { sha, subject, truth });
        }

        // Spread the sample across history instead of taking the newest N,
        // which in this corpus is all one feature area.
        let stride = stride.max(1);
        eligible
            .into_iter()
            .step_by(stride)
            .collect::<Vec<_>>()
    }

    fn terms(subject: &str) -> Vec<String> {
        // `fix(cyra-panel): drop the shortcuts` -- the type word is true of
        // every commit in the repo and carries nothing; the SCOPE is the most
        // informative token in the line. Drop one, keep the other.
        let body = match subject.find(": ") {
            Some(i) if i < 40 => {
                let (head, tail) = subject.split_at(i);
                let scope = head
                    .split_once('(')
                    .and_then(|(_, rest)| rest.strip_suffix(')'))
                    .unwrap_or("");
                let tail = &tail[2..];
                return tokenize(&format!("{} {}", scope, tail));
            }
            _ => subject,
        };
        tokenize(body)
    }

    fn tokenize(body: &str) -> Vec<String> {
        body.split(|c: char| !c.is_alphanumeric())
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 2 && !STOP_WORDS.contains(&w.as_str()) && !TYPE_WORDS.contains(&w.as_str()))
            .collect()
    }

    /// Index the tree as it stood at `sha`'s first parent: that is the state a
    /// developer was looking at when they wrote this commit.
    fn index_parent_of(sha: &str) -> (tempfile::TempDir, std::sync::Arc<Db>, BTreeSet<String>) {
        git(&["checkout", "--detach", "--quiet", &format!("{}^", sha)]);

        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::shared(&dir.path().join("eval.db")).expect("open db");
        db.init_schema().expect("init schema");
        // Seed exactly the way `archex init` does. Without this the scanner
        // matches no module and indexes nothing: the eval would be scoring an
        // empty index and calling it a baseline.
        let plan = crate::cli::init::seeding_plan(
            &repo(),
            &crate::cli::init::detect_project_type(&repo()),
        );
        crate::cli::init::seed_modules(&db, &plan);
        let scan = Parser::new(db.clone()).scan(&repo()).expect("scan");
        eprintln!(
            "  indexed {} modules, {} files mapped ({} symbols)",
            plan.len(),
            scan.files_mapped,
            scan.symbols_extracted
        );

        let universe: BTreeSet<String> = db.indexed_files().expect("files").into_iter().collect();
        (dir, db, universe)
    }

    /// Plain grep, the way a developer would run it: keyword hits in the path
    /// count for more than keyword hits in the body.
    fn grep_rank(query: &[String], universe: &BTreeSet<String>) -> Vec<String> {
        let mut scored: Vec<(usize, String)> = Vec::new();
        for path in universe {
            let body = match std::fs::read_to_string(repo().join(path)) {
                Ok(b) => b.to_lowercase(),
                Err(_) => continue,
            };
            let lower_path = path.to_lowercase();
            let mut score = 0;
            for term in query {
                if lower_path.contains(term.as_str()) {
                    score += 30;
                }
                score += body.matches(term.as_str()).count().min(10);
            }
            if score > 0 {
                scored.push((score, path.clone()));
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, p)| p).collect()
    }

    /// Today's `find_similar`, faithfully: per keyword a `LIKE %kw%` over
    /// symbol names, dedup by (file, name), take the first 5 symbols.
    fn archex_rank(db: &Db, query: &[String]) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut files: Vec<String> = Vec::new();
        let mut taken = 0;

        'outer: for keyword in query {
            let Ok(symbols) = db.search_symbols(keyword) else { continue };
            for symbol in symbols {
                let key = format!("{}:{}", symbol.file_path, symbol.name);
                if !seen.insert(key) {
                    continue;
                }
                if !files.contains(&symbol.file_path) {
                    files.push(symbol.file_path);
                }
                taken += 1;
                if taken >= 5 {
                    break 'outer;
                }
            }
        }
        files
    }

    fn rank_of(ranked: &[String], truth: &BTreeSet<String>) -> Option<usize> {
        ranked
            .iter()
            .position(|f| truth.contains(f))
            .map(|i| i + 1)
    }

    #[test]
    #[ignore = "needs a real repo; run: cargo test eval -- --ignored --nocapture"]
    fn eval_retrieval_against_real_commits() {
        assert_safe_target();

        let pool: usize = std::env::var("ARCHEX_EVAL_POOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        let stride: usize = std::env::var("ARCHEX_EVAL_STRIDE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);
        let want: usize = std::env::var("ARCHEX_EVAL_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);

        let samples = eligible_samples(pool, stride);
        assert!(!samples.is_empty(), "no eligible commits found");
        let samples: Vec<Sample> = samples.into_iter().take(want).collect();
        eprintln!("{} eligible commits, evaluating {}", pool, samples.len());

        let mut stats: HashMap<&str, (usize, usize, usize, usize, f64, usize)> = HashMap::new();
        // (hit@1, hit@3, hit@5, total, sum 1/rank, total candidates)

        for (i, sample) in samples.iter().enumerate() {
            let (_keep, db, universe) = index_parent_of(&sample.sha);
            let query = terms(&sample.subject);

            let grep = grep_rank(&query, &universe);
            let archex = archex_rank(&db, &query);

            let mut line = format!(
                "[{:2}/{}] {} {}\n     truth: {}\n     grep  rank={:?} top5={:?}",
                i + 1,
                samples.len(),
                &sample.sha[..9],
                sample.subject,
                sample
                    .truth
                    .iter()
                    .map(|t| t.rsplit('/').next().unwrap_or(t).to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                rank_of(&grep, &sample.truth),
                grep.iter().take(5).cloned().collect::<Vec<_>>()
            );
            line.push_str(&format!(
                "\n     archex rank={:?} top5={:?}",
                rank_of(&archex, &sample.truth),
                archex.iter().take(5).cloned().collect::<Vec<_>>()
            ));
            eprintln!("{}", line);

            for (name, ranked) in [("grep", &grep), ("archex", &archex)] {
                let entry = stats.entry(name).or_insert((0, 0, 0, 0, 0.0, 0));
                entry.3 += 1;
                entry.5 += ranked.len();
                match rank_of(ranked, &sample.truth) {
                    Some(1) => {
                        entry.0 += 1;
                        entry.1 += 1;
                        entry.2 += 1;
                        entry.4 += 1.0;
                    }
                    Some(3) => {
                        entry.1 += 1;
                        entry.2 += 1;
                        entry.4 += 1.0 / 3.0;
                    }
                    Some(5) => {
                        entry.2 += 1;
                        entry.4 += 1.0 / 5.0;
                    }
                    Some(_) => {}
                    None => {}
                }
            }
        }

        eprintln!("\n=== Step 1 baseline: retrieval vs real commits ===");
        eprintln!(
            "{:<8} {:>7} {:>7} {:>7} {:>7} {:>12}",
            "system", "Hit@1", "Hit@3", "Hit@5", "MRR", "avg cands"
        );
        for name in ["grep", "archex"] {
            let (h1, h3, h5, total, mrr, cands) = stats[name];
            eprintln!(
                "{:<8} {:>6.1}% {:>6.1}% {:>6.1}% {:>7.3} {:>12.1}",
                name,
                100.0 * h1 as f64 / total as f64,
                100.0 * h3 as f64 / total as f64,
                100.0 * h5 as f64 / total as f64,
                mrr / total as f64,
                cands as f64 / total as f64
            );
        }
    }
}