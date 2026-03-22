// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Unified CLI for org-mode: lint, format, query, clock, export, archive.

mod clock;
mod date;
mod export;
mod query;

use std::io::BufRead;
use std::path::PathBuf;
use std::process;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::id::{self, IdGenerator};
use org_tools_core::locator::{resolve_locator, OrgLocator};
use org_tools_core::output::{render_diagnostics, OutputFormat};
use org_tools_core::runner::Runner;
use org_tools_core::source::SourceFile;

/// Unified CLI for org-mode files.
#[derive(Parser)]
#[command(
    name = "org",
    about = "Unified CLI for org-mode: lint, format, query, clock, export"
)]
struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    command: Option<OrgCommand>,

    /// Generate shell completions and exit.
    #[arg(long, value_enum)]
    completions: Option<Shell>,
}

/// Top-level subcommands.
#[derive(Subcommand)]
enum OrgCommand {
    /// Lint and format org files.
    Fmt {
        #[command(subcommand)]
        command: FmtCommand,

        /// Path to config file.
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
    /// Query org entries.
    Query {
        #[command(subcommand)]
        command: QueryCommand,
    },
    /// Update org entries (add IDs, modify properties).
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Clock time tracking and reports.
    Clock {
        #[command(subcommand)]
        command: ClockCommand,
    },
    /// Export org entries to calendar formats.
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
    /// Archive done entries to a separate file.
    Archive {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Archive target (file::heading). Overrides #+ARCHIVE: setting.
        #[arg(long)]
        target: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,

        /// Print what would be archived without writing files.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Subcommands for `org query`.
#[derive(Subcommand)]
enum QueryCommand {
    /// Search for entries matching a query.
    Search {
        /// Query expression (e.g., "todo:TODO tags:work").
        query: String,

        /// Files or directories to search.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,

        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show agenda view (scheduled/deadline timeline).
    Agenda {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Number of days to show.
        #[arg(long, default_value = "7")]
        days: usize,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,
    },
    /// Show blocked entries with dependency details.
    Blocked {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,

        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// GTD next actions: actionable tasks grouped by context.
    Next {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Filter to a specific @context tag.
        #[arg(long)]
        context: Option<String>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,

        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// GTD inbox: entries with TODO keywords but no tags and no planning dates.
    Inbox {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,

        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// GTD waiting-for: entries in a waiting state.
    Waiting {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,

        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// GTD stuck projects: projects with no actionable children.
    Stuck {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,

        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show org-edna dependency graph.
    Deps {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "mermaid")]
        format: DepsOutputFormat,

        /// Only include entries with TODO keywords.
        #[arg(long, default_value = "true")]
        todo_only: bool,
    },
}

/// Output format for dependency graphs.
#[derive(Clone, ValueEnum)]
enum DepsOutputFormat {
    /// Mermaid diagram syntax.
    Mermaid,
    /// PlantUML syntax.
    Plantuml,
    /// Graphviz DOT format.
    Dot,
    /// JSON array of edges.
    Json,
}

/// Subcommands for `org update`.
#[derive(Subcommand)]
enum UpdateCommand {
    /// Add :ID: properties to entries that lack them.
    AddId {
        /// Targets: file paths, directory paths, or org locator strings.
        targets: Vec<String>,

        /// Include descendant entries when targeting via locator.
        #[arg(long, short)]
        recursive: bool,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,

        /// Generate IDs from a template with {placeholders}.
        ///
        /// Available: {uuid}, {uuid_short}, {file_stem}, {title_slug},
        /// {level}, {index}, {ts}.
        #[arg(long, conflicts_with = "id_command")]
        id_format: Option<String>,

        /// Pipe entry metadata (JSON) to a command that outputs the ID.
        #[arg(long, conflicts_with = "id_format")]
        id_command: Option<String>,
    },
    /// Change the TODO state of entries.
    SetState {
        /// New TODO keyword (e.g., DONE, TODO, NEXT).
        state: String,

        /// Targets: file paths, directory paths, or org locator strings.
        targets: Vec<String>,

        /// Read additional locator targets from stdin.
        #[arg(long)]
        stdin: bool,

        /// Don't add/remove CLOSED: timestamp on state transitions.
        #[arg(long)]
        no_closed: bool,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Add a new TODO entry.
    AddTodo {
        /// Title for the new entry.
        title: String,

        /// Target file or locator for parent entry.
        #[arg(default_value = ".")]
        target: String,

        /// Heading level (default: 1, or parent+1 if --parent is used).
        #[arg(long)]
        level: Option<usize>,

        /// TODO keyword (default: TODO).
        #[arg(long, default_value = "TODO")]
        keyword: String,

        /// Priority letter (A-Z).
        #[arg(long)]
        priority: Option<char>,

        /// Tags (comma-separated).
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,

        /// SCHEDULED date (YYYY-MM-DD).
        #[arg(long)]
        scheduled: Option<String>,

        /// DEADLINE date (YYYY-MM-DD).
        #[arg(long)]
        deadline: Option<String>,

        /// Insert as child of this locator target.
        #[arg(long)]
        parent: Option<String>,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Update statistic cookies ([/] and [%]) on headings.
    AddCookie {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Insert cookies on headings that don't have one yet.
        #[arg(long, short)]
        recursive: bool,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Subcommands for `org clock`.
#[derive(Subcommand)]
enum ClockCommand {
    /// Show clock time report.
    Report {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Start date (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: ClockOutputFormat,

        /// Group time by entry, tag, or day.
        #[arg(long, value_enum, default_value = "entry")]
        group_by: clock::report::GroupBy,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Show running clocks.
    Status {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: ClockOutputFormat,
    },
}

/// Subcommands for `org export`.
#[derive(Subcommand)]
enum ExportCommand {
    /// Export to iCalendar (.ics) format.
    Ical {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Start date filter (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date filter (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Export to JSCalendar (JSON) format.
    Jscal {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Start date filter (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date filter (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
}

/// Output format for clock commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ClockOutputFormat {
    /// Human-readable output.
    Human,
    /// JSON output.
    Json,
}

/// Output format for query results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum QueryOutputFormat {
    /// Human-readable output.
    Human,
    /// JSON output (validated against schema).
    Json,
    /// One OrgLocator string per line.
    Locator,
}

/// Subcommands for `org fmt`.
#[derive(Subcommand)]
enum FmtCommand {
    /// Lint org files and report diagnostics.
    Check {
        /// Files or directories to check.
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,

        /// Auto-fix fixable issues (applies format rule fixes in-place).
        #[arg(long)]
        fix: bool,
    },
    /// Format org files.
    Format {
        /// Files or directories to format.
        paths: Vec<PathBuf>,

        /// Check mode: exit 1 if changes needed, don't modify files.
        #[arg(long)]
        check: bool,

        /// Write to stdout instead of modifying files.
        #[arg(long)]
        stdout: bool,
    },
}

/// Loads configuration from an explicit path or by searching ancestor directories.
fn load_config(cli_config: &Option<PathBuf>) -> Config {
    if let Some(path) = cli_config {
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(contents) => match toml::from_str::<Config>(&contents) {
                    Ok(config) => return config,
                    Err(e) => {
                        eprintln!("org: error parsing {}: {}", path.display(), e);
                        process::exit(2);
                    }
                },
                Err(e) => {
                    eprintln!("org: error reading {}: {}", path.display(), e);
                    process::exit(2);
                }
            }
        } else {
            eprintln!("org: config file not found: {}", path.display());
            process::exit(2);
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Config::load(&cwd)
}

fn main() {
    let cli = Cli::parse();

    // Handle --completions flag.
    if let Some(shell) = cli.completions {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "org", &mut std::io::stdout());
        process::exit(0);
    }

    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            Cli::command().print_help().ok();
            process::exit(0);
        }
    };

    let exit_code = match command {
        OrgCommand::Fmt { command, config } => {
            let config = load_config(&config);
            let runner = Runner::new(config);
            run_fmt(command, &runner)
        }
        OrgCommand::Query { command } => run_query(command),
        OrgCommand::Update { command } => run_update(command),
        OrgCommand::Clock { command } => run_clock(command),
        OrgCommand::Export { command } => run_export(command),
        OrgCommand::Archive {
            paths,
            target,
            tags,
            dry_run,
        } => run_archive(paths, target, tags, dry_run),
    };

    process::exit(exit_code);
}

/// Runs the `fmt` subcommand (check or format).
fn run_fmt(command: FmtCommand, runner: &Runner) -> i32 {
    match command {
        FmtCommand::Check { paths, format, fix } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            if fix {
                let mut has_issues = false;
                for file in &files {
                    match SourceFile::from_path(file) {
                        Ok(source) => {
                            let (formatted, lint_diags) = runner.format(&source);
                            let changed = formatted != source.content;

                            if changed {
                                if let Err(e) = std::fs::write(file, &formatted) {
                                    eprintln!("org: error writing {}: {}", file.display(), e);
                                } else {
                                    println!("Fixed: {}", file.display());
                                }
                            }

                            if !lint_diags.is_empty() {
                                has_issues = true;
                                print!("{}", render_diagnostics(&lint_diags, format));
                            }
                        }
                        Err(e) => {
                            eprintln!("org: error reading {}: {}", file.display(), e);
                        }
                    }
                }
                if has_issues {
                    1
                } else {
                    0
                }
            } else {
                let mut all_diagnostics = Vec::new();
                for file in &files {
                    match SourceFile::from_path(file) {
                        Ok(source) => {
                            all_diagnostics.extend(runner.check(&source));
                        }
                        Err(e) => {
                            eprintln!("org: error reading {}: {}", file.display(), e);
                        }
                    }
                }

                if !all_diagnostics.is_empty() {
                    print!("{}", render_diagnostics(&all_diagnostics, format));
                    1
                } else {
                    0
                }
            }
        }
        FmtCommand::Format {
            paths,
            check,
            stdout,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let mut has_changes = false;
            let mut has_lint_issues = false;

            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => {
                        let (formatted, lint_diags) = runner.format(&source);
                        let changed = formatted != source.content;

                        if changed {
                            has_changes = true;
                        }

                        if !lint_diags.is_empty() {
                            has_lint_issues = true;
                            print!("{}", render_diagnostics(&lint_diags, OutputFormat::Human));
                        }

                        if check {
                            if changed {
                                println!("Would reformat: {}", file.display());
                            }
                        } else if stdout {
                            print!("{}", formatted);
                        } else if changed {
                            if let Err(e) = std::fs::write(file, &formatted) {
                                eprintln!("org: error writing {}: {}", file.display(), e);
                            } else {
                                println!("Formatted: {}", file.display());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("org: error reading {}: {}", file.display(), e);
                    }
                }
            }

            if (check && has_changes) || has_lint_issues {
                1
            } else {
                0
            }
        }
    }
}

/// Load all org documents from the given paths.
fn load_docs(paths: &[PathBuf]) -> Vec<OrgDocument> {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return Vec::new();
    }
    let mut docs: Vec<OrgDocument> = Vec::new();
    for file in &files {
        match SourceFile::from_path(file) {
            Ok(source) => docs.push(OrgDocument::from_source(&source)),
            Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
        }
    }
    docs
}

/// Runs the `query` subcommand.
fn run_query(command: QueryCommand) -> i32 {
    match command {
        QueryCommand::Search {
            query: query_str,
            paths,
            format,
            limit,
        } => {
            let pred = match query::parser::parse_query(&query_str) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("org: {e}");
                    return 2;
                }
            };

            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let today = date::current_date();
            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
            let mut matches: Vec<query::output::MatchedEntry<'_>> = Vec::new();
            for doc in &docs {
                for (idx, entry) in doc.entries.iter().enumerate() {
                    if query::predicate::matches(&pred, entry, doc, &doc_refs, today) {
                        matches.push(query::output::MatchedEntry {
                            doc,
                            entry_idx: idx,
                        });
                    }
                }
            }

            if let Some(n) = limit {
                matches.truncate(n);
            }

            if matches.is_empty() {
                return 1;
            }

            match format {
                QueryOutputFormat::Human => print!("{}", query::output::render_human(&matches)),
                QueryOutputFormat::Json => {
                    print!("{}", query::output::render_json(&matches, &doc_refs))
                }
                QueryOutputFormat::Locator => {
                    print!("{}", query::output::render_locators(&matches))
                }
            }

            0
        }
        QueryCommand::Agenda {
            paths,
            days,
            format,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let today = date::current_date();
            let agenda_days = query::agenda::build_agenda(&docs, today, days);

            let has_items = agenda_days.iter().any(|d| !d.items.is_empty());

            match format {
                QueryOutputFormat::Human => {
                    print!("{}", query::agenda::render_agenda_human(&agenda_days));
                }
                QueryOutputFormat::Json => {
                    // Agenda JSON: array of day objects.
                    let json_days: Vec<serde_json::Value> = agenda_days
                        .iter()
                        .filter(|d| !d.items.is_empty())
                        .map(|d| {
                            let items: Vec<serde_json::Value> = d
                                .items
                                .iter()
                                .map(|item| {
                                    serde_json::json!({
                                        "file": item.file.display().to_string(),
                                        "line": item.entry.heading_line,
                                        "keyword": item.entry.keyword,
                                        "priority": item.entry.priority.map(|p| p.to_string()),
                                        "title": item.entry.title,
                                        "tags": item.entry.tags,
                                        "kind": format!("{:?}", item.kind),
                                        "time": match (item.timestamp.hour, item.timestamp.minute) {
                                            (Some(h), Some(m)) => Some(format!("{h:02}:{m:02}")),
                                            _ => None,
                                        },
                                    })
                                })
                                .collect();
                            serde_json::json!({
                                "date": format!("{:04}-{:02}-{:02}", d.year, d.month, d.day),
                                "items": items,
                            })
                        })
                        .collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json_days).unwrap_or_default()
                    );
                }
                QueryOutputFormat::Locator => {
                    for d in &agenda_days {
                        for item in &d.items {
                            // Find entry index in doc to generate locator.
                            // Since we don't have easy access to the doc ref here,
                            // output file:line format.
                            println!("{}::{}", item.file.display(), item.entry.heading_line);
                        }
                    }
                }
            }

            if has_items {
                0
            } else {
                1
            }
        }
        QueryCommand::Blocked {
            paths,
            format,
            limit,
        } => {
            let docs = load_docs(&paths);
            if docs.is_empty() {
                return 2;
            }
            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();

            let today = date::current_date();
            let pred = query::parser::Predicate::Blocked;
            let mut matches: Vec<query::output::MatchedEntry<'_>> = Vec::new();
            for doc in &docs {
                for (idx, entry) in doc.entries.iter().enumerate() {
                    if query::predicate::matches(&pred, entry, doc, &doc_refs, today) {
                        matches.push(query::output::MatchedEntry {
                            doc,
                            entry_idx: idx,
                        });
                    }
                }
            }

            if let Some(n) = limit {
                matches.truncate(n);
            }

            if matches.is_empty() {
                eprintln!("org: no blocked entries found");
                return 1;
            }

            match format {
                QueryOutputFormat::Human => {
                    print!(
                        "{}",
                        query::output::render_blocked_human(&matches, &doc_refs)
                    )
                }
                QueryOutputFormat::Json => {
                    print!(
                        "{}",
                        query::output::render_blocked_json(&matches, &doc_refs)
                    )
                }
                QueryOutputFormat::Locator => {
                    print!("{}", query::output::render_locators(&matches))
                }
            }

            0
        }
        QueryCommand::Next {
            paths,
            context,
            format,
            limit,
        } => {
            let docs = load_docs(&paths);
            if docs.is_empty() {
                return 2;
            }
            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();

            let today = date::current_date();
            let pred = query::parser::Predicate::Actionable;
            let mut matches: Vec<query::output::MatchedEntry<'_>> = Vec::new();
            for doc in &docs {
                for (idx, entry) in doc.entries.iter().enumerate() {
                    if query::predicate::matches(&pred, entry, doc, &doc_refs, today) {
                        // If a context filter is set, check for that tag.
                        if let Some(ref ctx_tag) = context {
                            let inherited = doc.inherited_tags(idx);
                            if !inherited.iter().any(|t| t.eq_ignore_ascii_case(ctx_tag)) {
                                continue;
                            }
                        }
                        matches.push(query::output::MatchedEntry {
                            doc,
                            entry_idx: idx,
                        });
                    }
                }
            }

            if let Some(n) = limit {
                matches.truncate(n);
            }

            if matches.is_empty() {
                eprintln!("org: no actionable entries found");
                return 1;
            }

            match format {
                QueryOutputFormat::Human => {
                    print!("{}", query::output::render_grouped_by_context(&matches))
                }
                QueryOutputFormat::Json => {
                    print!("{}", query::output::render_json(&matches, &doc_refs))
                }
                QueryOutputFormat::Locator => {
                    print!("{}", query::output::render_locators(&matches))
                }
            }

            0
        }
        QueryCommand::Inbox {
            paths,
            format,
            limit,
        } => {
            let docs = load_docs(&paths);
            if docs.is_empty() {
                return 2;
            }
            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();

            let today = date::current_date();
            let mut matches: Vec<query::output::MatchedEntry<'_>> = Vec::new();
            for doc in &docs {
                for (idx, entry) in doc.entries.iter().enumerate() {
                    // Inbox = has a TODO keyword, not done, no tags, no SCHEDULED/DEADLINE.
                    let has_todo = entry.keyword.is_some()
                        && !entry
                            .keyword
                            .as_deref()
                            .is_some_and(|k| doc.todo_keywords.is_done(k));
                    if !has_todo {
                        continue;
                    }
                    let all_tags = doc.inherited_tags(idx);
                    if !all_tags.is_empty() {
                        continue;
                    }
                    if entry.planning.scheduled.is_some() || entry.planning.deadline.is_some() {
                        continue;
                    }
                    matches.push(query::output::MatchedEntry {
                        doc,
                        entry_idx: idx,
                    });
                }
            }
            // Suppress unused variable warning — today is used in other branches.
            let _ = today;

            if let Some(n) = limit {
                matches.truncate(n);
            }

            if matches.is_empty() {
                eprintln!("org: inbox is empty");
                return 1;
            }

            match format {
                QueryOutputFormat::Human => print!("{}", query::output::render_human(&matches)),
                QueryOutputFormat::Json => {
                    print!("{}", query::output::render_json(&matches, &doc_refs))
                }
                QueryOutputFormat::Locator => {
                    print!("{}", query::output::render_locators(&matches))
                }
            }

            0
        }
        QueryCommand::Waiting {
            paths,
            format,
            limit,
        } => {
            let docs = load_docs(&paths);
            if docs.is_empty() {
                return 2;
            }
            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();

            let today = date::current_date();
            let pred = query::parser::Predicate::Waiting;
            let mut matches: Vec<query::output::MatchedEntry<'_>> = Vec::new();
            for doc in &docs {
                for (idx, entry) in doc.entries.iter().enumerate() {
                    if query::predicate::matches(&pred, entry, doc, &doc_refs, today) {
                        matches.push(query::output::MatchedEntry {
                            doc,
                            entry_idx: idx,
                        });
                    }
                }
            }

            if let Some(n) = limit {
                matches.truncate(n);
            }

            if matches.is_empty() {
                eprintln!("org: no waiting entries found");
                return 1;
            }

            match format {
                QueryOutputFormat::Human => {
                    print!("{}", query::output::render_waiting_human(&matches))
                }
                QueryOutputFormat::Json => {
                    print!("{}", query::output::render_json(&matches, &doc_refs))
                }
                QueryOutputFormat::Locator => {
                    print!("{}", query::output::render_locators(&matches))
                }
            }

            0
        }
        QueryCommand::Stuck {
            paths,
            format,
            limit,
        } => {
            let docs = load_docs(&paths);
            if docs.is_empty() {
                return 2;
            }
            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();

            let today = date::current_date();
            let stuck = query::output::find_stuck_projects(&docs, &doc_refs, today);

            if let Some(n) = limit {
                let stuck = &stuck[..stuck.len().min(n)];
                if stuck.is_empty() {
                    eprintln!("org: no stuck projects found");
                    return 1;
                }
                match format {
                    QueryOutputFormat::Human => {
                        print!("{}", query::output::render_stuck_human(stuck))
                    }
                    QueryOutputFormat::Json => {
                        print!("{}", query::output::render_stuck_json(stuck))
                    }
                    QueryOutputFormat::Locator => {
                        for sp in stuck {
                            let loc =
                                org_tools_core::locator::locator_for_entry(sp.doc, sp.entry_idx);
                            println!("{loc}");
                        }
                    }
                }
                return 0;
            }

            if stuck.is_empty() {
                eprintln!("org: no stuck projects found");
                return 1;
            }

            match format {
                QueryOutputFormat::Human => {
                    print!("{}", query::output::render_stuck_human(&stuck))
                }
                QueryOutputFormat::Json => {
                    print!("{}", query::output::render_stuck_json(&stuck))
                }
                QueryOutputFormat::Locator => {
                    for sp in &stuck {
                        let loc = org_tools_core::locator::locator_for_entry(sp.doc, sp.entry_idx);
                        println!("{loc}");
                    }
                }
            }

            0
        }
        QueryCommand::Deps {
            paths,
            format,
            todo_only: _,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
            let edges = query::deps::extract_edges(&doc_refs);

            if edges.is_empty() {
                eprintln!("org: no edna dependencies found");
                return 1;
            }

            match format {
                DepsOutputFormat::Mermaid => {
                    print!("{}", query::deps::render_mermaid(&edges, &doc_refs))
                }
                DepsOutputFormat::Plantuml => print!("{}", query::deps::render_plantuml(&edges)),
                DepsOutputFormat::Dot => print!("{}", query::deps::render_dot(&edges)),
                DepsOutputFormat::Json => println!("{}", query::deps::render_json(&edges)),
            }

            0
        }
    }
}

/// Runs the `update` subcommand.
fn run_update(command: UpdateCommand) -> i32 {
    match command {
        UpdateCommand::AddId {
            targets,
            recursive,
            dry_run,
            id_format,
            id_command,
        } => run_add_id(targets, recursive, dry_run, id_format, id_command),
        UpdateCommand::SetState {
            state,
            targets,
            stdin,
            no_closed,
            dry_run,
        } => run_set_state(state, targets, stdin, no_closed, dry_run),
        UpdateCommand::AddTodo {
            title,
            target,
            level,
            keyword,
            priority,
            tags,
            scheduled,
            deadline,
            parent,
            dry_run,
        } => run_add_todo(
            title, target, level, keyword, priority, tags, scheduled, deadline, parent, dry_run,
        ),
        UpdateCommand::AddCookie {
            paths,
            recursive,
            dry_run,
        } => run_add_cookie(paths, recursive, dry_run),
    }
}

/// Runs `org update add-id`.
fn run_add_id(
    targets: Vec<String>,
    recursive: bool,
    dry_run: bool,
    id_format: Option<String>,
    id_command: Option<String>,
) -> i32 {
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return 2;
    }

    let generator = if let Some(tpl) = id_format {
        IdGenerator::Template(tpl)
    } else if let Some(cmd) = id_command {
        IdGenerator::Command(cmd)
    } else {
        IdGenerator::Uuid
    };

    // Resolve targets into (file_path, entry_indices) pairs.
    // entry_indices = None means all entries in the file.
    let mut file_targets: std::collections::HashMap<PathBuf, Option<Vec<usize>>> =
        std::collections::HashMap::new();

    for target in &targets {
        // Try to parse as a locator first.
        if let Ok(locator) = OrgLocator::parse(target) {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match resolve_locator(&locator, &[cwd]) {
                Ok(resolved) => {
                    let source = match SourceFile::from_path(&resolved.file) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("org: error reading {}: {}", resolved.file.display(), e);
                            return 2;
                        }
                    };
                    let doc = OrgDocument::from_source(&source);

                    let indices = if recursive {
                        id::collect_subtree(&doc, resolved.entry_index)
                    } else {
                        vec![resolved.entry_index]
                    };

                    // Merge with existing targets for this file.
                    let entry = file_targets
                        .entry(resolved.file.clone())
                        .or_insert(Some(Vec::new()));
                    if let Some(ref mut existing) = entry {
                        existing.extend(indices);
                    }
                }
                Err(e) => {
                    eprintln!("org: {e}");
                    return 2;
                }
            }
        } else {
            // Treat as file/directory path.
            let path = PathBuf::from(target);
            let files = collect_org_files(&[path]);
            if files.is_empty() {
                eprintln!("org: no .org files found in {target}");
                return 2;
            }
            for file in files {
                // None = all entries.
                file_targets.insert(file, None);
            }
        }
    }

    let mut total_added = 0;

    // Sort file paths for deterministic output order.
    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));

    for (file, indices) in file_list {
        let source = match SourceFile::from_path(&file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("org: error reading {}: {}", file.display(), e);
                continue;
            }
        };
        let doc = OrgDocument::from_source(&source);

        let result = match id::add_ids(&source, &doc, indices.as_deref(), &generator) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("org: error processing {}: {}", file.display(), e);
                continue;
            }
        };

        total_added += result.ids_added;

        if dry_run {
            println!(
                "Would add {} ID{} to {}",
                result.ids_added,
                if result.ids_added == 1 { "" } else { "s" },
                file.display()
            );
        } else {
            if let Err(e) = std::fs::write(&file, &result.content) {
                eprintln!("org: error writing {}: {}", file.display(), e);
                continue;
            }
            println!(
                "Added {} ID{} to {}",
                result.ids_added,
                if result.ids_added == 1 { "" } else { "s" },
                file.display()
            );
        }
    }

    if total_added == 0 && !dry_run {
        println!("All entries already have IDs");
    }

    0
}

/// Runs the `clock` subcommand.
fn run_clock(command: ClockCommand) -> i32 {
    match command {
        ClockCommand::Report {
            paths,
            from,
            to,
            format,
            group_by,
            tags,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let from_date = from.as_deref().and_then(date::parse_date);
            let to_date = to.as_deref().and_then(date::parse_date);

            if from.is_some() && from_date.is_none() {
                eprintln!("org: invalid --from date (expected YYYY-MM-DD)");
                return 2;
            }
            if to.is_some() && to_date.is_none() {
                eprintln!("org: invalid --to date (expected YYYY-MM-DD)");
                return 2;
            }

            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let rows = clock::report::build_report(&docs, from_date, to_date, group_by, &tags);

            match format {
                ClockOutputFormat::Human => {
                    print!("{}", clock::report::render_human(&rows, group_by))
                }
                ClockOutputFormat::Json => {
                    println!("{}", clock::report::render_json(&rows, group_by))
                }
            }

            if rows.is_empty() {
                1
            } else {
                0
            }
        }
        ClockCommand::Status { paths, format } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let running = clock::status::find_running_clocks(&docs);

            match format {
                ClockOutputFormat::Human => print!("{}", clock::status::render_human(&running)),
                ClockOutputFormat::Json => println!("{}", clock::status::render_json(&running)),
            }

            if running.is_empty() {
                1
            } else {
                0
            }
        }
    }
}

/// Runs the `export` subcommand.
fn run_export(command: ExportCommand) -> i32 {
    match command {
        ExportCommand::Ical {
            paths,
            output,
            from,
            to,
            tags,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let from_date = from.as_deref().and_then(date::parse_date);
            let to_date = to.as_deref().and_then(date::parse_date);

            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let ical = export::ical::export_ical(&docs, from_date, to_date, &tags);

            if let Some(path) = output {
                if let Err(e) = std::fs::write(&path, &ical) {
                    eprintln!("org: error writing {}: {}", path.display(), e);
                    return 2;
                }
                println!("Exported to {}", path.display());
            } else {
                print!("{ical}");
            }

            0
        }
        ExportCommand::Jscal {
            paths,
            output,
            from,
            to,
            tags,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let from_date = from.as_deref().and_then(date::parse_date);
            let to_date = to.as_deref().and_then(date::parse_date);

            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let json = export::jscal::export_jscal(&docs, from_date, to_date, &tags);

            if let Some(path) = output {
                if let Err(e) = std::fs::write(&path, &json) {
                    eprintln!("org: error writing {}: {}", path.display(), e);
                    return 2;
                }
                println!("Exported to {}", path.display());
            } else {
                println!("{json}");
            }

            0
        }
    }
}

/// Read locator strings from stdin (one per line).
fn read_stdin_targets() -> Vec<String> {
    std::io::stdin()
        .lock()
        .lines()
        .map_while(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .collect()
}

/// Runs `org update set-state`.
fn run_set_state(
    state: String,
    mut targets: Vec<String>,
    stdin: bool,
    no_closed: bool,
    dry_run: bool,
) -> i32 {
    if stdin {
        targets.extend(read_stdin_targets());
    }
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return 2;
    }

    // Resolve targets to file+entry pairs.
    let mut file_targets: std::collections::HashMap<PathBuf, Option<Vec<usize>>> =
        std::collections::HashMap::new();

    for target in &targets {
        if let Ok(locator) = OrgLocator::parse(target) {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match resolve_locator(&locator, &[cwd]) {
                Ok(resolved) => {
                    let entry = file_targets
                        .entry(resolved.file.clone())
                        .or_insert(Some(Vec::new()));
                    if let Some(ref mut existing) = entry {
                        existing.push(resolved.entry_index);
                    }
                }
                Err(e) => {
                    eprintln!("org: {e}");
                    return 2;
                }
            }
        } else {
            let path = PathBuf::from(target);
            let files = collect_org_files(&[path]);
            for file in files {
                file_targets.insert(file, None);
            }
        }
    }

    let mut total_changed = 0;
    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));

    for (file, indices) in file_list {
        let source = match SourceFile::from_path(&file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("org: error reading {}: {}", file.display(), e);
                continue;
            }
        };
        let doc = OrgDocument::from_source(&source);

        if !doc.todo_keywords.contains(&state) {
            eprintln!(
                "org: '{}' is not a recognized TODO keyword in {}",
                state,
                file.display()
            );
            continue;
        }

        let result =
            org_tools_core::state::set_state(&source, &doc, indices.as_deref(), &state, !no_closed);

        if let Some(r) = result {
            total_changed += r.changed;
            if dry_run {
                println!(
                    "Would change {} entr{} in {}",
                    r.changed,
                    if r.changed == 1 { "y" } else { "ies" },
                    file.display()
                );
            } else {
                if let Err(e) = std::fs::write(&file, &r.content) {
                    eprintln!("org: error writing {}: {}", file.display(), e);
                    continue;
                }
                println!(
                    "Changed {} entr{} to {} in {}",
                    r.changed,
                    if r.changed == 1 { "y" } else { "ies" },
                    state,
                    file.display()
                );
            }
        }
    }

    if total_changed == 0 && !dry_run {
        println!("No entries to change");
    }
    0
}

/// Runs `org update add-todo`.
#[allow(clippy::too_many_arguments)]
fn run_add_todo(
    title: String,
    target: String,
    level: Option<usize>,
    keyword: String,
    priority: Option<char>,
    tags: Vec<String>,
    scheduled: Option<String>,
    deadline: Option<String>,
    parent: Option<String>,
    dry_run: bool,
) -> i32 {
    // Resolve parent if specified.
    let (file_path, parent_idx, parent_level) = if let Some(ref parent_loc) = parent {
        match OrgLocator::parse(parent_loc) {
            Ok(locator) => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                match resolve_locator(&locator, &[cwd]) {
                    Ok(resolved) => {
                        let source = match SourceFile::from_path(&resolved.file) {
                            Ok(s) => s,
                            Err(e) => {
                                eprintln!("org: {e}");
                                return 2;
                            }
                        };
                        let doc = OrgDocument::from_source(&source);
                        let plevel = doc.entries[resolved.entry_index].level;
                        (resolved.file, Some(resolved.entry_index), Some(plevel))
                    }
                    Err(e) => {
                        eprintln!("org: {e}");
                        return 2;
                    }
                }
            }
            Err(e) => {
                eprintln!("org: {e}");
                return 2;
            }
        }
    } else {
        // Target is a file path.
        let path = PathBuf::from(&target);
        if path.is_dir() {
            eprintln!("org: target must be a file, not a directory");
            return 2;
        }
        (path, None, None)
    };

    let entry_level = level.unwrap_or_else(|| parent_level.map(|l| l + 1).unwrap_or(1));

    let opts = org_tools_core::entry::NewEntryOpts {
        title,
        level: entry_level,
        keyword: Some(keyword),
        priority,
        tags,
        scheduled,
        deadline,
    };

    // Create file if it doesn't exist.
    if !file_path.exists() {
        if dry_run {
            println!("Would create {} with new entry", file_path.display());
            return 0;
        }
        std::fs::write(&file_path, "").ok();
    }

    let source = match SourceFile::from_path(&file_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org: error reading {}: {}", file_path.display(), e);
            return 2;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let result = org_tools_core::entry::add_entry(&source, &doc, parent_idx, &opts);

    if dry_run {
        println!("Would add entry to {}", file_path.display());
    } else {
        if let Err(e) = std::fs::write(&file_path, &result.content) {
            eprintln!("org: error writing {}: {}", file_path.display(), e);
            return 2;
        }
        println!("Added entry to {}", file_path.display());
    }
    0
}

/// Runs `org update add-cookie`.
fn run_add_cookie(paths: Vec<PathBuf>, recursive: bool, dry_run: bool) -> i32 {
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return 2;
    }

    let mut total_updated = 0;
    for file in &files {
        let source = match SourceFile::from_path(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("org: error reading {}: {}", file.display(), e);
                continue;
            }
        };
        let doc = OrgDocument::from_source(&source);

        if let Some(result) = org_tools_core::cookie::update_cookies(&source, &doc, recursive) {
            total_updated += result.updated;
            if dry_run {
                println!(
                    "Would update {} cookie{} in {}",
                    result.updated,
                    if result.updated == 1 { "" } else { "s" },
                    file.display()
                );
            } else {
                if let Err(e) = std::fs::write(file, &result.content) {
                    eprintln!("org: error writing {}: {}", file.display(), e);
                    continue;
                }
                println!(
                    "Updated {} cookie{} in {}",
                    result.updated,
                    if result.updated == 1 { "" } else { "s" },
                    file.display()
                );
            }
        }
    }

    if total_updated == 0 && !dry_run {
        println!("All cookies are up to date");
    }
    0
}

/// Runs `org archive`.
fn run_archive(
    paths: Vec<PathBuf>,
    target_override: Option<String>,
    tags: Vec<String>,
    dry_run: bool,
) -> i32 {
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return 2;
    }

    let mut total_archived = 0;
    for file in &files {
        let source = match SourceFile::from_path(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("org: error reading {}: {}", file.display(), e);
                continue;
            }
        };
        let doc = OrgDocument::from_source(&source);

        let target_str = target_override
            .as_deref()
            .or_else(|| doc.file_keywords.get("ARCHIVE").map(|s| s.as_str()))
            .unwrap_or("");

        let target = org_tools_core::archive::parse_archive_target(target_str, file);
        let archivable = org_tools_core::archive::find_archivable_entries(&source, &doc, &tags);

        if archivable.is_empty() {
            continue;
        }

        if let Some(result) =
            org_tools_core::archive::build_archive(&source, &doc, &archivable, &target)
        {
            total_archived += result.archived;
            if dry_run {
                println!(
                    "Would archive {} entr{} from {} to {}",
                    result.archived,
                    if result.archived == 1 { "y" } else { "ies" },
                    file.display(),
                    result.archive_path.display()
                );
                for entry in &archivable {
                    println!("  {} (line {})", entry.title, entry.start_line);
                }
            } else {
                // Write modified source.
                if let Err(e) = std::fs::write(file, &result.source_content) {
                    eprintln!("org: error writing {}: {}", file.display(), e);
                    continue;
                }
                // Append to archive file.
                let mut archive_content = if result.archive_path.exists() {
                    std::fs::read_to_string(&result.archive_path).unwrap_or_default()
                } else {
                    String::new()
                };
                archive_content.push_str(&result.archive_content);
                if let Err(e) = std::fs::write(&result.archive_path, &archive_content) {
                    eprintln!(
                        "org: error writing {}: {}",
                        result.archive_path.display(),
                        e
                    );
                    continue;
                }
                println!(
                    "Archived {} entr{} from {} to {}",
                    result.archived,
                    if result.archived == 1 { "y" } else { "ies" },
                    file.display(),
                    result.archive_path.display()
                );
            }
        }
    }

    if total_archived == 0 {
        println!("No entries to archive");
    }
    0
}
