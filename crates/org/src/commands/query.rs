// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org query` subcommand definitions and search, agenda, and dependency graph handlers.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;

use super::parse_org_files;
use super::query_gtd::{run_blocked, run_inbox, run_next, run_stuck, run_waiting};
use crate::{date, query, EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

// region CLI Types

/// Output format for query results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum QueryOutputFormat {
    /// Human-readable output.
    Human,
    /// JSON output (validated against schema).
    Json,
    /// One OrgLocator string per line.
    Locator,
}

/// Output format for dependency graphs.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum DepsOutputFormat {
    /// Mermaid diagram syntax.
    Mermaid,
    /// PlantUML syntax.
    Plantuml,
    /// Graphviz DOT format.
    Dot,
    /// JSON array of edges.
    Json,
}

/// Subcommands for `org query`.
#[derive(Subcommand)]
pub(crate) enum QueryCommand {
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

// endregion

// region Search & Shared Match Helpers

/// Collects all entries matching `pred` across `docs`, applying an optional `limit`.
pub(crate) fn collect_predicate_matches<'a>(
    pred: &query::parser::Predicate,
    docs: &'a [OrgDocument],
    doc_refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
    limit: Option<usize>,
) -> Vec<query::output::MatchedEntry<'a>> {
    let mut matches = Vec::new();
    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if query::predicate::matches(pred, entry, doc, doc_refs, today) {
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
    matches
}

/// Prints standard `human`, `json`, or `locator` output for matched entries.
pub(crate) fn render_standard_matches(
    matches: &[query::output::MatchedEntry<'_>],
    doc_refs: &[&OrgDocument],
    format: QueryOutputFormat,
) {
    match format {
        QueryOutputFormat::Human => print!("{}", query::output::render_human(matches)),
        QueryOutputFormat::Json => print!("{}", query::output::render_json(matches, doc_refs)),
        QueryOutputFormat::Locator => print!("{}", query::output::render_locators(matches)),
    }
}

/// Executes `org query search`.
fn run_search(
    query_str: &str,
    paths: &[PathBuf],
    format: QueryOutputFormat,
    limit: Option<usize>,
) -> i32 {
    let pred = match query::parser::parse_query(query_str) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let docs = parse_org_files(&files);
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(&pred, &docs, &doc_refs, date::current_date(), limit);
    if matches.is_empty() {
        return EXIT_ISSUES;
    }
    render_standard_matches(&matches, &doc_refs, format);
    EXIT_OK
}

// endregion

// region Agenda & Dependency Handlers

/// Serializes one [`query::agenda::AgendaItem`] into its JSON object representation.
fn agenda_item_to_json(item: &query::agenda::AgendaItem<'_>) -> serde_json::Value {
    let time = match (item.timestamp.hour, item.timestamp.minute) {
        (Some(h), Some(m)) => Some(format!("{h:02}:{m:02}")),
        _ => None,
    };
    serde_json::json!({
        "file": item.file.display().to_string(),
        "line": item.entry.heading_line,
        "keyword": item.entry.keyword,
        "priority": item.entry.priority.map(|p| p.to_string()),
        "title": item.entry.title,
        "tags": item.entry.tags,
        "kind": format!("{:?}", item.kind),
        "time": time,
    })
}

/// Serializes one non-empty [`query::agenda::AgendaDay`] into its JSON object representation.
fn agenda_day_to_json(day: &query::agenda::AgendaDay<'_>) -> serde_json::Value {
    let items: Vec<serde_json::Value> = day.items.iter().map(agenda_item_to_json).collect();
    serde_json::json!({
        "date": format!("{:04}-{:02}-{:02}", day.year, day.month, day.day),
        "items": items,
    })
}

/// Renders agenda output in human, JSON, or locator format.
fn render_agenda_output(agenda_days: &[query::agenda::AgendaDay<'_>], format: QueryOutputFormat) {
    match format {
        QueryOutputFormat::Human => print!("{}", query::agenda::render_agenda_human(agenda_days)),
        QueryOutputFormat::Json => {
            let json_days: Vec<serde_json::Value> = agenda_days
                .iter()
                .filter(|d| !d.items.is_empty())
                .map(agenda_day_to_json)
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json_days).unwrap_or_default()
            );
        }
        QueryOutputFormat::Locator => {
            for d in agenda_days {
                for item in &d.items {
                    println!("{}::{}", item.file.display(), item.entry.heading_line);
                }
            }
        }
    }
}

/// Executes `org query agenda`.
fn run_agenda(paths: &[PathBuf], days: usize, format: QueryOutputFormat) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let docs = parse_org_files(&files);
    let agenda_days = query::agenda::build_agenda(&docs, date::current_date(), days);
    let has_items = agenda_days.iter().any(|d| !d.items.is_empty());
    render_agenda_output(&agenda_days, format);
    if has_items {
        EXIT_OK
    } else {
        EXIT_ISSUES
    }
}

/// Executes `org query deps`.
fn run_deps(paths: &[PathBuf], format: DepsOutputFormat) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let docs = parse_org_files(&files);
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let edges = query::deps::extract_edges(&doc_refs);
    if edges.is_empty() {
        eprintln!("org: no edna dependencies found");
        return EXIT_ISSUES;
    }
    match format {
        DepsOutputFormat::Mermaid => print!("{}", query::deps::render_mermaid(&edges, &doc_refs)),
        DepsOutputFormat::Plantuml => print!("{}", query::deps::render_plantuml(&edges)),
        DepsOutputFormat::Dot => print!("{}", query::deps::render_dot(&edges)),
        DepsOutputFormat::Json => println!("{}", query::deps::render_json(&edges)),
    }
    EXIT_OK
}

// endregion

// region Entrypoint

/// Runs the `query` subcommand.
pub(crate) fn run_query(command: QueryCommand) -> i32 {
    match command {
        QueryCommand::Search {
            query: query_str,
            paths,
            format,
            limit,
        } => run_search(&query_str, &paths, format, limit),
        QueryCommand::Agenda {
            paths,
            days,
            format,
        } => run_agenda(&paths, days, format),
        QueryCommand::Blocked {
            paths,
            format,
            limit,
        } => run_blocked(&paths, format, limit),
        QueryCommand::Next {
            paths,
            context,
            format,
            limit,
        } => run_next(&paths, context.as_deref(), format, limit),
        QueryCommand::Inbox {
            paths,
            format,
            limit,
        } => run_inbox(&paths, format, limit),
        QueryCommand::Waiting {
            paths,
            format,
            limit,
        } => run_waiting(&paths, format, limit),
        QueryCommand::Stuck {
            paths,
            format,
            limit,
        } => run_stuck(&paths, format, limit),
        QueryCommand::Deps {
            paths,
            format,
            todo_only: _,
        } => run_deps(&paths, format),
    }
}

// endregion
