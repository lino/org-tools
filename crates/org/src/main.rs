// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=3100 reason="Unified CLI entrypoint housing all top-level clap subcommands"

//! Unified CLI for org-mode: lint, format, query, clock, export, archive.

mod clock;
mod date;
mod export;
mod query;

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// Operation completed successfully.
const EXIT_OK: i32 = 0;
/// No results found, or lint/format issues detected.
#[allow(dead_code)]
const EXIT_ISSUES: i32 = 1;
/// Operational error (file not found, parse error, invalid arguments).
const EXIT_ERROR: i32 = 2;
/// Operation requires Emacs Lisp or Emacs Calc (not supported by org-tools).
const EXIT_REQUIRES_EMACS: i32 = 3;

use std::io::{BufRead, IsTerminal, Read as _};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::id::{self, IdGenerator};
use org_tools_core::locator::{resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::output::{render_diagnostics, OutputFormat};
use org_tools_core::property::{
    self, DeterministicIdOptions, FinalizeTodoOptions, NewTodoPropertySpec, StempelamtConfig,
    TodoIdentityMode,
};
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

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: MutationOutputFormat,
    },
    /// Print the JSON schema for a given output type.
    Schema {
        /// Schema name (diagnostic, query, blocked, stuck, deps, clock-report, clock-status, mutation).
        name: String,
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

/// CLI mode for `:ID:` generation on `org update add-todo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TodoIdArg {
    /// Do not automatically mint an `:ID:` property.
    None,
    /// Mint a random UUIDv4 `:ID:` property.
    Auto,
    /// Mint a deterministic Stempelamt UUIDv5 `:ID:` and `:ID_SEED:` pair.
    Deterministic,
}

impl TodoIdArg {
    /// Converts the CLI flag value into the core [`TodoIdentityMode`].
    fn into_core(self) -> TodoIdentityMode {
        match self {
            Self::None => TodoIdentityMode::None,
            Self::Auto => TodoIdentityMode::Auto,
            Self::Deterministic => TodoIdentityMode::Deterministic,
        }
    }
}

/// CLI arguments for `org update add-id`.
#[derive(Args)]
struct AddIdArgs {
    /// Targets: file paths, directory paths, or org locator strings.
    targets: Vec<String>,

    /// Include descendant entries when targeting via locator.
    #[arg(long, short)]
    recursive: bool,

    /// Print what would be changed without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Generate IDs from a template with {placeholders}.
    #[arg(long, conflicts_with_all = ["id_command", "deterministic"])]
    id_format: Option<String>,

    /// Pipe entry metadata (JSON) to a command that outputs the ID.
    #[arg(long, conflicts_with_all = ["id_format", "deterministic"])]
    id_command: Option<String>,

    /// Mint deterministic RFC 9562 UUIDv5 `:ID:` and `:ID_SEED:` properties.
    #[arg(long)]
    deterministic: bool,

    /// Root namespace UUID override for deterministic Stempelamt minting.
    #[arg(long)]
    namespace: Option<String>,

    /// URN prefix override for deterministic Stempelamt seed derivation.
    #[arg(long)]
    urn_prefix: Option<String>,

    /// Explicit Stempelamt seed URN override.
    #[arg(long)]
    seed: Option<String>,

    /// Scope slug for deterministic Stempelamt seed derivation.
    #[arg(long)]
    scope: Option<String>,

    /// Artifact kind for deterministic Stempelamt seed derivation.
    #[arg(long)]
    kind: Option<String>,

    /// Optional author handle for deterministic Stempelamt seed derivation.
    #[arg(long)]
    author: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    format: MutationOutputFormat,
}

/// CLI arguments for `org update set-state`.
#[derive(Args)]
struct SetStateArgs {
    /// New TODO keyword (e.g., DONE, TODO, NEXT).
    state: String,

    /// Targets: file paths, directory paths, or org locator strings.
    targets: Vec<String>,

    /// Read additional locator targets from stdin.
    #[arg(long)]
    stdin: bool,

    /// Filter entries by query expression (e.g., "todo:TODO tags:work").
    #[arg(long)]
    query: Option<String>,

    /// Don't add/remove CLOSED: timestamp on state transitions.
    #[arg(long)]
    no_closed: bool,

    /// Print what would be changed without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    format: MutationOutputFormat,
}

/// CLI arguments for `org update add-todo`.
#[derive(Args)]
struct AddTodoArgs {
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

    /// ID minting strategy for the new entry.
    #[arg(long = "id", value_enum, default_value = "none")]
    id_mode: TodoIdArg,

    /// Root namespace UUID override for deterministic ID minting.
    #[arg(long)]
    namespace: Option<String>,

    /// URN prefix override for deterministic Stempelamt seed derivation.
    #[arg(long)]
    urn_prefix: Option<String>,

    /// Explicit Stempelamt seed URN override.
    #[arg(long)]
    seed: Option<String>,

    /// Scope slug for deterministic Stempelamt seed derivation.
    #[arg(long)]
    scope: Option<String>,

    /// Artifact kind for deterministic Stempelamt seed derivation.
    #[arg(long)]
    kind: Option<String>,

    /// Optional author handle for deterministic Stempelamt seed derivation.
    #[arg(long)]
    author: Option<String>,

    /// Set property KEY=VALUE inside :PROPERTIES: (repeatable).
    #[arg(long = "property", value_name = "KEY=VALUE")]
    properties: Vec<String>,

    /// Body text inserted below the heading and property drawer.
    #[arg(long)]
    body: Option<String>,

    /// Target display column for heading tag alignment (`F007`).
    #[arg(long)]
    tag_column: Option<usize>,

    /// Print what would be changed without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    format: MutationOutputFormat,
}

/// CLI arguments for `org update set-property`.
#[derive(Args)]
struct SetPropertyArgs {
    /// Org locator identifying the target entry.
    locator: String,

    /// Property key name without colons.
    key: String,

    /// Property value to set.
    value: String,

    /// Print what would be changed without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    format: MutationOutputFormat,
}

/// CLI arguments for `org update delete-property`.
#[derive(Args)]
struct DeletePropertyArgs {
    /// Org locator identifying the target entry.
    locator: String,

    /// Property key name without colons.
    key: String,

    /// Print what would be changed without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    format: MutationOutputFormat,
}

/// CLI arguments for `org update set-tags`.
#[derive(Args)]
struct SetTagsArgs {
    /// Org locator identifying the target entry.
    locator: String,

    /// Tags formatted as `:tag1:tag2:`, `tag1,tag2`, or empty `""` to clear.
    tags: Option<String>,

    /// Clear all tags from the heading.
    #[arg(long)]
    clear: bool,

    /// Target display column for heading tag alignment (`F007`).
    #[arg(long)]
    tag_column: Option<usize>,

    /// Print what would be changed without writing files.
    #[arg(long)]
    dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    format: MutationOutputFormat,
}

/// Subcommands for `org update`.
#[derive(Subcommand)]
enum UpdateCommand {
    /// Add :ID: properties to entries that lack them.
    AddId(Box<AddIdArgs>),
    /// Change the TODO state of entries.
    SetState(Box<SetStateArgs>),
    /// Add a new TODO entry.
    AddTodo(Box<AddTodoArgs>),
    /// Set or insert a property in an entry's :PROPERTIES: drawer.
    SetProperty(SetPropertyArgs),
    /// Delete a property from an entry's :PROPERTIES: drawer.
    DeleteProperty(DeletePropertyArgs),
    /// Set or clear heading tags aligned to column 77.
    SetTags(SetTagsArgs),
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

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: MutationOutputFormat,
    },
    /// Evaluate #+TBLFM: table formulas and fill in results.
    Calc {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,

        /// Recalculate even if cells already have values.
        #[arg(long)]
        force: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: MutationOutputFormat,
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

/// Output format for mutation commands (update, archive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum MutationOutputFormat {
    /// Human-readable output.
    Human,
    /// JSON output.
    Json,
}

/// Execution telemetry included in uniform JSON mutation envelopes.
#[derive(serde::Serialize)]
struct MutationTelemetry {
    /// Wall-clock command execution duration in microseconds.
    duration_us: u64,
    /// Number of Org source files inspected during the mutation pass.
    files_scanned: usize,
    /// Number of document cache hits.
    cache_hits: usize,
    /// Number of document cache misses.
    cache_misses: usize,
}

/// Structured result from a mutation command.
#[derive(serde::Serialize)]
struct MutationResult {
    /// Canonical subcommand identifier.
    command: String,
    /// Number of files modified or scheduled for modification.
    files_modified: usize,
    /// Total number of entries or cells affected across all files.
    entries_affected: usize,
    /// Whether no file writes were performed (`--dry-run`).
    dry_run: bool,
    /// Per-entry or per-file mutation details.
    details: Vec<MutationDetail>,
    /// Operational error messages collected during execution.
    errors: Vec<String>,
    /// Execution duration and file-scan telemetry.
    telemetry: MutationTelemetry,
}

impl MutationResult {
    /// Creates an empty [`MutationResult`] envelope for `command`.
    fn new(command: &str, dry_run: bool) -> Self {
        Self {
            command: command.to_string(),
            files_modified: 0,
            entries_affected: 0,
            dry_run,
            details: Vec::new(),
            errors: Vec::new(),
            telemetry: MutationTelemetry {
                duration_us: 0,
                files_scanned: 0,
                cache_hits: 0,
                cache_misses: 0,
            },
        }
    }

    /// Populates wall-clock duration and scanned-file telemetry prior to output.
    fn finish(&mut self, started: Instant, files_scanned: usize) {
        self.telemetry.duration_us = started.elapsed().as_micros() as u64;
        self.telemetry.files_scanned = files_scanned;
    }
}

/// A single action within a mutation result.
#[derive(serde::Serialize)]
struct MutationDetail {
    /// Path to the Org file containing the mutated entry.
    file: String,
    /// One-based line number of the affected heading or table.
    line: usize,
    /// Machine-readable mutation action label.
    action: String,
    /// Optional human-readable detail describing the mutation.
    detail: Option<String>,
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
            format,
        } => run_archive(paths, target, tags, dry_run, format),
        OrgCommand::Schema { name } => run_schema(&name),
    };

    process::exit(exit_code);
}

/// Runs the `fmt` subcommand (check or format).
fn run_fmt(command: FmtCommand, runner: &Runner) -> i32 {
    match command {
        FmtCommand::Check { paths, format, fix } => {
            // Auto-detect piped stdin when no paths given.
            if paths.is_empty() && !std::io::stdin().is_terminal() {
                let source = match read_stdin_source() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{e}");
                        return EXIT_ERROR;
                    }
                };
                if fix {
                    let (formatted, lint_diags) = runner.format(&source);
                    // Can't write back to stdin — emit fixed content to stdout.
                    print!("{formatted}");
                    if !lint_diags.is_empty() {
                        eprint!("{}", render_diagnostics(&lint_diags, format));
                        return 1;
                    }
                    return 0;
                }
                let diagnostics = runner.check(&source);
                if !diagnostics.is_empty() {
                    print!("{}", render_diagnostics(&diagnostics, format));
                    return 1;
                }
                return 0;
            }

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
            // Auto-detect piped stdin when no paths given.
            if paths.is_empty() && !std::io::stdin().is_terminal() {
                let source = match read_stdin_source() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{e}");
                        return EXIT_ERROR;
                    }
                };
                let (formatted, lint_diags) = runner.format(&source);
                let changed = formatted != source.content;
                if !lint_diags.is_empty() {
                    eprint!("{}", render_diagnostics(&lint_diags, OutputFormat::Human));
                }
                if check {
                    if changed {
                        eprintln!("Would reformat: <stdin>");
                    }
                    return if changed { 1 } else { 0 };
                }
                // Stdin mode always writes to stdout.
                print!("{formatted}");
                return 0;
            }

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

/// Sorted `(file_path, optional_entry_indices)` targets for batch mutation commands.
type FileEntryTargets = Vec<(PathBuf, Option<Vec<usize>>)>;

/// Runs the `update` subcommand.
fn run_update(command: UpdateCommand) -> i32 {
    match command {
        UpdateCommand::AddId(args) => run_add_id(*args),
        UpdateCommand::SetState(args) => run_set_state(*args),
        UpdateCommand::AddTodo(args) => run_add_todo(*args),
        UpdateCommand::SetProperty(args) => run_set_property(args),
        UpdateCommand::DeleteProperty(args) => run_delete_property(args),
        UpdateCommand::SetTags(args) => run_set_tags(args),
        UpdateCommand::AddCookie {
            paths,
            recursive,
            dry_run,
            format,
        } => run_add_cookie(paths, recursive, dry_run, format),
        UpdateCommand::Calc {
            paths,
            dry_run,
            force: _,
            format,
        } => run_calc(paths, dry_run, format),
    }
}

/// Output a MutationResult as JSON or human text.
fn output_mutation(result: &MutationResult, format: MutationOutputFormat) {
    match format {
        MutationOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).unwrap_or_default()
            );
        }
        MutationOutputFormat::Human => {
            for detail in &result.details {
                let extra = detail
                    .detail
                    .as_deref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default();
                if result.dry_run {
                    println!(
                        "Would {}: {}:{}{}",
                        detail.action, detail.file, detail.line, extra
                    );
                } else {
                    println!(
                        "{}: {}:{}{}",
                        detail.action, detail.file, detail.line, extra
                    );
                }
            }
            for err in &result.errors {
                eprintln!("org: {err}");
            }
            if result.entries_affected > 0 {
                let verb = if result.dry_run {
                    "Would affect"
                } else {
                    "Affected"
                };
                println!(
                    "{verb} {} entries in {} files",
                    result.entries_affected, result.files_modified
                );
            }
        }
    }
}

/// Resolves a single Org locator string into its parsed entry, source file, and document.
fn resolve_single_target(
    locator_str: &str,
) -> Result<(ResolvedEntry, SourceFile, OrgDocument), i32> {
    let locator = OrgLocator::parse(locator_str).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolved = resolve_locator(&locator, &[cwd]).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let source = SourceFile::from_path(&resolved.file).map_err(|e| {
        eprintln!("org: error reading {}: {e}", resolved.file.display());
        EXIT_ERROR
    })?;
    let doc = OrgDocument::from_source(&source);
    Ok((resolved, source, doc))
}

/// Inserts resolved locator indices for `add-id` into `file_targets`.
fn insert_add_id_locator_target(
    file_targets: &mut std::collections::HashMap<PathBuf, Option<Vec<usize>>>,
    target: &str,
    recursive: bool,
) -> Result<(), i32> {
    let (resolved, _source, doc) = resolve_single_target(target)?;
    if resolved.is_file_node() {
        if recursive {
            file_targets.insert(resolved.file, None);
        } else {
            file_targets
                .entry(resolved.file)
                .or_insert(Some(Vec::new()));
        }
        return Ok(());
    }
    let indices = if recursive {
        id::collect_subtree(&doc, resolved.entry_index)
    } else {
        vec![resolved.entry_index]
    };
    if let Some(ref mut existing) = file_targets
        .entry(resolved.file)
        .or_insert(Some(Vec::new()))
    {
        existing.extend(indices);
    }
    Ok(())
}

/// Resolves `add-id` CLI target arguments into sorted `(file, entry_indices)` pairs.
fn resolve_add_id_targets(targets: &[String], recursive: bool) -> Result<FileEntryTargets, i32> {
    let mut file_targets: std::collections::HashMap<PathBuf, Option<Vec<usize>>> =
        std::collections::HashMap::new();

    for target in targets {
        if OrgLocator::parse(target).is_ok() {
            insert_add_id_locator_target(&mut file_targets, target, recursive)?;
        } else {
            let files = collect_org_files(&[PathBuf::from(target)]);
            if files.is_empty() {
                eprintln!("org: no .org files found in {target}");
                return Err(EXIT_ERROR);
            }
            for file in files {
                file_targets.insert(file, None);
            }
        }
    }

    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(file_list)
}

/// Builds per-entry [`MutationDetail`] records by comparing pre/post `:ID:` properties.
fn collect_added_id_details(
    file: &Path,
    before: &OrgDocument,
    after_content: &str,
) -> Vec<MutationDetail> {
    let after_source = SourceFile::new(file, after_content.to_string());
    let after_doc = OrgDocument::from_source(&after_source);
    let file_str = file.display().to_string();

    after_doc
        .entries
        .iter()
        .enumerate()
        .filter(|(idx, entry)| {
            entry.properties.contains_key("ID")
                && before
                    .entries
                    .get(*idx)
                    .is_some_and(|prev| !prev.properties.contains_key("ID"))
        })
        .map(|(_, entry)| MutationDetail {
            file: file_str.clone(),
            line: entry.heading_line,
            action: "added-id".to_string(),
            detail: entry.properties.get("ID").cloned(),
        })
        .collect()
}

/// Applies either deterministic Stempelamt or classic `:ID:` minting to one document.
fn apply_file_add_ids(
    source: &SourceFile,
    doc: &OrgDocument,
    indices: Option<&[usize]>,
    generator: &IdGenerator,
    det_opts: Option<&DeterministicIdOptions>,
) -> Result<Option<(String, Vec<MutationDetail>)>, String> {
    if let Some(opts) = det_opts {
        let Some(res) = property::add_deterministic_ids(source, doc, indices, opts)? else {
            return Ok(None);
        };
        let file_str = source.path.display().to_string();
        let details = res
            .entries
            .into_iter()
            .map(|entry| MutationDetail {
                file: file_str.clone(),
                line: entry.line,
                action: "added-id".to_string(),
                detail: Some(format!("{} ({})", entry.id, entry.seed)),
            })
            .collect();
        return Ok(Some((res.content, details)));
    }

    let Some(res) = id::add_ids(source, doc, indices, generator)? else {
        return Ok(None);
    };
    let details = collect_added_id_details(&source.path, doc, &res.content);
    Ok(Some((res.content, details)))
}

/// Prints the human-mode per-file summary for `org update add-id`.
fn emit_add_id_file_human(file: &Path, count: usize, dry_run: bool) {
    let suffix = if count == 1 { "" } else { "s" };
    if dry_run {
        println!("Would add {count} ID{suffix} to {}", file.display());
    } else {
        println!("Added {count} ID{suffix} to {}", file.display());
    }
}

/// Processes `add-id` mutations for a single target file and updates `mutation`.
fn process_add_id_file(
    file: &Path,
    indices: Option<&[usize]>,
    args: &AddIdArgs,
    generator: &IdGenerator,
    det_opts: Option<&DeterministicIdOptions>,
    mutation: &mut MutationResult,
) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("error reading {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let applied = match apply_file_add_ids(&source, &doc, indices, generator, det_opts) {
        Ok(Some(pair)) => pair,
        Ok(None) => return,
        Err(e) => {
            let msg = format!("error processing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };

    let (new_content, details) = applied;
    let added_count = details.len();
    if !args.dry_run {
        if let Err(e) = std::fs::write(file, &new_content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += added_count;
    mutation.details.extend(details);
    if args.format == MutationOutputFormat::Human {
        emit_add_id_file_human(file, added_count, args.dry_run);
    }
}

/// Selects the classic [`IdGenerator`] configured on [`AddIdArgs`].
fn build_id_generator(args: &AddIdArgs) -> IdGenerator {
    if let Some(ref tpl) = args.id_format {
        IdGenerator::Template(tpl.clone())
    } else if let Some(ref cmd) = args.id_command {
        IdGenerator::Command(cmd.clone())
    } else {
        IdGenerator::Uuid
    }
}

/// Resolves optional Stempelamt [`DeterministicIdOptions`] from [`AddIdArgs`].
fn build_deterministic_opts(args: &AddIdArgs) -> Result<Option<DeterministicIdOptions>, i32> {
    if !args.deterministic {
        return Ok(None);
    }
    let cfg = StempelamtConfig::resolve(args.namespace.as_deref(), args.urn_prefix.as_deref())
        .map_err(|e| {
            eprintln!("org: {e}");
            EXIT_ERROR
        })?;
    Ok(Some(DeterministicIdOptions {
        config: cfg,
        explicit_seed: args.seed.clone(),
        scope: args.scope.clone(),
        kind: args.kind.clone(),
        author: args.author.clone(),
    }))
}

/// Runs `org update add-id`.
fn run_add_id(args: AddIdArgs) -> i32 {
    let started = Instant::now();
    if args.targets.is_empty() {
        eprintln!("org: no targets specified");
        return EXIT_ERROR;
    }

    let det_opts = match build_deterministic_opts(&args) {
        Ok(opts) => opts,
        Err(code) => return code,
    };
    let generator = build_id_generator(&args);
    let file_list = match resolve_add_id_targets(&args.targets, args.recursive) {
        Ok(list) => list,
        Err(code) => return code,
    };

    let mut mutation = MutationResult::new("add-id", args.dry_run);
    for (file, indices) in &file_list {
        process_add_id_file(
            file,
            indices.as_deref(),
            &args,
            &generator,
            det_opts.as_ref(),
            &mut mutation,
        );
    }
    mutation.finish(started, file_list.len());

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !args.dry_run {
        println!("All entries already have IDs");
    }
    EXIT_OK
}

/// Runs `org update set-property`.
fn run_set_property(args: SetPropertyArgs) -> i32 {
    let started = Instant::now();
    let (resolved, source, doc) = match resolve_single_target(&args.locator) {
        Ok(triple) => triple,
        Err(code) => return code,
    };

    let result =
        match property::set_property(&source, &doc, resolved.entry_index, &args.key, &args.value) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let mut mutation = MutationResult::new("set-property", args.dry_run);
    if result.changed {
        if !args.dry_run {
            if let Err(e) = std::fs::write(&resolved.file, &result.content) {
                eprintln!("org: error writing {}: {e}", resolved.file.display());
                return EXIT_ERROR;
            }
        }
        mutation.files_modified = 1;
        mutation.entries_affected = 1;
        mutation.details.push(MutationDetail {
            file: resolved.file.display().to_string(),
            line: result.line,
            action: "set-property".to_string(),
            detail: Some(format!("{}={}", args.key.trim(), args.value.trim())),
        });
    }
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
}

/// Runs `org update delete-property`.
fn run_delete_property(args: DeletePropertyArgs) -> i32 {
    let started = Instant::now();
    let (resolved, source, doc) = match resolve_single_target(&args.locator) {
        Ok(triple) => triple,
        Err(code) => return code,
    };

    let result = match property::delete_property(&source, &doc, resolved.entry_index, &args.key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let mut mutation = MutationResult::new("delete-property", args.dry_run);
    if result.changed {
        if !args.dry_run {
            if let Err(e) = std::fs::write(&resolved.file, &result.content) {
                eprintln!("org: error writing {}: {e}", resolved.file.display());
                return EXIT_ERROR;
            }
        }
        mutation.files_modified = 1;
        mutation.entries_affected = 1;
        mutation.details.push(MutationDetail {
            file: resolved.file.display().to_string(),
            line: result.line,
            action: "deleted-property".to_string(),
            detail: Some(args.key.trim().to_string()),
        });
    }
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
}

/// Runs `org update set-tags`.
fn run_set_tags(args: SetTagsArgs) -> i32 {
    let started = Instant::now();
    let parsed_tags = match property::parse_tag_input(args.tags.as_deref(), args.clear) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    let (resolved, source, doc) = match resolve_single_target(&args.locator) {
        Ok(triple) => triple,
        Err(code) => return code,
    };

    let tag_col = property::resolve_tag_column(args.tag_column);
    let result =
        match property::set_tags(&source, &doc, resolved.entry_index, &parsed_tags, tag_col) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let mut mutation = MutationResult::new("set-tags", args.dry_run);
    if result.changed {
        if !args.dry_run {
            if let Err(e) = std::fs::write(&resolved.file, &result.content) {
                eprintln!("org: error writing {}: {e}", resolved.file.display());
                return EXIT_ERROR;
            }
        }
        let detail = if parsed_tags.is_empty() {
            Some("cleared".to_string())
        } else {
            Some(format!(":{}:", parsed_tags.join(":")))
        };
        mutation.files_modified = 1;
        mutation.entries_affected = 1;
        mutation.details.push(MutationDetail {
            file: resolved.file.display().to_string(),
            line: result.line,
            action: "set-tags".to_string(),
            detail,
        });
    }
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
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

/// Read org content from stdin into a `SourceFile`.
fn read_stdin_source() -> Result<SourceFile, String> {
    let mut content = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut content)
        .map_err(|e| format!("org: error reading stdin: {e}"))?;
    Ok(SourceFile::new("<stdin>", content))
}

/// Check whether a line looks like org-mode content rather than a locator/path.
///
/// Returns `true` for lines starting with `*`, `#+`, `- `, `: `, `|`, or common
/// org syntax that would never be a valid locator or file path.
fn looks_like_org_content(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("* ")
        || trimmed.starts_with("#+")
        || trimmed.starts_with("- ")
        || trimmed.starts_with("| ")
        || trimmed.starts_with(": ")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("CLOSED:")
        || trimmed.starts_with(":PROPERTIES:")
        || trimmed.starts_with(":END:")
}

/// Read locator strings from stdin (one per line), validating that they don't
/// look like org-mode file content.
fn read_stdin_targets() -> Result<Vec<String>, String> {
    let lines: Vec<String> = std::io::stdin()
        .lock()
        .lines()
        .map_while(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .collect();

    // If any line looks like org content, the user probably piped a file
    // instead of locators.
    if let Some(bad) = lines.iter().find(|l| looks_like_org_content(l)) {
        return Err(format!(
            "org: stdin looks like org-mode content, not locator targets\n\
             first offending line: {bad}\n\
             hint: pipe locators (e.g. from `org query search`), not org file content"
        ));
    }

    Ok(lines)
}

/// Filter entry indices by a query predicate.
fn filter_entries_by_query(doc: &OrgDocument, pred: &query::parser::Predicate) -> Vec<usize> {
    let today = date::current_date();
    let doc_refs = [doc];
    doc.entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query::predicate::matches(pred, entry, doc, &doc_refs, today))
        .map(|(idx, _)| idx)
        .collect()
}

/// Apply set-state to a single source/doc, returning the result.
/// `indices` is `None` to process all entries, or `Some` to restrict.
/// When `query_pred` is given, it further filters the entry set.
fn apply_set_state(
    source: &SourceFile,
    doc: &OrgDocument,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    state: &str,
    no_closed: bool,
) -> Option<org_tools_core::state::SetStateResult> {
    let effective_indices = match query_pred {
        Some(pred) => {
            let query_matches = filter_entries_by_query(doc, pred);
            // Intersect with explicit indices if present.
            let filtered = match indices {
                Some(idx) => query_matches
                    .into_iter()
                    .filter(|i| idx.contains(i))
                    .collect::<Vec<_>>(),
                None => query_matches,
            };
            if filtered.is_empty() {
                return None;
            }
            Some(filtered)
        }
        None => indices.map(|s| s.to_vec()),
    };

    org_tools_core::state::set_state(source, doc, effective_indices.as_deref(), state, !no_closed)
}

/// Processes piped Org-mode file content for `org update set-state`.
fn handle_piped_org_set_state(
    content: String,
    state: &str,
    query_pred: Option<&query::parser::Predicate>,
    no_closed: bool,
    dry_run: bool,
) -> i32 {
    let source = SourceFile::new("<stdin>", content);
    let doc = OrgDocument::from_source(&source);
    if !doc.todo_keywords.contains(state) {
        eprintln!("org: '{state}' is not a recognized TODO keyword in <stdin>");
        return EXIT_ERROR;
    }

    let result = apply_set_state(&source, &doc, None, query_pred, state, no_closed);
    match result {
        Some(r) => {
            let suffix = if r.changed == 1 { "y" } else { "ies" };
            if dry_run {
                eprintln!("Would change {} entr{suffix} in <stdin>", r.changed);
                print!("{}", source.content);
            } else {
                eprintln!("Changed {} entr{suffix} to {state}", r.changed);
                print!("{}", r.content);
            }
            EXIT_OK
        }
        None => {
            eprintln!("No entries to change");
            print!("{}", source.content);
            EXIT_OK
        }
    }
}

/// Reads piped stdin when no CLI targets or `--stdin` flag were supplied.
fn read_piped_state_stdin(
    targets: &mut Vec<String>,
    query_pred: Option<&query::parser::Predicate>,
    args: &SetStateArgs,
) -> Result<Option<i32>, i32> {
    if std::io::stdin().is_terminal() || !targets.is_empty() || args.stdin {
        return Ok(None);
    }

    let mut content = String::new();
    if let Err(e) = std::io::stdin().lock().read_to_string(&mut content) {
        eprintln!("org: error reading stdin: {e}");
        return Err(EXIT_ERROR);
    }
    let first_meaningful = content.lines().find(|l| !l.trim().is_empty());
    if first_meaningful.is_some_and(looks_like_org_content) {
        let code = handle_piped_org_set_state(
            content,
            &args.state,
            query_pred,
            args.no_closed,
            args.dry_run,
        );
        return Ok(Some(code));
    }

    let lines: Vec<String> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(ToString::to_string)
        .collect();
    if let Some(bad) = lines.iter().find(|l| looks_like_org_content(l)) {
        eprintln!(
            "org: stdin looks like org-mode content, not locator targets\n\
             first offending line: {bad}\n\
             hint: pipe locators (e.g. from `org query search`), not org file content"
        );
        return Err(EXIT_ERROR);
    }
    targets.extend(lines);
    Ok(None)
}

/// Resolves `set-state` target arguments into sorted `(file, entry_indices)` pairs.
fn resolve_state_targets(targets: &[String]) -> Result<FileEntryTargets, i32> {
    let mut file_targets: std::collections::HashMap<PathBuf, Option<Vec<usize>>> =
        std::collections::HashMap::new();

    for target in targets {
        if let Ok(locator) = OrgLocator::parse(target) {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let resolved = resolve_locator(&locator, &[cwd]).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            if resolved.is_file_node() {
                eprintln!("org: cannot set TODO state on file-level node {target}");
                return Err(EXIT_ERROR);
            }
            let entry = file_targets
                .entry(resolved.file)
                .or_insert(Some(Vec::new()));
            if let Some(ref mut existing) = entry {
                existing.push(resolved.entry_index);
            }
        } else {
            for file in collect_org_files(&[PathBuf::from(target)]) {
                file_targets.insert(file, None);
            }
        }
    }

    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(file_list)
}

/// Collects per-entry [`MutationDetail`] records for entries whose TODO state changed.
fn collect_changed_state_details(
    file: &Path,
    before: &OrgDocument,
    after_content: &str,
) -> Vec<MutationDetail> {
    let after_source = SourceFile::new(file, after_content.to_string());
    let after_doc = OrgDocument::from_source(&after_source);
    let file_str = file.display().to_string();

    after_doc
        .entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let prev = before.entries.get(idx)?;
            if prev.keyword == entry.keyword {
                return None;
            }
            let from_kw = prev.keyword.as_deref().unwrap_or("<none>");
            let to_kw = entry.keyword.as_deref().unwrap_or("<none>");
            Some(MutationDetail {
                file: file_str.clone(),
                line: entry.heading_line,
                action: "state-changed".to_string(),
                detail: Some(format!("{from_kw} -> {to_kw}")),
            })
        })
        .collect()
}

/// Prints the human-mode per-file summary for `org update set-state`.
fn emit_set_state_human(file: &Path, changed: usize, state: &str, dry_run: bool) {
    let suffix = if changed == 1 { "y" } else { "ies" };
    if dry_run {
        println!("Would change {changed} entr{suffix} in {}", file.display());
    } else {
        println!(
            "Changed {changed} entr{suffix} to {state} in {}",
            file.display()
        );
    }
}

/// Applies `set-state` to a single file and records results into `mutation`.
fn process_set_state_file(
    file: &Path,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    args: &SetStateArgs,
    mutation: &mut MutationResult,
) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("error reading {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };
    let doc = OrgDocument::from_source(&source);
    if !doc.todo_keywords.contains(&args.state) {
        let msg = format!(
            "'{}' is not a recognized TODO keyword in {}",
            args.state,
            file.display()
        );
        eprintln!("org: {msg}");
        mutation.errors.push(msg);
        return;
    }

    let Some(r) = apply_set_state(
        &source,
        &doc,
        indices,
        query_pred,
        &args.state,
        args.no_closed,
    ) else {
        return;
    };
    if !args.dry_run {
        if let Err(e) = std::fs::write(file, &r.content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += r.changed;
    mutation
        .details
        .extend(collect_changed_state_details(file, &doc, &r.content));
    if args.format == MutationOutputFormat::Human {
        emit_set_state_human(file, r.changed, &args.state, args.dry_run);
    }
}

/// Parses an optional `--query` predicate expression.
fn parse_optional_query(expr: Option<&str>) -> Result<Option<query::parser::Predicate>, i32> {
    match expr {
        Some(raw) => query::parser::parse_query(raw).map(Some).map_err(|e| {
            eprintln!("org: invalid query: {e}");
            EXIT_ERROR
        }),
        None => Ok(None),
    }
}

/// Gathers CLI and piped locator targets for `org update set-state`.
fn gather_set_state_targets(
    args: &mut SetStateArgs,
    query_pred: Option<&query::parser::Predicate>,
) -> Result<Option<Vec<String>>, i32> {
    let mut targets = std::mem::take(&mut args.targets);
    if let Some(exit_code) = read_piped_state_stdin(&mut targets, query_pred, args)? {
        if exit_code != EXIT_OK {
            return Err(exit_code);
        }
        return Ok(None);
    }
    if args.stdin {
        let stdin_targets = read_stdin_targets().map_err(|e| {
            eprintln!("{e}");
            EXIT_ERROR
        })?;
        targets.extend(stdin_targets);
    }
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return Err(EXIT_ERROR);
    }
    Ok(Some(targets))
}

/// Runs `org update set-state`.
fn run_set_state(mut args: SetStateArgs) -> i32 {
    let started = Instant::now();
    let query_pred = match parse_optional_query(args.query.as_deref()) {
        Ok(pred) => pred,
        Err(code) => return code,
    };

    let targets = match gather_set_state_targets(&mut args, query_pred.as_ref()) {
        Ok(Some(list)) => list,
        Ok(None) => return EXIT_OK,
        Err(code) => return code,
    };
    let file_list = match resolve_state_targets(&targets) {
        Ok(list) => list,
        Err(code) => return code,
    };

    let mut mutation = MutationResult::new("set-state", args.dry_run);
    for (file, indices) in &file_list {
        process_set_state_file(
            file,
            indices.as_deref(),
            query_pred.as_ref(),
            &args,
            &mut mutation,
        );
    }
    mutation.finish(started, file_list.len());

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !args.dry_run {
        println!("No entries to change");
    }
    EXIT_OK
}

/// Resolves the target file path and optional parent entry for `org update add-todo`.
fn resolve_add_todo_destination(
    target: &str,
    parent: Option<&str>,
) -> Result<(PathBuf, Option<usize>, Option<usize>), i32> {
    if let Some(parent_loc) = parent {
        let (resolved, _source, doc) = resolve_single_target(parent_loc)?;
        if resolved.is_file_node() {
            return Ok((resolved.file, None, None));
        }
        let plevel = doc.entries[resolved.entry_index].level;
        return Ok((resolved.file, Some(resolved.entry_index), Some(plevel)));
    }
    if OrgLocator::parse(target).is_ok() {
        let (resolved, _source, doc) = resolve_single_target(target)?;
        if resolved.is_file_node() {
            return Ok((resolved.file, None, None));
        }
        let plevel = doc.entries[resolved.entry_index].level;
        return Ok((resolved.file, Some(resolved.entry_index), Some(plevel)));
    }

    let path = PathBuf::from(target);
    if path.is_dir() {
        eprintln!("org: target must be a file, not a directory");
        return Err(EXIT_ERROR);
    }
    Ok((path, None, None))
}

/// Loads an existing Org file or initializes an empty in-memory [`SourceFile`].
fn load_or_init_todo_source(file_path: &Path) -> Result<(SourceFile, bool, bool), i32> {
    if file_path.exists() {
        let source = SourceFile::from_path(file_path).map_err(|e| {
            eprintln!("org: error reading {}: {e}", file_path.display());
            EXIT_ERROR
        })?;
        let was_empty = source.content.trim().is_empty();
        Ok((source, true, was_empty))
    } else {
        Ok((SourceFile::new(file_path, String::new()), false, true))
    }
}

/// Emits human-readable or JSON output for `org update add-todo`.
fn emit_add_todo_output(
    mutation: &MutationResult,
    file_path: &Path,
    existed: bool,
    format: MutationOutputFormat,
) {
    if format == MutationOutputFormat::Json {
        output_mutation(mutation, MutationOutputFormat::Json);
    } else if mutation.dry_run && !existed {
        println!("Would create {} with new entry", file_path.display());
    } else if mutation.dry_run {
        println!("Would add entry to {}", file_path.display());
    } else {
        println!("Added entry to {}", file_path.display());
    }
}

/// Inserts and post-processes a new TODO entry via `entry::add_entry` and `property`.
fn insert_and_finalize_todo(
    args: &AddTodoArgs,
    file_path: &Path,
    parent_idx: Option<usize>,
    parent_level: Option<usize>,
    properties: Vec<(String, String)>,
) -> Result<(property::FinalizedTodoResult, bool), i32> {
    let (source, existed, was_empty) = load_or_init_todo_source(file_path)?;
    let doc = OrgDocument::from_source(&source);
    let entry_level = args
        .level
        .unwrap_or_else(|| parent_level.map_or(1, |l| l + 1));
    let opts = org_tools_core::entry::NewEntryOpts {
        title: args.title.clone(),
        level: entry_level,
        keyword: Some(args.keyword.clone()),
        priority: args.priority,
        tags: args.tags.clone(),
        scheduled: args.scheduled.clone(),
        deadline: args.deadline.clone(),
    };

    let added = org_tools_core::entry::add_entry(&source, &doc, parent_idx, &opts);
    let finalized = property::finalize_added_todo(
        was_empty,
        &added.content,
        file_path,
        parent_idx,
        &FinalizeTodoOptions {
            properties,
            body: args.body.clone(),
            tag_column: property::resolve_tag_column(args.tag_column),
        },
    )
    .map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    Ok((finalized, existed))
}

/// Runs `org update add-todo`.
fn run_add_todo(args: AddTodoArgs) -> i32 {
    let started = Instant::now();
    let (file_path, parent_idx, parent_level) =
        match resolve_add_todo_destination(&args.target, args.parent.as_deref()) {
            Ok(dest) => dest,
            Err(code) => return code,
        };

    let properties = match property::build_todo_properties(&NewTodoPropertySpec {
        title: &args.title,
        file_path: &file_path,
        id_mode: args.id_mode.into_core(),
        namespace: args.namespace.as_deref(),
        urn_prefix: args.urn_prefix.as_deref(),
        seed: args.seed.as_deref(),
        scope: args.scope.as_deref(),
        kind: args.kind.as_deref(),
        author: args.author.as_deref(),
        raw_properties: &args.properties,
    }) {
        Ok(props) => props,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let (finalized, existed) =
        match insert_and_finalize_todo(&args, &file_path, parent_idx, parent_level, properties) {
            Ok(pair) => pair,
            Err(code) => return code,
        };

    if !args.dry_run {
        if let Err(e) = std::fs::write(&file_path, &finalized.content) {
            eprintln!("org: error writing {}: {e}", file_path.display());
            return EXIT_ERROR;
        }
    }

    let mut mutation = MutationResult::new("add-todo", args.dry_run);
    mutation.files_modified = 1;
    mutation.entries_affected = 1;
    mutation.details.push(MutationDetail {
        file: file_path.display().to_string(),
        line: finalized.heading_line,
        action: "added-todo".to_string(),
        detail: Some(args.title),
    });
    mutation.finish(started, usize::from(existed));
    emit_add_todo_output(&mutation, &file_path, existed, args.format);
    EXIT_OK
}

/// Collects per-line [`MutationDetail`] records for statistic cookies updated in `file`.
fn collect_cookie_details(file: &Path, before: &str, after: &str) -> Vec<MutationDetail> {
    let file_str = file.display().to_string();
    let details: Vec<MutationDetail> = before
        .lines()
        .zip(after.lines())
        .enumerate()
        .filter(|(_, (old_l, new_l))| old_l != new_l)
        .map(|(idx, (_, new_l))| MutationDetail {
            file: file_str.clone(),
            line: idx + 1,
            action: "updated-cookie".to_string(),
            detail: Some(new_l.trim().to_string()),
        })
        .collect();

    if details.is_empty() {
        vec![MutationDetail {
            file: file_str,
            line: 1,
            action: "updated-cookie".to_string(),
            detail: None,
        }]
    } else {
        details
    }
}

/// Applies statistic cookie updates to a single file and records results in `mutation`.
fn process_add_cookie_file(
    file: &Path,
    recursive: bool,
    dry_run: bool,
    format: MutationOutputFormat,
    mutation: &mut MutationResult,
) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("error reading {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let Some(result) = org_tools_core::cookie::update_cookies(&source, &doc, recursive) else {
        return;
    };

    if !dry_run {
        if let Err(e) = std::fs::write(file, &result.content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += result.updated;
    mutation.details.extend(collect_cookie_details(
        file,
        &source.content,
        &result.content,
    ));

    if format == MutationOutputFormat::Human {
        let suffix = if result.updated == 1 { "" } else { "s" };
        if dry_run {
            println!(
                "Would update {} cookie{suffix} in {}",
                result.updated,
                file.display()
            );
        } else {
            println!(
                "Updated {} cookie{suffix} in {}",
                result.updated,
                file.display()
            );
        }
    }
}

/// Runs `org update add-cookie`.
fn run_add_cookie(
    paths: Vec<PathBuf>,
    recursive: bool,
    dry_run: bool,
    format: MutationOutputFormat,
) -> i32 {
    let started = Instant::now();
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let mut mutation = MutationResult::new("add-cookie", dry_run);
    for file in &files {
        process_add_cookie_file(file, recursive, dry_run, format, &mut mutation);
    }
    mutation.finish(started, files.len());

    if format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !dry_run {
        println!("All cookies are up to date");
    }
    EXIT_OK
}

/// Writes both the pruned source file and appended `.org_archive` file to disk.
fn write_archived_files(
    file: &Path,
    result: &org_tools_core::archive::ArchiveResult,
) -> Result<(), String> {
    std::fs::write(file, &result.source_content)
        .map_err(|e| format!("error writing {}: {e}", file.display()))?;
    let mut archive_content = if result.archive_path.exists() {
        std::fs::read_to_string(&result.archive_path).unwrap_or_default()
    } else {
        String::new()
    };
    archive_content.push_str(&result.archive_content);
    std::fs::write(&result.archive_path, &archive_content)
        .map_err(|e| format!("error writing {}: {e}", result.archive_path.display()))
}

/// Prints the human-mode per-file summary for `org archive`.
fn emit_archive_human(
    file: &Path,
    archive_dest: &str,
    archived: usize,
    archivable: &[org_tools_core::archive::ArchiveEntry],
    dry_run: bool,
) {
    let suffix = if archived == 1 { "y" } else { "ies" };
    if dry_run {
        println!(
            "Would archive {archived} entr{suffix} from {} to {archive_dest}",
            file.display()
        );
        for entry in archivable {
            println!("  {} (line {})", entry.title, entry.start_line);
        }
    } else {
        println!(
            "Archived {archived} entr{suffix} from {} to {archive_dest}",
            file.display()
        );
    }
}

/// Processes archivable entries in a single Org file and records results in `mutation`.
fn process_archive_file(
    file: &Path,
    target_override: Option<&str>,
    tags: &[String],
    dry_run: bool,
    format: MutationOutputFormat,
    mutation: &mut MutationResult,
) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("error reading {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let target_str = target_override
        .or_else(|| doc.file_keywords.get("ARCHIVE").map(String::as_str))
        .unwrap_or("");
    let target = org_tools_core::archive::parse_archive_target(target_str, file);
    let archivable = org_tools_core::archive::find_archivable_entries(&source, &doc, tags);
    let Some(result) = org_tools_core::archive::build_archive(&source, &doc, &archivable, &target)
    else {
        return;
    };

    if !dry_run {
        if let Err(msg) = write_archived_files(file, &result) {
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += result.archived;
    let file_str = file.display().to_string();
    let archive_dest = result.archive_path.display().to_string();
    for entry in &archivable {
        mutation.details.push(MutationDetail {
            file: file_str.clone(),
            line: entry.start_line.max(1),
            action: "archived".to_string(),
            detail: Some(format!("{} -> {archive_dest}", entry.title)),
        });
    }
    if format == MutationOutputFormat::Human {
        emit_archive_human(file, &archive_dest, result.archived, &archivable, dry_run);
    }
}

/// Runs `org archive`.
fn run_archive(
    paths: Vec<PathBuf>,
    target_override: Option<String>,
    tags: Vec<String>,
    dry_run: bool,
    format: MutationOutputFormat,
) -> i32 {
    let started = Instant::now();
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let mut mutation = MutationResult::new("archive", dry_run);
    for file in &files {
        process_archive_file(
            file,
            target_override.as_deref(),
            &tags,
            dry_run,
            format,
            &mut mutation,
        );
    }
    mutation.finish(started, files.len());

    if format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 {
        println!("No entries to archive");
    }
    EXIT_OK
}

/// Evaluates `#+TBLFM:` formulas in a single file and returns whether Emacs was required.
fn process_calc_file(file: &Path, dry_run: bool, mutation: &mut MutationResult) -> bool {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            mutation.errors.push(format!("{}: {e}", file.display()));
            return false;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let result = org_tools_core::tblfm::calc_file(&source.content, &doc.table_constants);

    let mut has_emacs_error = false;
    for err in &result.errors {
        if matches!(err, org_tools_core::tblfm::TblfmError::RequiresEmacs(_)) {
            has_emacs_error = true;
        }
        mutation.errors.push(format!("{}: {err}", file.display()));
    }

    if result.cells_updated > 0 {
        let first_table_line = source
            .content
            .lines()
            .position(|l| l.trim_start().starts_with('|'))
            .map_or(1, |idx| idx + 1);
        mutation.files_modified += 1;
        mutation.entries_affected += result.cells_updated;
        mutation.details.push(MutationDetail {
            file: file.display().to_string(),
            line: first_table_line,
            action: format!("update {} cells", result.cells_updated),
            detail: None,
        });

        if !dry_run {
            if let Err(e) = std::fs::write(file, &result.content) {
                mutation
                    .errors
                    .push(format!("{}: write error: {e}", file.display()));
            }
        }
    }
    has_emacs_error
}

/// Runs `org update calc`.
fn run_calc(paths: Vec<PathBuf>, dry_run: bool, format: MutationOutputFormat) -> i32 {
    let started = Instant::now();
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let mut mutation = MutationResult::new("calc", dry_run);
    let mut has_emacs_error = false;
    for file in &files {
        if process_calc_file(file, dry_run, &mut mutation) {
            has_emacs_error = true;
        }
    }
    mutation.finish(started, files.len());
    output_mutation(&mutation, format);

    if has_emacs_error {
        return EXIT_REQUIRES_EMACS;
    }
    EXIT_OK
}

/// Runs `org schema`.
fn run_schema(name: &str) -> i32 {
    let schema = match name {
        "diagnostic" | "diagnostics" => {
            include_str!("../schemas/org-diagnostic-output.schema.json")
        }
        "query" | "entry" | "entries" => {
            include_str!("../schemas/org-query-output.schema.json")
        }
        "blocked" => include_str!("../schemas/org-blocked-output.schema.json"),
        "stuck" => include_str!("../schemas/org-stuck-output.schema.json"),
        "deps" | "dependencies" => include_str!("../schemas/org-deps-output.schema.json"),
        "clock-report" | "clock_report" => {
            include_str!("../schemas/org-clock-report-output.schema.json")
        }
        "clock-status" | "clock_status" => {
            include_str!("../schemas/org-clock-status-output.schema.json")
        }
        "mutation" | "update" | "archive" => {
            include_str!("../schemas/org-mutation-output.schema.json")
        }
        _ => {
            eprintln!("org: unknown schema: {name}");
            eprintln!(
                "Available: diagnostic, query, blocked, stuck, deps, clock-report, clock-status, mutation"
            );
            return EXIT_ERROR;
        }
    };
    println!("{schema}");
    EXIT_OK
}
