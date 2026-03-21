// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Unified CLI for org-mode: lint, format, query, clock, export.

mod query;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::output::{render_diagnostics, OutputFormat};
use org_tools_core::runner::Runner;
use org_tools_core::source::SourceFile;

/// Unified CLI for org-mode files.
#[derive(Parser)]
#[command(name = "org", about = "Unified CLI for org-mode: lint, format, query, clock, export")]
struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    command: OrgCommand,
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
    // TODO: Clock, Update, Export subcommands (phases 5-7)
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

    let exit_code = match cli.command {
        OrgCommand::Fmt { command, config } => {
            let config = load_config(&config);
            let runner = Runner::new(config);
            run_fmt(command, &runner)
        }
        OrgCommand::Query { command } => run_query(command),
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
                if has_issues { 1 } else { 0 }
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
                            print!(
                                "{}",
                                render_diagnostics(&lint_diags, OutputFormat::Human)
                            );
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

            let today = current_date();
            let mut docs: Vec<OrgDocument> = Vec::new();
            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => docs.push(OrgDocument::from_source(&source)),
                    Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
                }
            }

            let mut matches: Vec<query::output::MatchedEntry<'_>> = Vec::new();
            for doc in &docs {
                for (idx, entry) in doc.entries.iter().enumerate() {
                    if query::predicate::matches(&pred, entry, doc, today) {
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
                QueryOutputFormat::Json => print!("{}", query::output::render_json(&matches)),
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

            let today = current_date();
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
                            println!(
                                "{}::{}",
                                item.file.display(),
                                item.entry.heading_line
                            );
                        }
                    }
                }
            }

            if has_items { 0 } else { 1 }
        }
    }
}

/// Get today's date as (year, month, day).
fn current_date() -> (u16, u8, u8) {
    // Use a simple approach without chrono dependency for now.
    // Parse from system time.
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs() as i64;

    // Days since epoch (1970-01-01).
    let days = secs / 86400;

    // Civil date from day count (algorithm from Howard Hinnant).
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u16, m as u8, d as u8)
}
