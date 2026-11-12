// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! FTS5 search execution (warm WorkspaceCache or ephemeral), result ranking, and search command dispatch.

use std::path::{Path, PathBuf};

use org_tools_core::cache::{execute_ephemeral_fts5_search, WorkspaceCache};
use org_tools_core::document::OrgDocument;
use org_tools_core::scope::{resolve_effective_scope, ScopeQuery, ScopeRegistry};

use super::enrich::enrich_search_item;
use crate::query::output::MatchedEntry;

// region Types

/// A match produced by ranked SQLite FTS5 search.
#[derive(Debug, Clone)]
pub struct FtsSearchResult {
    /// Document index in the scanned documents slice.
    pub doc_idx: usize,
    /// Entry index in `doc.entries` or [`org_tools_core::locator::FILE_NODE_ENTRY_INDEX`].
    pub entry_idx: usize,
    /// BM25 relevance score.
    pub rank: f64,
    /// Contextual snippet with highlight tags.
    pub snippet: String,
}

/// Scope filter options passed to [`run_search_command`].
#[derive(Debug, Clone, Default)]
pub struct SearchScopeOpts<'a> {
    /// Explicit scope expression constraint.
    pub scope: Option<&'a str>,
    /// Target project slug.
    pub for_project: Option<&'a str>,
    /// Target landscape slug.
    pub for_landscape: Option<&'a str>,
    /// Target repository forge path.
    pub forge_path: Option<&'a str>,
    /// Exclude implicit global entries.
    pub strict_scope: bool,
    /// Explicit scope config file path.
    pub scope_config: Option<&'a Path>,
}

// endregion

// region FTS5 Search Execution

/// Attempts warm `WorkspaceCache` FTS5 search, returning `None` on cache miss or error.
fn try_warm_fts5_search(
    query_str: &str,
    docs: &[OrgDocument],
    effective_paths: &[PathBuf],
    limit: Option<usize>,
) -> Option<Vec<FtsSearchResult>> {
    let ws_root = effective_paths
        .first()
        .and_then(|p| {
            if p.is_dir() {
                Some(p.as_path())
            } else {
                p.parent()
            }
        })
        .unwrap_or_else(|| Path::new("."));

    let (cache, _) = WorkspaceCache::open_and_sync(ws_root).ok()?;
    let hits = cache.search_fts_ranked(query_str, limit).ok()?;
    if hits.is_empty() {
        return Some(Vec::new());
    }

    let mut results = Vec::new();
    for hit in hits {
        let doc_idx = docs.iter().position(|d| {
            d.file.ends_with(&hit.rel_path) || hit.rel_path.ends_with(&d.file.display().to_string())
        })?;
        results.push(FtsSearchResult {
            doc_idx,
            entry_idx: hit.entry_idx,
            rank: hit.rank,
            snippet: hit.snippet,
        });
    }
    Some(results)
}

/// Executes FTS5 search via warm `WorkspaceCache` or falls back to in-memory ephemeral table.
pub fn execute_fts5_search(
    query_str: &str,
    docs: &[OrgDocument],
    effective_paths: &[PathBuf],
    limit: Option<usize>,
    no_cache: bool,
) -> Result<Vec<FtsSearchResult>, String> {
    if !no_cache {
        if let Some(results) = try_warm_fts5_search(query_str, docs, effective_paths, limit) {
            return Ok(results);
        }
    }
    let hits = execute_ephemeral_fts5_search(query_str, docs, limit)?;
    Ok(hits
        .into_iter()
        .map(|h| FtsSearchResult {
            doc_idx: h.doc_idx,
            entry_idx: h.entry_idx,
            rank: h.rank,
            snippet: h.snippet,
        })
        .collect())
}

// endregion

// region Sorting

/// Sorts matched entries in place according to `order`.
pub fn sort_matched_entries(
    matches: &mut [MatchedEntry<'_>],
    order: crate::commands::query::QuerySortOrder,
) {
    match order {
        crate::commands::query::QuerySortOrder::Title => {
            matches.sort_by(|a, b| {
                let get_title = |m: &MatchedEntry<'_>| {
                    if m.is_file_node() {
                        m.doc.file_title_or_stem()
                    } else {
                        m.entry().title.clone()
                    }
                };
                get_title(a).cmp(&get_title(b))
            });
        }
        crate::commands::query::QuerySortOrder::Line => {
            matches.sort_by_key(|m| {
                if m.is_file_node() {
                    m.doc.file_properties_line.unwrap_or(1)
                } else {
                    m.entry().heading_line
                }
            });
        }
        crate::commands::query::QuerySortOrder::File => {
            matches.sort_by(|a, b| a.doc.file.cmp(&b.doc.file));
        }
        crate::commands::query::QuerySortOrder::Level => {
            matches.sort_by_key(|m| if m.is_file_node() { 0 } else { m.entry().level });
        }
        crate::commands::query::QuerySortOrder::Rank => {}
    }
}

// endregion

// region Command Execution

/// Rendering and filtering options for search command output.
pub struct SearchRenderOpts<'a> {
    /// Desired output format.
    pub format: crate::commands::query::QueryOutputFormat,
    /// Optional field inclusion list.
    pub include: Option<&'a [String]>,
    /// Optional excluded property names for digest calculation.
    pub exclude_properties: Option<&'a [String]>,
    /// Maximum number of matched entries to emit.
    pub limit: Option<usize>,
    /// Active scope query constraints.
    pub scope_query: ScopeQuery,
    /// Loaded project landscape registry.
    pub scope_reg: ScopeRegistry,
}

/// Executes FTS search branch and outputs formatted results.
fn run_fts_branch(
    fts_query: &str,
    docs: &[OrgDocument],
    doc_refs: &[&OrgDocument],
    paths: &[PathBuf],
    no_cache: bool,
    opts: &SearchRenderOpts<'_>,
) -> i32 {
    let raw_results = match execute_fts5_search(fts_query, docs, paths, None, no_cache) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return crate::EXIT_ERROR;
        }
    };
    let mut results: Vec<FtsSearchResult> = raw_results
        .into_iter()
        .filter(|r| {
            if opts.scope_query.is_empty() {
                return true;
            }
            let doc = &docs[r.doc_idx];
            let tags = doc.effective_tags(r.entry_idx);
            let raw_scope = resolve_effective_scope(doc, r.entry_idx);
            opts.scope_query.matches(&raw_scope, &tags, &opts.scope_reg)
        })
        .collect();
    if let Some(n) = opts.limit {
        results.truncate(n);
    }
    if results.is_empty() {
        return crate::EXIT_ISSUES;
    }
    if opts.format == crate::commands::query::QueryOutputFormat::Json {
        let json_items: Vec<_> = results
            .iter()
            .map(|r| {
                let m = MatchedEntry {
                    doc: &docs[r.doc_idx],
                    entry_idx: r.entry_idx,
                };
                enrich_search_item(
                    &m,
                    doc_refs,
                    opts.include,
                    opts.exclude_properties,
                    Some(r.rank),
                    Some(r.snippet.clone()),
                )
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_items).unwrap_or_default()
        );
        return crate::EXIT_OK;
    }
    let matched: Vec<_> = results
        .iter()
        .map(|r| MatchedEntry {
            doc: &docs[r.doc_idx],
            entry_idx: r.entry_idx,
        })
        .collect();
    crate::commands::query::render_standard_matches(&matched, doc_refs, opts.format);
    crate::EXIT_OK
}

/// Executes predicate search branch and outputs formatted results.
fn run_predicate_branch(
    full_query: &str,
    docs: &[OrgDocument],
    doc_refs: &[&OrgDocument],
    sort: Option<crate::commands::query::QuerySortOrder>,
    opts: &SearchRenderOpts<'_>,
) -> i32 {
    let Ok(pred) = crate::query::parser::parse_query(full_query).map_err(|e| eprintln!("org: {e}"))
    else {
        return crate::EXIT_ERROR;
    };
    let mut matches = crate::commands::query::collect_predicate_matches(
        &pred,
        docs,
        doc_refs,
        crate::date::current_date(),
        None,
    );
    if !opts.scope_query.is_empty() {
        matches.retain(|m| {
            let tags = m.doc.effective_tags(m.entry_idx);
            let raw_scope = resolve_effective_scope(m.doc, m.entry_idx);
            opts.scope_query.matches(&raw_scope, &tags, &opts.scope_reg)
        });
    }
    if matches.is_empty() {
        return crate::EXIT_ISSUES;
    }
    if let Some(s) = sort {
        sort_matched_entries(&mut matches, s);
    }
    if let Some(n) = opts.limit {
        matches.truncate(n);
    }
    if opts.format == crate::commands::query::QueryOutputFormat::Json {
        let json_items: Vec<_> = matches
            .iter()
            .map(|m| {
                enrich_search_item(
                    m,
                    doc_refs,
                    opts.include,
                    opts.exclude_properties,
                    None,
                    None,
                )
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_items).unwrap_or_default()
        );
        return crate::EXIT_OK;
    }
    crate::commands::query::render_standard_matches(&matches, doc_refs, opts.format);
    crate::EXIT_OK
}

/// Appends command line tag filters to the predicate query text.
fn build_full_query(effective_query: &str, tag: Option<&[String]>) -> String {
    let mut full_query = effective_query.to_string();
    if let Some(tags) = tag {
        for t in tags {
            let prefix = if !t.starts_with('+') && !t.starts_with('-') {
                "+"
            } else {
                ""
            };
            full_query.push_str(&format!(" {prefix}{t}"));
        }
    }
    full_query
}

/// Executes `org query search` with optional FTS5 BM25 ranking, tags, sort, and limits.
#[allow(clippy::too_many_arguments)]
pub fn run_search_command(
    query_str: &str,
    paths: &[PathBuf],
    format: crate::commands::query::QueryOutputFormat,
    limit: Option<usize>,
    fts: Option<&str>,
    include: Option<&[String]>,
    exclude_properties: Option<&[String]>,
    sort: Option<crate::commands::query::QuerySortOrder>,
    tag: Option<&[String]>,
    scope_opts: &SearchScopeOpts<'_>,
    no_cache: bool,
) -> i32 {
    let (effective_query, effective_paths) =
        if Path::new(query_str).exists() && (paths == [PathBuf::from(".")] || paths.is_empty()) {
            ("", vec![PathBuf::from(query_str)])
        } else {
            (query_str, paths.to_vec())
        };

    let files = org_tools_core::files::collect_org_files(&effective_paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return crate::EXIT_ERROR;
    }
    let docs = crate::commands::parse_org_files(&files);
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();

    let scan_dir = effective_paths
        .first()
        .and_then(|p| {
            if p.is_dir() {
                Some(p.as_path())
            } else {
                p.parent()
            }
        })
        .unwrap_or_else(|| Path::new("."));
    let scope_reg =
        ScopeRegistry::load_for_dir(scan_dir, scope_opts.scope_config).unwrap_or_default();
    let scope_query = ScopeQuery {
        scope_expr: scope_opts.scope.map(str::to_string),
        for_project: scope_opts.for_project.map(str::to_string),
        for_landscape: scope_opts.for_landscape.map(str::to_string),
        forge_path: scope_opts.forge_path.map(str::to_string),
        strict_scope: scope_opts.strict_scope,
    };

    let opts = SearchRenderOpts {
        format,
        include,
        exclude_properties,
        limit,
        scope_query,
        scope_reg,
    };

    if let Some(fts_query) = fts {
        return run_fts_branch(
            fts_query,
            &docs,
            &doc_refs,
            &effective_paths,
            no_cache,
            &opts,
        );
    }

    let full_query = build_full_query(effective_query, tag);
    run_predicate_branch(&full_query, &docs, &doc_refs, sort, &opts)
}

// endregion
