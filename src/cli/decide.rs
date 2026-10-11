use crate::core::decision::{self, Author, Decision, DecisionStatus};
use clap::Subcommand;
use std::path::{Path, PathBuf};

/// Decisions live in `.archex/decisions/<ULID>-<slug>.md` and nowhere else.
/// `archex decision add` wrote to SQLite and printed [OK], which reads like
/// a record and is not one: it is on this machine, uncommitted, and no
/// teammate's agent will ever see it.
#[derive(Subcommand)]
pub enum DecideCommands {
    /// Write a new decision file, validated exactly as sync validates it
    Add {
        /// One sentence. This is what an agent reads first.
        #[arg(long)]
        title: String,
        /// The decision itself, in one or two sentences.
        #[arg(long)]
        decision: String,
        /// Why this came up. Optional, and the half people skip.
        #[arg(long)]
        context: Option<String>,
        /// An option that was rejected. Repeatable. An ADR without
        /// alternatives is a note, not a decision.
        #[arg(long)]
        alternative: Vec<String>,
        /// Module or layer names this applies to. Repeatable. Empty means
        /// "everywhere", which is almost never what someone means.
        #[arg(long)]
        scope: Vec<String>,
        /// Provenance: the task or branch this came out of.
        #[arg(long)]
        task: Option<String>,
        /// human (default) | agent
        #[arg(long, default_value = "human")]
        author: String,
        /// proposed | accepted — defaults to proposed for an agent, since
        /// agents propose and humans accept.
        #[arg(long)]
        status: Option<String>,
        /// ULID of the decision this replaces.
        #[arg(long)]
        supersedes: Option<String>,
    },
    /// List the current decisions, newest first
    List {
        /// Only decisions scoped to this module.
        #[arg(long)]
        scope: Option<String>,
        /// Include superseded records, marked as such.
        #[arg(long)]
        include_superseded: bool,
    },
    /// Print one decision in full, by ULID or by unique slug
    Show {
        id: String,
    },
}

pub fn run(command: DecideCommands) -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::current_dir()?;
    match command {
        DecideCommands::Add {
            title,
            decision,
            context,
            alternative,
            scope,
            task,
            author,
            status,
            supersedes,
        } => add(
            &root,
            &AddArgs {
                title,
                decision,
                context,
                alternative,
                scope,
                task,
                author,
                status,
                supersedes,
            },
        ),
        DecideCommands::List {
            scope,
            include_superseded,
        } => list(&root, scope.as_deref(), include_superseded),
        DecideCommands::Show { id } => show(&root, &id),
    }
}

fn decisions_dir(root: &Path) -> PathBuf {
    root.join(".archex/decisions")
}

struct AddArgs {
    title: String,
    decision: String,
    context: Option<String>,
    alternative: Vec<String>,
    scope: Vec<String>,
    task: Option<String>,
    author: String,
    status: Option<String>,
    supersedes: Option<String>,
}

fn add(root: &Path, args: &AddArgs) -> Result<(), Box<dyn std::error::Error>> {
    let author = match args.author.to_lowercase().as_str() {
        "human" => Author::Human,
        "agent" => Author::Agent,
        other => {
            return Err(format!("unknown author '{}'. Use: human, agent", other).into())
        }
    };

    // Agents propose, humans accept. A record an agent wrote and marked
    // accepted would be an agent deciding, which is the one thing the
    // authority ordering forbids.
    let status = match args.status.as_deref().map(str::to_lowercase) {
        Some(value) if value == "proposed" => DecisionStatus::Proposed,
        Some(value) if value == "accepted" => DecisionStatus::Accepted,
        Some(other) => {
            return Err(format!(
                "unknown status '{}'. Use: proposed, accepted",
                other
            )
            .into())
        }
        None if author == Author::Agent => DecisionStatus::Proposed,
        None => DecisionStatus::Accepted,
    };

    if status == DecisionStatus::Accepted && author == Author::Agent {
        return Err(
            "an agent cannot record an accepted decision: write it as `proposed` \
             and let a human accept it in the PR"
                .into(),
        );
    }

    if let Some(superseded) = &args.supersedes {
        if !decision::is_ulid(superseded) {
            return Err(format!("`{}` is not a ULID", superseded).into());
        }
    }

    let id = decision::new_ulid();
    let slug = slugify(&args.title);
    let path = decisions_dir(root).join(format!("{}-{}.md", id, slug));

    if path.exists() {
        return Err(format!("refusing to overwrite {}", path.display()).into());
    }

    let body = render(
        &id,
        &slug,
        args.status.as_deref().map(|_| status),
        status,
        author,
        args,
    );

    // The record goes through the parser the ledger reads before it
    // touches the disk: a decision that only fails at sync was written by a
    // broken CLI.
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Decision::parse_named(&name, &body)
        .map_err(|e| format!("the decision as written is not valid: {}\nnothing was written", e))?;

    std::fs::create_dir_all(decisions_dir(root))?;
    std::fs::write(&path, &body)?;

    println!("[OK] Decision written: {}", path.display());
    match status {
        DecisionStatus::Proposed => {
            println!("  Proposed. It becomes current when a human merges it.");
        }
        _ => println!("  Commit it — the ledger is what the next agent session reads."),
    }
    if args.scope.is_empty() {
        println!(
            "  note: no scope, so this decision will not be injected into any file's \
             context. Add --scope <module> to target one."
        );
    }
    Ok(())
}

/// Canonical on purpose: fixed field order, defaults omitted, so the diff of
/// a decision is the decision.
fn render(
    id: &str,
    slug: &str,
    _requested: Option<DecisionStatus>,
    status: DecisionStatus,
    author: Author,
    args: &AddArgs,
) -> String {
    let status = match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Superseded => "superseded",
    };
    let author = match author {
        Author::Human => "human",
        Author::Agent => "agent",
    };

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("schema_version = {}\n", decision::SUPPORTED_SCHEMA_VERSION));
    out.push_str(&format!("id = {}\n", toml_string(id)));
    out.push_str(&format!("title = {}\n", toml_string(args.title.trim())));
    out.push_str(&format!("status = {}\n", toml_string(status)));
    out.push_str(&format!(
        "supersedes = [{}]\n",
        args.supersedes
            .as_deref()
            .map(toml_string)
            .unwrap_or_default()
    ));
    if let Some(task) = &args.task {
        out.push_str(&format!("task = {}\n", toml_string(task.trim())));
    }
    out.push_str(&format!("scope = [{}]\n", toml_array(&args.scope)));
    out.push_str(&format!("author = {}\n", toml_string(author)));
    out.push_str(&format!("recorded_at = {}\n", toml_string(&decision::now_rfc3339())));
    let _ = slug;
    out.push_str("---\n");
    out.push_str("## Context\n\n");
    out.push_str(args.context.as_deref().unwrap_or("").trim());
    out.push_str("\n\n## Decision\n\n");
    out.push_str(args.decision.trim());
    if !args.alternative.is_empty() {
        out.push_str("\n\n## Alternatives rejected\n\n");
        for alternative in &args.alternative {
            out.push_str("- ");
            out.push_str(alternative.trim());
            out.push('\n');
        }
    }
    out
}

/// A TOML basic string, with the two escapes that matter and a refusal for
/// the control characters that would silently mangle a record.
fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str(" "),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml_array(values: &[String]) -> String {
    values
        .iter()
        .map(|value| toml_string(value.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lowercase kebab-case, the same shape `rule add` file names use: it lands
/// in `git status` and in agent output.
fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = true;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "decision".to_string()
    } else {
        slug
    }
}

fn list(
    root: &Path,
    scope: Option<&str>,
    include_superseded: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let all = decision::load_decisions(root)?;
    if all.is_empty() {
        println!(
            "No decisions. They live in {}, one file each.",
            decisions_dir(root).display()
        );
        return Ok(());
    }

    let superseded: std::collections::HashSet<&str> = all
        .iter()
        .flat_map(|d| d.meta.supersedes.iter().map(|s| s.as_str()))
        .collect();

    let mut shown = 0usize;
    for decision in all.iter().rev() {
        let is_superseded = superseded.contains(decision.meta.id.as_str());
        if is_superseded && !include_superseded {
            continue;
        }
        if let Some(scope) = scope {
            if !decision.meta.scope.iter().any(|entry| entry == scope) {
                continue;
            }
        }
        shown += 1;
        println!(
            "{}  [{}]  {}{}",
            &decision.meta.id[..decision.meta.id.len().min(10)],
            decision.meta.status.as_str(),
            decision.meta.title,
            if is_superseded { "  (superseded)" } else { "" }
        );
        if !decision.decision.is_empty() {
            println!("      {}", first_line(&decision.decision));
        }
        if !decision.meta.scope.is_empty() {
            println!("      scope: {}", decision.meta.scope.join(", "));
        }
    }

    if shown == 0 {
        println!("No current decisions match that scope.");
    }
    Ok(())
}

fn show(root: &Path, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let all = decision::load_decisions(root)?;
    let exact = all.iter().find(|d| d.meta.id == id);
    let by_slug = all.iter().find(|d| {
        d.path
            .as_deref()
            .map(|name| {
                name.trim_end_matches(".md")
                    .split_once('-')
                    .map(|(_, slug)| slug == id)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    });
    let found = match (exact, by_slug) {
        (Some(found), _) => found,
        (None, Some(found)) => found,
        (None, None) => {
            return Err(format!(
                "no decision matches `{}`. `archex decide list` shows the ids.",
                id
            )
            .into())
        }
    };

    println!("# {}", found.meta.title);
    println!();
    println!("id:       {}", found.meta.id);
    println!("status:   {}", found.meta.status.as_str());
    println!("author:   {}", found.meta.author.as_str());
    println!("recorded: {}", found.meta.recorded_at);
    if !found.meta.scope.is_empty() {
        println!("scope:    {}", found.meta.scope.join(", "));
    }
    if !found.meta.supersedes.is_empty() {
        println!("replaces: {}", found.meta.supersedes.join(", "));
    }
    println!();
    if !found.context.is_empty() {
        println!("## Context\n\n{}\n", found.context);
    }
    println!("## Decision\n\n{}\n", found.decision);
    if !found.alternatives.is_empty() {
        println!("## Alternatives rejected\n\n{}\n", found.alternatives);
    }
    if let Some(path) = &found.path {
        println!("file: {}", path);
    }
    Ok(())
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("");
    if line.len() > 96 {
        format!("{}…", &line[..96])
    } else {
        line.to_string()
    }
}