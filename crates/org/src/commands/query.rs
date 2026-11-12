// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand definitions and search, agenda, and dependency graph handlers for `org query`.

/// Agenda subcommand CLI options, cache integration, and multi-format output dispatch.
#[path = "query_agenda.rs"]
pub(crate) mod query_agenda;

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum, ValueHint};

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;

use super::parse_org_files;
use super::query_columns::{run_columns, ColumnsOutputFormat};
use super::query_gtd::{
    run_blocked, run_inbox, run_next, run_stuck, run_waiting, GtdViewArgs, NextArgs,
};
use crate::{query, EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

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

/// Field to sort query search results by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum QuerySortOrder {
    /// Sort by FTS5 BM25 relevance score best-first.
    Rank,
    /// Sort alphabetically by title.
    Title,
    /// Sort by line number ascending.
    Line,
    /// Sort alphabetically by file path.
    File,
    /// Sort by outline level ascending.
    Level,
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
        #[arg(default_value = "")]
        query: String,
        /// Files or directories to search.
        #[arg(default_value = ".", value_hint = ValueHint::AnyPath)]
        paths: Vec<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: QueryOutputFormat,
        /// Maximum results to return.
        #[arg(long)]
        limit: Option<usize>,
        /// Ranked full-text search query across title, aliases, tags, properties, meta, and body.
        #[arg(long)]
        fts: Option<String>,
        /// Optional fields to include in JSON output (body, vulpea_meta, checkboxes, links, digest).
        #[arg(long, value_delimiter = ',')]
        include: Option<Vec<String>>,
        /// Exclude property keys from canonical digest calculation.
        #[arg(long, value_delimiter = ',')]
        exclude_properties: Option<Vec<String>>,
        /// Field to sort results by.
        #[arg(long, value_enum)]
        sort: Option<QuerySortOrder>,
        /// Tag filters (e.g. +tagA or -tagB).
        #[arg(long)]
        tag: Option<Vec<String>>,
    },
    /// Show agenda view (scheduled/deadline timeline and optional composite GTD sections).
    Agenda(query_agenda::AgendaArgs),
    /// Show blocked entries with dependency details.
    Blocked(GtdViewArgs),
    /// GTD next actions: actionable tasks ranked by urgency and grouped by dimension.
    Next(Box<NextArgs>),
    /// GTD inbox: entries in `inbox.org` or with TODO keywords and no tags/planning dates.
    Inbox(GtdViewArgs),
    /// GTD waiting-for: entries in a waiting state.
    Waiting(GtdViewArgs),
    /// GTD stuck projects: projects with no actionable children.
    Stuck(GtdViewArgs),
    /// Show org-edna dependency graph.
    Deps {
        /// Files or directories to scan.
        #[arg(default_value = ".", value_hint = ValueHint::AnyPath)]
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
        #[arg(default_value = ".", value_hint = ValueHint::AnyPath)]
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
        if (!doc.file_properties.is_empty() || doc.file_id().is_some())
            && query::predicate::matches_file_node(pred, doc, doc_refs, today)
        {
            matches.push(query::output::MatchedEntry {
                doc,
                entry_idx: org_tools_core::locator::FILE_NODE_ENTRY_INDEX,
            });
        }
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

/// Executes `org query search` across FTS5, cached phrases, or general predicate evaluation.
#[allow(clippy::too_many_arguments)]
fn run_search(
    query_str: &str,
    paths: &[PathBuf],
    format: QueryOutputFormat,
    limit: Option<usize>,
    fts: Option<&str>,
    include: Option<&[String]>,
    exclude_properties: Option<&[String]>,
    sort: Option<QuerySortOrder>,
    tag: Option<&[String]>,
    no_cache: bool,
) -> i32 {
    query::rich::run_search_command(
        query_str,
        paths,
        format,
        limit,
        fts,
        include,
        exclude_properties,
        sort,
        tag,
        no_cache,
    )
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
            fts,
            include,
            exclude_properties,
            sort,
            tag,
        } => run_search(
            &query_str,
            &paths,
            format,
            limit,
            fts.as_deref(),
            include.as_deref(),
            exclude_properties.as_deref(),
            sort,
            tag.as_deref(),
            no_cache,
        ),
        QueryCommand::Agenda(args) => query_agenda::run_agenda(args, no_cache),
        QueryCommand::Blocked(args) => run_blocked(args),
        QueryCommand::Next(args) => run_next(*args),
        QueryCommand::Inbox(args) => run_inbox(args),
        QueryCommand::Waiting(args) => run_waiting(args),
        QueryCommand::Stuck(args) => run_stuck(args),
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
