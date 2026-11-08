// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand definitions and search, agenda, and dependency graph handlers for `org query`.

/// Agenda subcommand CLI options, cache integration, and multi-format output dispatch.
#[path = "query_agenda.rs"]
pub(crate) mod query_agenda;

use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};

use org_tools_core::cache::{load_cached_fts_candidate_docs, load_docs_with_bodies, MabuseCache};
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;

use super::parse_org_files;
use super::query_columns::{run_columns, ColumnsOutputFormat};
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
    /// Show agenda view (scheduled/deadline timeline and optional composite GTD sections).
    Agenda(query_agenda::AgendaArgs),
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
    /// Render a tabular #+COLUMNS: / :COLUMNS: property view with parent rollups.
    Columns {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Optional locator scoping the column view to a specific subtree.
        #[arg(long)]
        locator: Option<String>,
        /// Override the #+COLUMNS: format specification string.
        #[arg(long)]
        columns: Option<String>,
        /// Output format (human, json, org, md).
        #[arg(long, value_enum, default_value = "human")]
        format: ColumnsOutputFormat,
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
            if query::predicate::matches_with_body(pred, entry, idx, None, doc, doc_refs, today) {
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

/// Renders `(doc_idx, entry_idx)` body-search pairs and returns `EXIT_OK` or `EXIT_ISSUES`.
fn render_body_pairs(
    loaded: &[org_tools_core::cache::BodyLoadedDoc],
    doc_refs: &[&OrgDocument],
    pairs: Vec<(usize, usize)>,
    format: QueryOutputFormat,
) -> i32 {
    if pairs.is_empty() {
        return EXIT_ISSUES;
    }
    let matches: Vec<_> = pairs
        .into_iter()
        .map(|(di, ei)| query::output::MatchedEntry {
            doc: &loaded[di].doc,
            entry_idx: ei,
        })
        .collect();
    render_standard_matches(&matches, doc_refs, format);
    EXIT_OK
}

/// Attempts an FTS5-accelerated `org query search` when `!no_cache` targets a single directory.
fn try_cached_fts_search(
    pred: &query::parser::Predicate,
    scan_dir: &Path,
    format: QueryOutputFormat,
    limit: Option<usize>,
) -> Option<i32> {
    let req = pred.required_body_phrases();
    if req.is_empty() || req != pred.positive_body_phrases() || pred.uses_edna() {
        return None;
    }
    let (cache, tel) = MabuseCache::open_and_sync(scan_dir).ok()?;
    if tel.files_scanned == 0 {
        eprintln!("org: no .org files found");
        return Some(EXIT_ERROR);
    }
    let (hits, loaded) = load_cached_fts_candidate_docs(&cache, scan_dir, &req).ok()??;
    if hits.is_empty() {
        return Some(EXIT_ISSUES);
    }
    let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
    let today = date::current_date();
    let pairs =
        query::predicate::filter_fts_hits(pred, scan_dir, hits, &loaded, &doc_refs, today, limit);
    Some(render_body_pairs(&loaded, &doc_refs, pairs, format))
}

/// Executes `org query search`.
fn run_search(
    query_str: &str,
    paths: &[PathBuf],
    format: QueryOutputFormat,
    limit: Option<usize>,
    no_cache: bool,
) -> i32 {
    let Ok(pred) = query::parser::parse_query(query_str).map_err(|e| eprintln!("org: {e}")) else {
        return EXIT_ERROR;
    };
    if !no_cache && paths.len() == 1 && paths[0].is_dir() {
        if let Some(code) = try_cached_fts_search(&pred, &paths[0], format, limit) {
            return code;
        }
        let _ = MabuseCache::open_and_sync(&paths[0]);
    }
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    if pred.uses_body() {
        let loaded = load_docs_with_bodies(&files);
        let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
        let pairs = query::predicate::eval_and_rank_body_docs(
            &pred,
            &loaded,
            &doc_refs,
            date::current_date(),
            limit,
        );
        return render_body_pairs(&loaded, &doc_refs, pairs, format);
    }
    let docs = parse_org_files(&files);
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(&pred, &docs, &doc_refs, date::current_date(), limit);
    if matches.is_empty() {
        EXIT_ISSUES
    } else {
        render_standard_matches(&matches, &doc_refs, format);
        EXIT_OK
    }
}

// endregion

// region Dependency Graph Handler

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
pub(crate) fn run_query(command: QueryCommand, no_cache: bool) -> i32 {
    match command {
        QueryCommand::Search {
            query: query_str,
            paths,
            format,
            limit,
        } => run_search(&query_str, &paths, format, limit, no_cache),
        QueryCommand::Agenda(args) => query_agenda::run_agenda(args, no_cache),
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
        QueryCommand::Columns {
            paths,
            locator,
            columns,
            format,
        } => run_columns(&paths, locator.as_deref(), columns.as_deref(), format),
    }
}

// endregion
