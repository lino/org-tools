// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output rendering for query search, GTD views, blocked dependencies, and stuck projects.

use std::collections::{BTreeMap, HashMap};

use org_tools_core::document::{ChecklistItem, ChecklistSummary, OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::locator::locator_for_entry;
use org_tools_core::rules::timestamp::OrgTimestamp;
use serde::Serialize;

use super::{parser::Predicate, predicate};

// region Public Types

/// A matched entry paired with its enclosing [`OrgDocument`].
pub struct MatchedEntry<'a> {
    /// The document containing this entry.
    pub doc: &'a OrgDocument,
    /// Index into `doc.entries`.
    pub entry_idx: usize,
}

impl MatchedEntry<'_> {
    /// Returns a reference to the matched [`OrgEntry`].
    pub fn entry(&self) -> &OrgEntry {
        &self.doc.entries[self.entry_idx]
    }
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
}

/// A project entry paired with child completion and actionability counts.
pub struct StuckProject<'a> {
    /// The document containing this project.
    pub doc: &'a OrgDocument,
    /// Index of the project entry in `doc.entries`.
    pub entry_idx: usize,
    /// Number of done child entries.
    pub done: usize,
    /// Number of actionable child entries.
    pub actionable: usize,
    /// Number of blocked child entries.
    pub blocked_count: usize,
    /// Number of waiting child entries.
    pub waiting: usize,
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
fn build_json_entry(m: &MatchedEntry<'_>, all_docs: &[&OrgDocument]) -> JsonEntry {
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
    }
}

/// Formats an [`OrgTimestamp`] as `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`.
fn format_ts(ts: &OrgTimestamp) -> String {
    match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => format!("{:04}-{:02}-{:02} {h:02}:{m:02}", ts.year, ts.month, ts.day),
        _ => format!("{:04}-{:02}-{:02}", ts.year, ts.month, ts.day),
    }
}

/// Formats a single matched entry line for human-readable query views.
fn format_entry_line(m: &MatchedEntry<'_>) -> String {
    let entry = m.entry();
    let tags = if entry.tags.is_empty() {
        String::new()
    } else {
        format!(" :{}:", entry.tags.join(":"))
    };
    let kw = entry
        .keyword
        .as_deref()
        .map(|k| format!("{k} "))
        .unwrap_or_default();
    let pri = entry
        .priority
        .map(|p| format!("[#{p}] "))
        .unwrap_or_default();
    format!(
        "{}:{}: {} {kw}{pri}{}{tags}",
        m.doc.file.display(),
        entry.heading_line,
        "*".repeat(entry.level),
        entry.title,
    )
}

// endregion

// region Blocked, Context & Waiting Renderers

/// Renders blocked entries with dependency details in human-readable format.
pub fn render_blocked_human(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let mut out = String::new();
    for m in matches {
        out.push_str(&format!("{}\n", format_entry_line(m)));
        let details = edna::blocking_details(&EdnaContext {
            all_docs,
            doc: m.doc,
            entry_idx: m.entry_idx,
        });
        if !details.is_empty() {
            out.push_str("  Blocked by:\n");
            for d in &details {
                let kw = d
                    .keyword
                    .as_deref()
                    .map(|k| format!("{k} — "))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    {}:{}: {} ({kw}{})\n",
                    d.file, d.line, d.title, d.condition_desc
                ));
            }
        }
        out.push('\n');
    }
    out
}

/// Renders blocked entries as pretty JSON conforming to `org-blocked-output.schema.json`.
pub fn render_blocked_json(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let items: Vec<serde_json::Map<String, serde_json::Value>> = matches
        .iter()
        .map(|m| {
            let mut obj = match serde_json::to_value(build_json_entry(m, all_docs)) {
                Ok(serde_json::Value::Object(map)) => map,
                _ => serde_json::Map::new(),
            };
            obj.remove("checklist_summary");
            obj.remove("checklist");
            let blockers: Vec<serde_json::Value> = edna::blocking_details(&EdnaContext {
                all_docs,
                doc: m.doc,
                entry_idx: m.entry_idx,
            })
            .into_iter()
            .map(|d| {
                serde_json::json!({
                    "title": d.title,
                    "keyword": d.keyword,
                    "file": d.file,
                    "line": d.line,
                    "locator": d.locator,
                    "condition": d.condition_desc,
                })
            })
            .collect();
            obj.insert("blocked".into(), true.into());
            obj.insert("blocking_entries".into(), blockers.into());
            obj
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

/// Renders matched entries grouped by `@`-prefixed GTD context tags.
pub fn render_grouped_by_context(matches: &[MatchedEntry<'_>]) -> String {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut no_context: Vec<String> = Vec::new();

    for m in matches {
        let line = format_entry_line(m);
        let mut tagged = false;
        for tag in m.entry().tags.iter().filter(|t| t.starts_with('@')) {
            tagged = true;
            groups.entry(tag.clone()).or_default().push(line.clone());
        }
        if !tagged {
            no_context.push(line);
        }
    }

    let mut out = String::new();
    for (ctx, entries) in &groups {
        append_context_group(&mut out, ctx, entries);
    }
    if !no_context.is_empty() {
        append_context_group(&mut out, "No context", &no_context);
    }
    out
}

/// Appends a single named GTD context section to `out`.
fn append_context_group(out: &mut String, label: &str, entries: &[String]) {
    out.push_str(&format!("{label} ({})\n", entries.len()));
    for entry in entries {
        out.push_str(&format!("  {entry}\n"));
    }
    out.push('\n');
}

/// Renders waiting entries with optional `:WAITING_FOR:` details.
pub fn render_waiting_human(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
        out.push_str(&format!("{}\n", format_entry_line(m)));
        if let Some(waiting_for) = m.entry().properties.get("WAITING_FOR") {
            out.push_str(&format!("  Waiting for: {waiting_for}\n"));
        }
    }
    out
}

// endregion

// region Stuck Project Analysis & Renderers

/// Finds projects (headings with TODO children) that have zero actionable children.
pub fn find_stuck_projects<'a>(
    docs: &'a [OrgDocument],
    doc_refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
) -> Vec<StuckProject<'a>> {
    let mut stuck = Vec::new();
    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if let Some(project) = evaluate_stuck_candidate(doc, idx, entry, doc_refs, today) {
                stuck.push(project);
            }
        }
    }
    stuck
}

/// Evaluates whether `entry` is a project with zero actionable direct TODO children.
fn evaluate_stuck_candidate<'a>(
    doc: &'a OrgDocument,
    idx: usize,
    entry: &OrgEntry,
    doc_refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
) -> Option<StuckProject<'a>> {
    if !entry
        .children
        .iter()
        .any(|&i| doc.entries[i].keyword.is_some())
    {
        return None;
    }

    let (mut done, mut actionable, mut blocked_count, mut waiting) = (0, 0, 0, 0);
    for &ci in &entry.children {
        classify_project_child(
            doc,
            ci,
            doc_refs,
            today,
            (&mut done, &mut actionable, &mut blocked_count, &mut waiting),
        );
    }

    (actionable == 0).then_some(StuckProject {
        doc,
        entry_idx: idx,
        done,
        actionable,
        blocked_count,
        waiting,
    })
}

/// Classifies a single project child into done, waiting, blocked, or actionable buckets.
fn classify_project_child(
    doc: &OrgDocument,
    child_idx: usize,
    doc_refs: &[&OrgDocument],
    today: (u16, u8, u8),
    counts: (&mut usize, &mut usize, &mut usize, &mut usize),
) {
    let child = &doc.entries[child_idx];
    let Some(kw) = child.keyword.as_deref() else {
        return;
    };
    let is_actionable = predicate::matches(&Predicate::Actionable, child, doc, doc_refs, today);
    if doc.todo_keywords.is_done(kw) {
        *counts.0 += 1;
    } else if kw.to_uppercase().contains("WAIT") || child.properties.contains_key("WAITING_FOR") {
        *counts.3 += 1;
    } else if edna::is_blocked(&EdnaContext {
        all_docs: doc_refs,
        doc,
        entry_idx: child_idx,
    }) {
        *counts.2 += 1;
    } else if is_actionable {
        *counts.1 += 1;
    }
}

/// Renders stuck projects in human-readable summary format.
pub fn render_stuck_human(stuck: &[StuckProject<'_>]) -> String {
    let mut out = String::new();
    for sp in stuck {
        let entry = &sp.doc.entries[sp.entry_idx];
        let tags = if entry.tags.is_empty() {
            String::new()
        } else {
            format!(" :{}:", entry.tags.join(":"))
        };
        let kw = entry
            .keyword
            .as_deref()
            .map(|k| format!("{k} "))
            .unwrap_or_default();
        out.push_str(&format!(
            "{}:{}: {} {kw}{}{tags}\n  Children: {}\n\n",
            sp.doc.file.display(),
            entry.heading_line,
            "*".repeat(entry.level),
            entry.title,
            format_stuck_counts(sp),
        ));
    }
    out
}

/// Formats comma-separated child status counts for a [`StuckProject`].
fn format_stuck_counts(sp: &StuckProject<'_>) -> String {
    let mut parts = Vec::new();
    if sp.done > 0 {
        parts.push(format!("{} done", sp.done));
    }
    parts.push(format!("{} actionable", sp.actionable));
    if sp.blocked_count > 0 {
        parts.push(format!("{} blocked", sp.blocked_count));
    }
    if sp.waiting > 0 {
        parts.push(format!("{} waiting", sp.waiting));
    }
    parts.join(", ")
}

/// Renders stuck projects as pretty JSON matching `org-stuck-output.schema.json`.
pub fn render_stuck_json(stuck: &[StuckProject<'_>]) -> String {
    let items: Vec<serde_json::Value> = stuck
        .iter()
        .map(|sp| {
            let entry = &sp.doc.entries[sp.entry_idx];
            serde_json::json!({
                "file": sp.doc.file.display().to_string(),
                "line": entry.heading_line,
                "locator": locator_for_entry(sp.doc, sp.entry_idx).to_string(),
                "keyword": entry.keyword,
                "title": entry.title,
                "tags": entry.tags,
                "children_done": sp.done,
                "children_actionable": sp.actionable,
                "children_blocked": sp.blocked_count,
                "children_waiting": sp.waiting,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

// endregion
