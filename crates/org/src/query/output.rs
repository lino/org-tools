// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output rendering for query results.

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::locator::locator_for_entry;
use serde::Serialize;

/// A matched entry with its document context.
pub struct MatchedEntry<'a> {
    /// The document containing this entry.
    pub doc: &'a OrgDocument,
    /// Index into `doc.entries`.
    pub entry_idx: usize,
}

impl MatchedEntry<'_> {
    /// Get the entry reference.
    pub fn entry(&self) -> &OrgEntry {
        &self.doc.entries[self.entry_idx]
    }
}

/// Render matches in human-readable format.
pub fn render_human(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
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
        let stars = "*".repeat(entry.level);

        out.push_str(&format!(
            "{}:{}: {stars} {kw}{pri}{}{tags}\n",
            m.doc.file.display(),
            entry.heading_line,
            entry.title,
        ));
    }
    out
}

/// JSON output entry.
#[derive(Serialize)]
pub struct JsonEntry {
    pub file: String,
    pub line: usize,
    pub locator: String,
    pub level: usize,
    pub keyword: Option<String>,
    pub priority: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub properties: std::collections::HashMap<String, String>,
    pub scheduled: Option<String>,
    pub deadline: Option<String>,
    pub closed: Option<String>,
    pub clocked_minutes: i64,
    pub blocked: bool,
}

/// Render matches as JSON.
///
/// The `all_docs` parameter enables edna blocker evaluation for the `blocked` field.
pub fn render_json(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let items: Vec<JsonEntry> = matches
        .iter()
        .map(|m| {
            let entry = m.entry();
            let loc = locator_for_entry(m.doc, m.entry_idx);
            let total_clocked: i64 = entry.clocks.iter().filter_map(|c| c.duration_minutes).sum();
            let ctx = EdnaContext {
                all_docs,
                doc: m.doc,
                entry_idx: m.entry_idx,
            };
            let blocked = edna::is_blocked(&ctx);

            JsonEntry {
                file: m.doc.file.display().to_string(),
                line: entry.heading_line,
                locator: loc.to_string(),
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
                blocked,
            }
        })
        .collect();

    serde_json::to_string_pretty(&items).unwrap_or_default()
}

/// Render matches as locator strings (one per line).
pub fn render_locators(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
        let loc = locator_for_entry(m.doc, m.entry_idx);
        out.push_str(&loc.to_string());
        out.push('\n');
    }
    out
}

/// Format a timestamp for display.
fn format_ts(ts: &org_tools_core::rules::timestamp::OrgTimestamp) -> String {
    let time = match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => format!(" {h:02}:{m:02}"),
        _ => String::new(),
    };
    format!("{:04}-{:02}-{:02}{time}", ts.year, ts.month, ts.day)
}

/// Format a single entry line for human output.
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
    let stars = "*".repeat(entry.level);
    format!(
        "{}:{}: {stars} {kw}{pri}{}{tags}",
        m.doc.file.display(),
        entry.heading_line,
        entry.title,
    )
}

// ---------------------------------------------------------------------------
// Blocked view
// ---------------------------------------------------------------------------

/// Render blocked entries with dependency details in human format.
pub fn render_blocked_human(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let mut out = String::new();
    for m in matches {
        out.push_str(&format_entry_line(m));
        out.push('\n');

        let ctx = EdnaContext {
            all_docs,
            doc: m.doc,
            entry_idx: m.entry_idx,
        };
        let details = edna::blocking_details(&ctx);
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

/// JSON entry for blocked view with blocker details.
#[derive(Serialize)]
struct BlockedJsonEntry {
    #[serde(flatten)]
    entry: JsonEntry,
    blocking_entries: Vec<BlockerJsonEntry>,
}

/// JSON representation of a single blocking dependency.
#[derive(Serialize)]
struct BlockerJsonEntry {
    title: String,
    keyword: Option<String>,
    file: String,
    line: usize,
    locator: String,
    condition: String,
}

/// Render blocked entries as JSON with blocker details.
pub fn render_blocked_json(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let items: Vec<BlockedJsonEntry> = matches
        .iter()
        .map(|m| {
            let entry = m.entry();
            let loc = locator_for_entry(m.doc, m.entry_idx);
            let total_clocked: i64 = entry.clocks.iter().filter_map(|c| c.duration_minutes).sum();
            let ctx = EdnaContext {
                all_docs,
                doc: m.doc,
                entry_idx: m.entry_idx,
            };
            let details = edna::blocking_details(&ctx);

            BlockedJsonEntry {
                entry: JsonEntry {
                    file: m.doc.file.display().to_string(),
                    line: entry.heading_line,
                    locator: loc.to_string(),
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
                    blocked: true,
                },
                blocking_entries: details
                    .into_iter()
                    .map(|d| BlockerJsonEntry {
                        title: d.title,
                        keyword: d.keyword,
                        file: d.file,
                        line: d.line,
                        locator: d.locator,
                        condition: d.condition_desc,
                    })
                    .collect(),
            }
        })
        .collect();

    serde_json::to_string_pretty(&items).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Next actions (grouped by @context)
// ---------------------------------------------------------------------------

/// Render matches grouped by `@`-prefixed context tags.
pub fn render_grouped_by_context(matches: &[MatchedEntry<'_>]) -> String {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut no_context: Vec<String> = Vec::new();

    for m in matches {
        let entry = m.entry();
        let all_tags = &entry.tags;
        let ctx_tags: Vec<&String> = all_tags.iter().filter(|t| t.starts_with('@')).collect();
        let line = format_entry_line(m);

        if ctx_tags.is_empty() {
            no_context.push(line);
        } else {
            for tag in ctx_tags {
                groups.entry(tag.clone()).or_default().push(line.clone());
            }
        }
    }

    let mut out = String::new();
    for (ctx, entries) in &groups {
        out.push_str(&format!("{ctx} ({})\n", entries.len()));
        for entry in entries {
            out.push_str(&format!("  {entry}\n"));
        }
        out.push('\n');
    }

    if !no_context.is_empty() {
        out.push_str(&format!("No context ({})\n", no_context.len()));
        for entry in &no_context {
            out.push_str(&format!("  {entry}\n"));
        }
        out.push('\n');
    }

    out
}

// ---------------------------------------------------------------------------
// Waiting view
// ---------------------------------------------------------------------------

/// Render waiting entries with optional `:WAITING_FOR:` details.
pub fn render_waiting_human(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
        out.push_str(&format_entry_line(m));
        out.push('\n');

        if let Some(waiting_for) = m.entry().properties.get("WAITING_FOR") {
            out.push_str(&format!("  Waiting for: {waiting_for}\n"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Stuck projects
// ---------------------------------------------------------------------------

/// A project entry with child status summary.
pub struct StuckProject<'a> {
    /// The document containing this project.
    pub doc: &'a OrgDocument,
    /// Index of the project entry.
    pub entry_idx: usize,
    /// Number of done children.
    pub done: usize,
    /// Number of actionable children.
    pub actionable: usize,
    /// Number of blocked children.
    pub blocked_count: usize,
    /// Number of waiting children.
    pub waiting: usize,
}

/// Find projects (headings with TODO children) that have no actionable children.
pub fn find_stuck_projects<'a>(
    docs: &'a [OrgDocument],
    doc_refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
) -> Vec<StuckProject<'a>> {
    let mut stuck = Vec::new();

    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            // A project = has children with TODO keywords.
            if entry.children.is_empty() {
                continue;
            }

            let has_todo_children = entry
                .children
                .iter()
                .any(|&ci| doc.entries[ci].keyword.is_some());
            if !has_todo_children {
                continue;
            }

            let mut done = 0usize;
            let mut actionable_count = 0usize;
            let mut blocked_count = 0usize;
            let mut waiting_count = 0usize;

            for &ci in &entry.children {
                let child = &doc.entries[ci];
                if child.keyword.is_none() {
                    continue;
                }
                let kw = child.keyword.as_deref().unwrap();

                if doc.todo_keywords.is_done(kw) {
                    done += 1;
                    continue;
                }

                // Check waiting.
                if kw.to_uppercase().contains("WAIT")
                    || child.properties.contains_key("WAITING_FOR")
                {
                    waiting_count += 1;
                    continue;
                }

                // Check blocked.
                let ctx = EdnaContext {
                    all_docs: doc_refs,
                    doc,
                    entry_idx: ci,
                };
                if edna::is_blocked(&ctx) {
                    blocked_count += 1;
                    continue;
                }

                // Check actionable.
                let pred = super::parser::Predicate::Actionable;
                if super::predicate::matches(&pred, child, doc, doc_refs, today) {
                    actionable_count += 1;
                }
            }

            // Stuck = no actionable children.
            if actionable_count == 0 {
                stuck.push(StuckProject {
                    doc,
                    entry_idx: idx,
                    done,
                    actionable: actionable_count,
                    blocked_count,
                    waiting: waiting_count,
                });
            }
        }
    }

    stuck
}

/// Render stuck projects in human format.
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
        let stars = "*".repeat(entry.level);
        out.push_str(&format!(
            "{}:{}: {stars} {kw}{}{tags}\n",
            sp.doc.file.display(),
            entry.heading_line,
            entry.title,
        ));

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
        out.push_str(&format!("  Children: {}\n", parts.join(", ")));
        out.push('\n');
    }
    out
}

/// Render stuck projects as JSON.
pub fn render_stuck_json(stuck: &[StuckProject<'_>]) -> String {
    #[derive(Serialize)]
    struct StuckJson {
        file: String,
        line: usize,
        locator: String,
        keyword: Option<String>,
        title: String,
        tags: Vec<String>,
        children_done: usize,
        children_actionable: usize,
        children_blocked: usize,
        children_waiting: usize,
    }

    let items: Vec<StuckJson> = stuck
        .iter()
        .map(|sp| {
            let entry = &sp.doc.entries[sp.entry_idx];
            let loc = locator_for_entry(sp.doc, sp.entry_idx);
            StuckJson {
                file: sp.doc.file.display().to_string(),
                line: entry.heading_line,
                locator: loc.to_string(),
                keyword: entry.keyword.clone(),
                title: entry.title.clone(),
                tags: entry.tags.clone(),
                children_done: sp.done,
                children_actionable: sp.actionable,
                children_blocked: sp.blocked_count,
                children_waiting: sp.waiting,
            }
        })
        .collect();

    serde_json::to_string_pretty(&items).unwrap_or_default()
}
