// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output rendering for query search, GTD views, blocked dependencies, and stuck projects.

use std::collections::HashMap;

use org_tools_core::document::{ChecklistItem, ChecklistSummary, OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::locator::locator_for_entry;
use org_tools_core::rules::timestamp::OrgTimestamp;
use serde::Serialize;

// region Public Types

/// A matched entry paired with its enclosing [`OrgDocument`].
pub struct MatchedEntry<'a> {
    /// The document containing this entry.
    pub doc: &'a OrgDocument,
    /// Index into `doc.entries`.
    pub entry_idx: usize,
}

impl MatchedEntry<'_> {
    /// Returns `true` when this match points to a level-0 file node.
    pub fn is_file_node(&self) -> bool {
        self.entry_idx == org_tools_core::locator::FILE_NODE_ENTRY_INDEX
    }

    /// Returns a reference to the matched [`OrgEntry`].
    pub fn entry(&self) -> &OrgEntry {
        &self.doc.entries[self.entry_idx]
    }
}

/// An outbound link emitted in rich query/show JSON payloads.
#[derive(Debug, Clone, Serialize)]
pub struct QueryLinkJson {
    /// Link kind or scheme (e.g., "id", "file", "https").
    pub kind: String,
    /// Destination target.
    pub target: String,
    /// Optional link description text.
    pub description: Option<String>,
    /// 1-based line number of the link in the source file.
    pub line: usize,
}

/// JSON output entry matching `schemas/org-query-output.schema.json`.
#[derive(Serialize)]
pub struct JsonEntry {
    /// Path to the Org file containing this entry.
    pub file: String,
    /// One-based line number of the heading.
    pub line: usize,
    /// Canonical [`OrgLocator`](org_tools_core::locator::OrgLocator) string for this entry.
    pub locator: String,
    /// Heading outline level (`1` = top-level `*`).
    pub level: usize,
    /// TODO keyword (`TODO`, `NEXT`, `DONE`, etc.) or `None`.
    pub keyword: Option<String>,
    /// Priority character (`A`, `B`, `C`, etc.) or `None`.
    pub priority: Option<String>,
    /// Heading title text without keyword, priority, or tags.
    pub title: String,
    /// Local tags attached directly to this heading.
    pub tags: Vec<String>,
    /// Properties from the `:PROPERTIES:` drawer.
    pub properties: HashMap<String, String>,
    /// Formatted `SCHEDULED:` timestamp or `None`.
    pub scheduled: Option<String>,
    /// Formatted `DEADLINE:` timestamp or `None`.
    pub deadline: Option<String>,
    /// Formatted `CLOSED:` timestamp or `None`.
    pub closed: Option<String>,
    /// Total clocked duration in minutes across this entry's clock lines.
    pub clocked_minutes: i64,
    /// Whether this entry is currently blocked by an `org-edna` condition.
    pub blocked: bool,
    /// Aggregate completion counts across this entry's direct checklist items.
    pub checklist_summary: ChecklistSummary,
    /// Plain-list checklist items in this entry's own body.
    pub checklist: Vec<ChecklistItem>,
    /// Optional file-level tags from #+FILETAGS:.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filetags: Option<Vec<String>>,
    /// Optional effective inherited tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_tags: Option<Vec<String>>,
    /// Optional top-of-body vulpea-meta description list items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vulpea_meta: Option<HashMap<String, Vec<String>>>,
    /// Optional FTS5 BM25 relevance score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<f64>,
    /// Optional highlighted FTS5 snippet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Optional clean body text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Optional outbound links.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<QueryLinkJson>>,
    /// Optional subtree sha256 digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Optional plain-list checklist items alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkboxes: Option<Vec<ChecklistItem>>,
    /// Optional configured `org-roam` vault name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roam_dir: Option<String>,
    /// Optional resolved owning GTD project title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Optional resolved owning GTD project `:ID:`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

// endregion

// region Search & Locator Renderers

/// Renders matched entries in human-readable `file:line: * KW [#P] Title :tags:` format.
pub fn render_human(matches: &[MatchedEntry<'_>]) -> String {
    matches
        .iter()
        .map(|m| format!("{}\n", format_entry_line(m)))
        .collect()
}

/// Renders matched entries as pretty JSON conforming to `org-query-output.schema.json`.
pub fn render_json(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let items: Vec<JsonEntry> = matches
        .iter()
        .map(|m| build_json_entry(m, all_docs))
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

/// Renders matched entries as canonical locator strings (one per line).
pub fn render_locators(matches: &[MatchedEntry<'_>]) -> String {
    matches
        .iter()
        .map(|m| format!("{}\n", locator_for_entry(m.doc, m.entry_idx)))
        .collect()
}

/// Constructs a [`JsonEntry`] from a [`MatchedEntry`] and workspace document context.
pub(crate) fn build_json_entry(m: &MatchedEntry<'_>, all_docs: &[&OrgDocument]) -> JsonEntry {
    if m.is_file_node() {
        let line = m.doc.file_properties_line.unwrap_or(1);
        let locator = locator_for_entry(m.doc, m.entry_idx).to_string();
        let title = m.doc.file_title_or_stem().to_string();
        let tags = m.doc.filetags.clone();
        let properties = m.doc.file_properties.clone();

        return JsonEntry {
            file: m.doc.file.display().to_string(),
            line,
            locator,
            level: 0,
            keyword: None,
            priority: None,
            title,
            tags,
            properties,
            scheduled: None,
            deadline: None,
            closed: None,
            clocked_minutes: 0,
            blocked: false,
            checklist_summary: ChecklistSummary::default(),
            checklist: Vec::new(),
            filetags: None,
            effective_tags: None,
            vulpea_meta: None,
            rank: None,
            snippet: None,
            body: None,
            links: None,
            digest: None,
            checkboxes: None,
            roam_dir: None,
            project: None,
            project_id: None,
        };
    }

    let entry = m.entry();
    let total_clocked: i64 = entry.clocks.iter().filter_map(|c| c.duration_minutes).sum();
    let ctx = EdnaContext {
        all_docs,
        doc: m.doc,
        entry_idx: m.entry_idx,
    };

    JsonEntry {
        file: m.doc.file.display().to_string(),
        line: entry.heading_line,
        locator: locator_for_entry(m.doc, m.entry_idx).to_string(),
        level: entry.level,
        keyword: entry.keyword.clone(),
        priority: entry.priority.map(|p| p.to_string()),
        title: entry.title.clone(),
        tags: entry.tags.clone(),
        properties: entry.properties.clone(),
        scheduled: entry.planning.scheduled.as_ref().map(format_ts),
        deadline: entry.planning.deadline.as_ref().map(format_ts),
        closed: entry.planning.closed.as_ref().map(format_ts),
        clocked_minutes: total_clocked,
        blocked: edna::is_blocked(&ctx),
        checklist_summary: entry.checklist_summary(),
        checklist: entry.checklist.clone(),
        filetags: None,
        effective_tags: None,
        vulpea_meta: None,
        rank: None,
        snippet: None,
        body: None,
        links: None,
        digest: None,
        checkboxes: None,
        roam_dir: None,
        project: None,
        project_id: None,
    }
}

/// Formats an [`OrgTimestamp`] as `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`.
pub(crate) fn format_ts(ts: &OrgTimestamp) -> String {
    match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => format!("{:04}-{:02}-{:02} {h:02}:{m:02}", ts.year, ts.month, ts.day),
        _ => format!("{:04}-{:02}-{:02}", ts.year, ts.month, ts.day),
    }
}

/// Formats a single matched entry line for human-readable query views.
pub(crate) fn format_entry_line(m: &MatchedEntry<'_>) -> String {
    if m.is_file_node() {
        let tags = if m.doc.filetags.is_empty() {
            String::new()
        } else {
            format!(" :{}:", m.doc.filetags.join(":"))
        };
        return format!(
            "{}:{}: {}{tags}",
            m.doc.file.display(),
            m.doc.file_properties_line.unwrap_or(1),
            m.doc.file_title_or_stem()
        );
    }
    let e = m.entry();
    let tags = if e.tags.is_empty() {
        String::new()
    } else {
        format!(" :{}:", e.tags.join(":"))
    };
    let kw = e
        .keyword
        .as_deref()
        .map_or(String::new(), |k| format!("{k} "));
    let pri = e.priority.map_or(String::new(), |p| format!("[#{p}] "));
    format!(
        "{}:{}: {} {kw}{pri}{}{tags}",
        m.doc.file.display(),
        e.heading_line,
        "*".repeat(e.level),
        e.title
    )
}

// endregion

#[path = "output/gtd.rs"]
pub mod gtd;
pub use gtd::*;
