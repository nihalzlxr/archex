use crate::core::config::{self, Kind, Rule as FileRule, Severity, Status};
use clap::Subcommand;
use std::path::{Path, PathBuf};

/// The rule CLI is file-native: `add` writes `.archex/rules/<id>.toml`
/// through the same validator CI uses, `list` reads the committed files, and
/// `remove` retires a file rather than deleting a row. Rules in the database
/// enforce nothing on anyone else's machine.
#[derive(Subcommand)]
pub enum RuleCommands {
    /// List the rules committed in .archex/rules/
    List,
    /// Write a new rule file, validated exactly as CI validates it
    Add {
        /// Rule id, and the file name. Lowercase kebab-case.
        #[arg(long)]
        id: String,
        /// boundary | pattern | presence
        #[arg(long)]
        kind: String,
        /// error | warning (default: error)
        #[arg(long, default_value = "error")]
        severity: String,
        /// What the rule is for, in one sentence. Shown with every violation.
        #[arg(long)]
        message: String,

        // boundary
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        /// Exempt targets, repeatable. A package's public entry is the
        /// legitimate way in; its internals are what the rule is about.
        #[arg(long)]
        allow_to: Vec<String>,
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long, default_value_t = false)]
        include_tests: bool,

        // pattern
        #[arg(long)]
        scope: Option<String>,
        /// forbidden | required | warning
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        pattern: Option<String>,

        // presence
        #[arg(long)]
        applies_to: Option<String>,
        #[arg(long)]
        requires: Option<String>,
    },
    /// Retire a rule (default) or delete its file with --purge
    Remove {
        #[arg(long)]
        id: String,
        /// Delete the file instead of retiring it. Git is the undo.
        #[arg(long)]
        purge: bool,
    },
}

pub fn run(command: RuleCommands) -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::current_dir()?;
    match command {
        RuleCommands::List => list(&root),
        RuleCommands::Add {
            id,
            kind,
            severity,
            message,
            from,
            to,
            allow_to,
            exclude,
            include_tests,
            scope,
            mode,
            pattern,
            applies_to,
            requires,
        } => add(
            &root,
            &AddArgs {
                id,
                kind,
                severity,
                message,
                from,
                to,
                allow_to,
                exclude,
                include_tests,
                scope,
                mode,
                pattern,
                applies_to,
                requires,
            },
        ),
        RuleCommands::Remove { id, purge } => remove(&root, &id, purge),
    }
}

fn rules_dir(root: &Path) -> PathBuf {
    root.join(".archex/rules")
}

fn rule_path(root: &Path, id: &str) -> PathBuf {
    rules_dir(root).join(format!("{}.toml", id))
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Boundary => "boundary",
        Kind::Pattern => "pattern",
        Kind::Presence => "presence",
    }
}

fn list(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let rules = config::load_rules(root)?;
    if rules.is_empty() {
        println!(
            "No rules. Rules live in {} — one file each.",
            rules_dir(root).display()
        );
        return Ok(());
    }

    println!(
        "{:<24} {:<10} {:<10} {:<9} {}",
        "ID", "STATUS", "KIND", "SEVERITY", "SUMMARY"
    );
    println!("{}", "-".repeat(92));
    for rule in &rules {
        let summary = if let Some(b) = &rule.boundary {
            format!("{} -> {}", b.from_layer, b.to_layer)
        } else if let Some(p) = &rule.pattern {
            format!("{} {:?}", p.scope, p.mode)
        } else if let Some(p) = &rule.presence {
            format!("{} requires {}", p.applies_to, p.requires)
        } else {
            String::new()
        };
        println!(
            "{:<24} {:<10} {:<10} {:<9} {}",
            rule.id,
            format!("{:?}", rule.status).to_lowercase(),
            kind_name(rule.kind),
            format!("{:?}", rule.severity).to_lowercase(),
            summary
        );
    }
    Ok(())
}

struct AddArgs {
    id: String,
    kind: String,
    severity: String,
    message: String,
    from: Option<String>,
    to: Option<String>,
    allow_to: Vec<String>,
    exclude: Vec<String>,
    include_tests: bool,
    scope: Option<String>,
    mode: Option<String>,
    pattern: Option<String>,
    applies_to: Option<String>,
    requires: Option<String>,
}

fn add(root: &Path, args: &AddArgs) -> Result<(), Box<dyn std::error::Error>> {
    // The id becomes a file name, so it is validated as one: no traversal,
    // no spaces, no surprises in `git status`.
    validate_slug(&args.id)?;

    let kind = match args.kind.to_lowercase().as_str() {
        "boundary" => Kind::Boundary,
        "pattern" => Kind::Pattern,
        "presence" => Kind::Presence,
        other => {
            return Err(format!(
                "unknown kind '{}'. Use: boundary, pattern, presence",
                other
            )
            .into())
        }
    };

    let severity = match args.severity.to_lowercase().as_str() {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        other => {
            return Err(format!("unknown severity '{}'. Use: error, warning", other).into())
        }
    };

    let path = rule_path(root, &args.id);
    if path.exists() {
        return Err(format!(
            "refusing to overwrite {} — edit the file, or retire it with \
             `archex rule remove --id {}` first",
            path.display(),
            args.id
        )
        .into());
    }

    let body = render_rule(root, args, kind, severity)?;

    // Validate through the real validator before touching the filesystem: a
    // rule that only fails in CI was written by a broken CLI.
    FileRule::parse_str(&body)
        .map_err(|e| format!("the rule as written is not valid: {}\nnothing was written", e))?;

    std::fs::create_dir_all(rules_dir(root))?;
    std::fs::write(&path, &body)?;

    println!("[OK] Rule written: {}", path.display());
    if kind != Kind::Boundary {
        println!(
            "  note: only boundary rules are enforced today — this {} rule is \
             stored and listed, but nothing checks it yet",
            kind_name(kind)
        );
    } else {
        println!("  Commit it — the gate reads what is committed.");
    }
    Ok(())
}

/// Build the rule file. Canonical on purpose: fixed field order, defaults
/// omitted, so the diff of a rule change is the rule change.
fn render_rule(
    root: &Path,
    args: &AddArgs,
    kind: Kind,
    severity: Severity,
) -> Result<String, Box<dyn std::error::Error>> {
    // Flags that belong to the other engines. Passing them is always a
    // mistake — usually the wrong --kind — and silently dropping them turns a
    // typo into a rule that does not mean what was typed.
    let foreign: &[(&str, bool)] = &[
        ("from", args.from.is_some()),
        ("to", args.to.is_some()),
        ("allow-to", !args.allow_to.is_empty()),
        ("exclude", !args.exclude.is_empty()),
        ("include-tests", args.include_tests),
        ("scope", args.scope.is_some()),
        ("mode", args.mode.is_some()),
        ("pattern", args.pattern.is_some()),
        ("applies-to", args.applies_to.is_some()),
        ("requires", args.requires.is_some()),
    ];
    let mine: &[(&str, bool)] = match kind {
        Kind::Boundary => &[
            ("from", true),
            ("to", true),
            ("allow-to", true),
            ("exclude", true),
            ("include-tests", true),
        ],
        Kind::Pattern => &[
            ("scope", true),
            ("mode", true),
            ("pattern", true),
        ],
        Kind::Presence => &[("applies-to", true), ("requires", true)],
    };
    let rejected: Vec<&str> = foreign
        .iter()
        .filter(|(name, present)| *present && !mine.iter().any(|(m, _)| m == name))
        .map(|(name, _)| *name)
        .collect();
    if !rejected.is_empty() {
        return Err(format!(
            "--{} {} not valid for a {} rule",
            rejected.join(", --"),
            if rejected.len() == 1 { "is" } else { "are" },
            kind_name(kind)
        )
        .into());
    }

    let mut body = String::new();
    body.push_str("schema_version = 1\n");
    body.push_str(&format!("id = \"{}\"\n", toml_escape(&args.id)));
    body.push_str("status = \"accepted\"\n");
    body.push_str(&format!("kind = \"{}\"\n", kind_name(kind)));
    body.push_str(&format!(
        "severity = \"{}\"\n",
        format!("{:?}", severity).to_lowercase()
    ));
    body.push('\n');

    match kind {
        Kind::Boundary => {
            let from = args
                .from
                .as_deref()
                .ok_or("a boundary rule needs --from (the layer or module the rule protects)")?;
            let to = args
                .to
                .as_deref()
                .ok_or("a boundary rule needs --to (the layer or module it must not reach)")?;

            // A side that names nothing is a rule that matches nothing. The
            // gate would report it as suspicious, but a CLI that already knows
            // should refuse rather than write a dead rule.
            let config = config::Config::load(root).map_err(|e| {
                format!(
                    "{}\nThe rule CLI needs a committed architecture. Run `archex init` first.",
                    e
                )
            })?;
            for (flag, value) in [("--from", from), ("--to", to)] {
                let is_layer = config.layers.iter().any(|l| l == value);
                let is_module = config.modules.iter().any(|m| m.name == value);
                if !is_layer && !is_module {
                    return Err(format!(
                        "{} `{}` is neither a declared layer nor a module in config.toml",
                        flag, value
                    )
                    .into());
                }
            }

            body.push_str("[boundary]\n");
            body.push_str(&format!("from_layer = \"{}\"\n", toml_escape(from)));
            body.push_str(&format!("to_layer = \"{}\"\n", toml_escape(to)));
            if !args.allow_to.is_empty() {
                body.push_str(&format!(
                    "allow_to = [{}]\n",
                    toml_array(&args.allow_to)
                ));
            }
            if args.include_tests {
                body.push_str("include_tests = true\n");
            }
            if !args.exclude.is_empty() {
                body.push_str(&format!("exclude = [{}]\n", toml_array(&args.exclude)));
            }
            body.push_str(&format!("message = \"{}\"\n", toml_escape(&args.message)));
        }
        Kind::Pattern => {
            let scope = args
                .scope
                .as_deref()
                .ok_or("a pattern rule needs --scope (the files it applies to)")?;
            let mode = args
                .mode
                .as_deref()
                .ok_or("a pattern rule needs --mode (forbidden, required, warning)")?;
            let mode = match mode.to_lowercase().as_str() {
                "forbidden" => "forbidden",
                "required" => "required",
                "warning" => "warning",
                other => {
                    return Err(format!(
                        "unknown mode '{}'. Use: forbidden, required, warning",
                        other
                    )
                    .into())
                }
            };
            let pattern = args
                .pattern
                .as_deref()
                .ok_or("a pattern rule needs --pattern")?;

            // A pattern is a regex (PLAN.md §5). An invalid one would sit in
            // the rules directory until the pattern engine shipped and then
            // fail to compile there, in CI, with a worse message.
            regex::Regex::new(pattern).map_err(|e| format!("--pattern is not a valid regex: {}", e))?;

            body.push_str("[pattern]\n");
            body.push_str(&format!("scope = \"{}\"\n", toml_escape(scope)));
            body.push_str(&format!("mode = \"{}\"\n", mode));
            body.push_str(&format!("pattern = \"{}\"\n", toml_escape(pattern)));
            body.push_str(&format!("message = \"{}\"\n", toml_escape(&args.message)));
        }
        Kind::Presence => {
            let applies_to = args
                .applies_to
                .as_deref()
                .ok_or("a presence rule needs --applies-to")?;
            let requires = args
                .requires
                .as_deref()
                .ok_or("a presence rule needs --requires")?;
            body.push_str("[presence]\n");
            body.push_str(&format!("applies_to = \"{}\"\n", toml_escape(applies_to)));
            body.push_str(&format!("requires = \"{}\"\n", toml_escape(requires)));
            body.push_str(&format!("message = \"{}\"\n", toml_escape(&args.message)));
        }
    }

    Ok(body)
}

fn remove(root: &Path, id: &str, purge: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Same guard as add: the id is a file name.
    validate_slug(id)?;

    let path = rule_path(root, id);
    if !path.exists() {
        let mut existing: Vec<String> = std::fs::read_dir(rules_dir(root))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        name.strip_suffix(".toml").map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        existing.sort();
        return Err(format!(
            "no rule `{}`. {}",
            id,
            if existing.is_empty() {
                "There are no rule files.".to_string()
            } else {
                format!("Existing: {}", existing.join(", "))
            }
        )
        .into());
    }

    if purge {
        std::fs::remove_file(&path)?;
        println!("[OK] Rule `{}` deleted ({}).", id, path.display());
        return Ok(());
    }

    let body = std::fs::read_to_string(&path)?;

    // A decision may point at this rule id. Retiring keeps the file and its
    // history; deleting leaves a dangling reference.
    let mut retired = String::new();
    let mut replaced = false;
    for line in body.lines() {
        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let trimmed = line.trim_start();
        if !replaced && trimmed.starts_with("status") && trimmed.contains('=') {
            retired.push_str(&indent);
            retired.push_str("status = \"retired\"\n");
            replaced = true;
        } else {
            retired.push_str(line);
            retired.push('\n');
        }
    }
    if !replaced {
        return Err(format!(
            "{} has no `status =` line — edit it by hand",
            path.display()
        )
        .into());
    }

    if body.contains("status = \"retired\"") {
        println!("[OK] Rule `{}` is already retired.", id);
        return Ok(());
    }

    // Validate the retired file through the same validator: status is a
    // schema field, not a free-text edit.
    FileRule::parse_str(&retired)
        .map_err(|e| format!("the retired rule is not valid, nothing was written: {}", e))?;

    std::fs::write(&path, retired)?;
    println!(
        "[OK] Rule `{}` retired. The file stays; nothing enforces it.",
        id
    );
    Ok(())
}

/// Lowercase kebab-case: what a file name can be, what a rule id should read
/// like, and what no agent has to escape.
fn validate_slug(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--");
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a valid rule id: use lowercase letters, digits and single dashes, e.g. ui-no-db",
            id
        )
        .into())
    }
}

fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn toml_array(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("\"{}\"", toml_escape(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

// Referenced so the Status import documents intent: retire writes this value.
#[allow(dead_code)]
const _RETIRED: Status = Status::Retired;