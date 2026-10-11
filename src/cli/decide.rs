use crate::core::decision::{self, Author, Decision, DecisionStatus, NewDecision};
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
        /// ULID of the decision this replaces. Superseded is derived from
        /// this chain; it is never written into the old record.
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
    /// Print one decision in full, by ULID or by slug
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
            return Err(format!("unknown status '{}'. Use: proposed, accepted", other).into())
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

    let supersedes = match &args.supersedes {
        Some(id) if !decision::is_ulid(id) => {
            return Err(format!("`{}` is not a ULID", id).into())
        }
        Some(id) => vec![id.clone()],
        None => Vec::new(),
    };

    let new = NewDecision {
        id: decision::new_ulid(),
        title: args.title.clone(),
        decision: args.decision.clone(),
        context: args.context.clone(),
        alternatives: args.alternative.clone(),
        scope: args.scope.clone(),
        task: args.task.clone(),
        status,
        author,
        supersedes,
        recorded_at: decision::now_rfc3339(),
    };

    let path = decision::write_new(root, &new)?;

    println!("[OK] Decision written: {}/{}", root.display(), path);
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
            decision::short_id(&decision.meta.id),
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
    let found: &Decision = match (exact, by_slug) {
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
        println!("file: {}/.archex/decisions/{}", root.display(), path);
    }
    Ok(())
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if line.chars().count() > 96 {
        let head: String = line.chars().take(96).collect();
        format!("{}…", head.trim_end())
    } else {
        line.to_string()
    }
}