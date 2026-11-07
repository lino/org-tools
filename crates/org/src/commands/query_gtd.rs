// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! GTD workflow query views (`blocked`, `next`, `inbox`, `waiting`, `stuck`) for `org query`.

use std::path::PathBuf;

use org_tools_core::document::{OrgDocument, OrgEntry};

use super::load_docs;
use super::query::{collect_predicate_matches, render_standard_matches, QueryOutputFormat};
use crate::{date, query, EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

/// Executes `org query blocked`.
pub(crate) fn run_blocked(
    paths: &[PathBuf],
    format: QueryOutputFormat,
    limit: Option<usize>,
) -> i32 {
    let docs = load_docs(paths);
    if docs.is_empty() {
        return EXIT_ERROR;
    }
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(
        &query::parser::Predicate::Blocked,
        &docs,
        &doc_refs,
        date::current_date(),
        limit,
    );
    if matches.is_empty() {
        eprintln!("org: no blocked entries found");
        return EXIT_ISSUES;
    }
    match format {
        QueryOutputFormat::Human => print!(
            "{}",
            query::output::render_blocked_human(&matches, &doc_refs)
        ),
        QueryOutputFormat::Json => print!(
            "{}",
            query::output::render_blocked_json(&matches, &doc_refs)
        ),
        QueryOutputFormat::Locator => print!("{}", query::output::render_locators(&matches)),
    }
    EXIT_OK
}

/// Checks whether `entry_idx` in `doc` satisfies the optional `@context` tag filter.
fn entry_matches_context(doc: &OrgDocument, entry_idx: usize, context: Option<&str>) -> bool {
    let Some(ctx_tag) = context else {
        return true;
    };
    doc.inherited_tags(entry_idx)
        .iter()
        .any(|t| t.eq_ignore_ascii_case(ctx_tag))
}

/// Collects actionable entries across `docs` matching the optional `@context` tag filter.
fn collect_next_matches<'a>(
    docs: &'a [OrgDocument],
    doc_refs: &[&'a OrgDocument],
    context: Option<&str>,
    limit: Option<usize>,
) -> Vec<query::output::MatchedEntry<'a>> {
    let today = date::current_date();
    let pred = query::parser::Predicate::Actionable;
    let mut matches = Vec::new();
    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if query::predicate::matches(&pred, entry, doc, doc_refs, today)
                && entry_matches_context(doc, idx, context)
            {
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

/// Executes `org query next`.
pub(crate) fn run_next(
    paths: &[PathBuf],
    context: Option<&str>,
    format: QueryOutputFormat,
    limit: Option<usize>,
) -> i32 {
    let docs = load_docs(paths);
    if docs.is_empty() {
        return EXIT_ERROR;
    }
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_next_matches(&docs, &doc_refs, context, limit);
    if matches.is_empty() {
        eprintln!("org: no actionable entries found");
        return EXIT_ISSUES;
    }
    match format {
        QueryOutputFormat::Human => {
            print!("{}", query::output::render_grouped_by_context(&matches))
        }
        QueryOutputFormat::Json => print!("{}", query::output::render_json(&matches, &doc_refs)),
        QueryOutputFormat::Locator => print!("{}", query::output::render_locators(&matches)),
    }
    EXIT_OK
}

/// Returns `true` when `entry` is an untagged, unscheduled active TODO item.
fn is_inbox_entry(doc: &OrgDocument, entry_idx: usize, entry: &OrgEntry) -> bool {
    let Some(ref kw) = entry.keyword else {
        return false;
    };
    if doc.todo_keywords.is_done(kw) {
        return false;
    }
    if !doc.inherited_tags(entry_idx).is_empty() {
        return false;
    }
    entry.planning.scheduled.is_none() && entry.planning.deadline.is_none()
}

/// Executes `org query inbox`.
pub(crate) fn run_inbox(paths: &[PathBuf], format: QueryOutputFormat, limit: Option<usize>) -> i32 {
    let docs = load_docs(paths);
    if docs.is_empty() {
        return EXIT_ERROR;
    }
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let mut matches = Vec::new();
    for doc in &docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if is_inbox_entry(doc, idx, entry) {
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
        eprintln!("org: inbox is empty");
        return EXIT_ISSUES;
    }
    render_standard_matches(&matches, &doc_refs, format);
    EXIT_OK
}

/// Executes `org query waiting`.
pub(crate) fn run_waiting(
    paths: &[PathBuf],
    format: QueryOutputFormat,
    limit: Option<usize>,
) -> i32 {
    let docs = load_docs(paths);
    if docs.is_empty() {
        return EXIT_ERROR;
    }
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(
        &query::parser::Predicate::Waiting,
        &docs,
        &doc_refs,
        date::current_date(),
        limit,
    );
    if matches.is_empty() {
        eprintln!("org: no waiting entries found");
        return EXIT_ISSUES;
    }
    match format {
        QueryOutputFormat::Human => print!("{}", query::output::render_waiting_human(&matches)),
        QueryOutputFormat::Json => print!("{}", query::output::render_json(&matches, &doc_refs)),
        QueryOutputFormat::Locator => print!("{}", query::output::render_locators(&matches)),
    }
    EXIT_OK
}

/// Renders stuck project results in human, JSON, or locator format.
fn render_stuck_output(stuck: &[query::output::StuckProject<'_>], format: QueryOutputFormat) {
    match format {
        QueryOutputFormat::Human => print!("{}", query::output::render_stuck_human(stuck)),
        QueryOutputFormat::Json => print!("{}", query::output::render_stuck_json(stuck)),
        QueryOutputFormat::Locator => {
            for sp in stuck {
                let loc = org_tools_core::locator::locator_for_entry(sp.doc, sp.entry_idx);
                println!("{loc}");
            }
        }
    }
}

/// Executes `org query stuck`.
pub(crate) fn run_stuck(paths: &[PathBuf], format: QueryOutputFormat, limit: Option<usize>) -> i32 {
    let docs = load_docs(paths);
    if docs.is_empty() {
        return EXIT_ERROR;
    }
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let all_stuck = query::output::find_stuck_projects(&docs, &doc_refs, date::current_date());
    let slice_len = limit.map_or(all_stuck.len(), |n| all_stuck.len().min(n));
    let stuck = &all_stuck[..slice_len];
    if stuck.is_empty() {
        eprintln!("org: no stuck projects found");
        return EXIT_ISSUES;
    }
    render_stuck_output(stuck, format);
    EXIT_OK
}
