// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Multi-format renderers (`human`, `locator`, `org`, `md`, `json`) and composite GTD
//! collector for `org query agenda`.

use std::collections::HashSet;
use std::path::PathBuf;

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::locator::locator_for_entry;
use org_tools_core::planning::{date_to_days, day_of_week_abbrev, days_to_date};
use org_tools_core::rules::timestamp::find_timestamps;
use serde_json::{json, Value};

use super::agenda::{render_agenda_human, AgendaDay, AgendaItem};
use super::output::{
    render_blocked_human, render_blocked_json, render_human, render_json, MatchedEntry,
};
use super::parser::Predicate;
use super::predicate::matches_with_body;

// region Composite GTD Collection

/// Deterministic GTD sections appended when `org query agenda --composite` is requested.
pub struct CompositeAgenda<'a> {
    /// Actionable tasks matching `Predicate::Actionable`.
    pub next_actions: Vec<MatchedEntry<'a>>,
    /// Tasks blocked by unresolved `org-edna` conditions.
    pub blocked: Vec<MatchedEntry<'a>>,
    /// Active TODO tasks lacking both tags (including inherited) and planning timestamps.
    pub inbox: Vec<MatchedEntry<'a>>,
}

impl CompositeAgenda<'_> {
    /// Returns `true` when all three composite GTD sections contain zero entries.
    pub fn is_empty(&self) -> bool {
        self.next_actions.is_empty() && self.blocked.is_empty() && self.inbox.is_empty()
    }
}

/// Returns `true` when `entry` is an active TODO item with no inherited tags and no planning dates.
fn is_inbox_candidate(doc: &OrgDocument, entry_idx: usize, entry: &OrgEntry) -> bool {
    let Some(ref kw) = entry.keyword else {
        return false;
    };
    if doc.todo_keywords.is_done(kw) || !doc.inherited_tags(entry_idx).is_empty() {
        return false;
    }
    entry.planning.scheduled.is_none() && entry.planning.deadline.is_none()
}

/// Collects `Next Actions`, `Blocked`, and `Inbox` GTD sections across `docs`.
pub fn collect_composite_agenda<'a>(
    docs: &'a [OrgDocument],
    refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
) -> CompositeAgenda<'a> {
    let (mut next_actions, mut blocked, mut inbox) = (Vec::new(), Vec::new(), Vec::new());
    let (p_act, p_blk) = (Predicate::Actionable, Predicate::Blocked);
    for doc in docs {
        let mk = |entry_idx| MatchedEntry { doc, entry_idx };
        for (idx, entry) in doc.entries.iter().enumerate() {
            if matches_with_body(&p_act, entry, idx, None, doc, refs, today) {
                next_actions.push(mk(idx));
            }
            if matches_with_body(&p_blk, entry, idx, None, doc, refs, today) {
                blocked.push(mk(idx));
            }
            if is_inbox_candidate(doc, idx, entry) {
                inbox.push(mk(idx));
            }
        }
    }
    CompositeAgenda {
        next_actions,
        blocked,
        inbox,
    }
}

// endregion

// region Human & Locator Renderers

/// Renders the human-readable agenda timeline and optional composite GTD sections.
pub fn render_agenda_report_human(
    days: &[AgendaDay<'_>],
    composite: Option<&CompositeAgenda<'_>>,
    doc_refs: &[&OrgDocument],
) -> String {
    let mut out = render_agenda_human(days);
    let Some(comp) = composite else {
        return out;
    };
    if out.is_empty() && comp.is_empty() {
        return out;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    let next_txt = render_human(&comp.next_actions);
    append_human_section(&mut out, "Next Actions", comp.next_actions.len(), &next_txt);
    out.push('\n');
    let blk_txt = render_blocked_human(&comp.blocked, doc_refs);
    append_human_section(&mut out, "Blocked", comp.blocked.len(), blk_txt.trim_end());
    out.push('\n');
    let inbox_txt = render_human(&comp.inbox);
    append_human_section(&mut out, "Inbox", comp.inbox.len(), &inbox_txt);
    out
}

/// Appends one titled GTD section to the human-readable composite output buffer.
fn append_human_section(out: &mut String, title: &str, count: usize, body: &str) {
    out.push_str(&format!("{title} ({count})\n"));
    let trimmed = body.trim_end_matches('\n');
    if trimmed.is_empty() {
        return out.push_str("  (none)\n");
    }
    for line in trimmed.split('\n') {
        let pad = if line.is_empty() { "" } else { "  " };
        out.push_str(&format!("{pad}{line}\n"));
    }
}

/// Renders deduplicated `file::line` locators in deterministic first-seen encounter order.
pub fn render_agenda_locators(
    days: &[AgendaDay<'_>],
    composite: Option<&CompositeAgenda<'_>>,
) -> String {
    let (mut seen, mut out) = (HashSet::new(), String::new());
    let mut push_loc = |file: PathBuf, line: usize| {
        if seen.insert((file.clone(), line)) {
            out.push_str(&format!("{}::{line}\n", file.display()));
        }
    };
    for item in days.iter().flat_map(|d| &d.items) {
        push_loc(item.file.to_path_buf(), item.entry.heading_line);
    }
    if let Some(c) = composite {
        let gtd_lists = [&c.next_actions, &c.blocked, &c.inbox];
        for m in gtd_lists.into_iter().flatten() {
            push_loc(m.doc.file.clone(), m.entry().heading_line);
        }
    }
    out
}

// endregion

// region Org & Markdown Document Renderers

/// Formats the inclusive date window header label `(YYYY-MM-DD .. YYYY-MM-DD)`.
fn format_horizon_range(start: (u16, u8, u8), num_days: usize) -> String {
    let start_str = format!("{:04}-{:02}-{:02}", start.0, start.1, start.2);
    if num_days <= 1 {
        return start_str;
    }
    let (ey, em, ed) = days_to_date(date_to_days(start.0, start.1, start.2) + num_days as i64 - 1);
    format!("{start_str} .. {ey:04}-{em:02}-{ed:02}")
}

/// Wraps a bare link target in Markdown backticks or conflict-free Org code delimiters.
fn wrap_bare_link_target(t: &str, md: bool) -> String {
    match (md, t.contains('`'), t.contains('='), t.contains('~')) {
        (true, true, _, _) => format!("`` {t} ``"),
        (true, false, _, _) => format!("`{t}`"),
        (false, _, false, _) => format!("={t}="),
        (false, _, true, false) => format!("~{t}~"),
        (false, _, true, true) => t.to_string(),
    }
}

/// Formats the interior of an Org `[[...]]` bracket link for Org or Markdown output.
fn format_flattened_link(inner: &str, markdown: bool) -> String {
    let Some((target, desc)) = inner.split_once("][") else {
        return wrap_bare_link_target(inner, markdown);
    };
    let is_http = target.starts_with("http://") || target.starts_with("https://");
    if markdown && is_http && !target.contains(')') {
        format!("[{desc}]({target})")
    } else {
        desc.to_string()
    }
}

/// Flattens `[[target][desc]]` / `[[target]]` links and neutralizes unclosed `[[` tokens.
fn flatten_org_links(raw: &str, markdown: bool) -> String {
    let (mut out, mut rest) = (String::with_capacity(raw.len()), raw);
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        if let Some(close) = after_open.find("]]") {
            out.push_str(&format_flattened_link(&after_open[..close], markdown));
            rest = &after_open[close + 2..];
        } else {
            out.push_str("[ [");
            rest = after_open;
        }
    }
    out.push_str(rest);
    out
}

/// Rewrites `<YYYY-MM-DD...>` and `[YYYY-MM-DD...]` spans to `(...)` so `W027` never triggers.
fn neutralize_org_timestamps(text: &str) -> String {
    let spans = find_timestamps(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let (mut out, mut cursor) = (String::with_capacity(text.len()), 0);
    for (_ts, start, end) in spans {
        out.push_str(&text[cursor..start]);
        out.push_str(&format!("({})", &text[start + 1..end - 1]));
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Sanitizes inline heading or blocker text for standalone Org (`false`) or Markdown (`true`).
fn sanitize_inline_title(raw: &str, markdown: bool) -> String {
    let mut s = flatten_org_links(raw, markdown);
    if markdown {
        return s.trim_end().to_string();
    }
    let b_pairs = [("[[", "[ ["), ("]]]", "] ]]"), ("<<", "< <"), (">>", "> >")];
    for (pat, rep) in b_pairs.into_iter().chain([("[fn:", "[fn - ")]) {
        while s.contains(pat) {
            s = s.replace(pat, rep);
        }
    }
    while !find_timestamps(&s).is_empty() {
        s = neutralize_org_timestamps(&s);
    }
    s = s
        .replace("SCHEDULED:", "Scheduled")
        .replace("DEADLINE:", "Deadline");
    s.replace("CLOSED:", "Closed").trim_end().to_string()
}

/// Formats `keyword + priority + sanitized_title + tags` for Org (`false`) or Markdown (`true`).
fn format_entry_inline(entry: &OrgEntry, md: bool) -> String {
    let kw = match (entry.keyword.as_deref(), md) {
        (Some(k), true) => format!("**{k}** "),
        (Some(k), false) => format!("{k} "),
        (None, _) => String::new(),
    };
    let pri = match (entry.priority, md) {
        (Some(p), true) => format!("`[#{p}]` "),
        (Some(p), false) => format!("[#{p}] "),
        (None, _) => String::new(),
    };
    let wrap = if md { '`' } else { '=' };
    let tags = if entry.tags.is_empty() {
        String::new()
    } else {
        format!(" {wrap}:{}:{wrap}", entry.tags.join(":"))
    };
    let title = sanitize_inline_title(&entry.title, md);
    format!("{kw}{pri}{title}{tags}").trim_end().to_string()
}

/// Formats one [`AgendaItem`] bullet line for Org (`false`) or Markdown (`true`).
fn format_doc_timeline_bullet(item: &AgendaItem<'_>, md: bool) -> String {
    let inline = format_entry_inline(item.entry, md);
    let hm = item.timestamp.hour.zip(item.timestamp.minute);
    let time = hm.map_or(String::new(), |(h, m)| format!("{h:02}:{m:02} "));
    let (f, ln, lbl) = (
        item.file.display(),
        item.entry.heading_line,
        item.kind.human_label(),
    );
    let (w, sep, b0, b1) = match md {
        true => ('`', " —", "*(", ")*"),
        false => ('=', "", "— ", ""),
    };
    let g = item.habit.as_ref().map(|h| h.consistency_graph.as_str());
    let hab = g.map_or(String::new(), |cg| format!(" {w}[{cg}]{w}"));
    format!("- {w}{f}:{ln}{w}{sep} {inline} {b0}{time}{lbl}{b1}{hab}\n")
}

/// Formats a concise blocker summary string for `m` across `doc_refs`.
fn summarize_blockers(m: &MatchedEntry<'_>, refs: &[&OrgDocument], md: bool) -> Option<String> {
    let details = edna::blocking_details(&EdnaContext {
        all_docs: refs,
        doc: m.doc,
        entry_idx: m.entry_idx,
    });
    if details.is_empty() {
        return None;
    }
    let w = if md { '`' } else { '=' };
    let fmt_d = |d: edna::BlockerDetail| {
        let clean = sanitize_inline_title(&d.title, md);
        format!("{w}{}:{}{w} {clean}", d.file, d.line)
    };
    let parts: Vec<String> = details.into_iter().map(fmt_d).collect();
    Some(format!("blocked by {}", parts.join(", ")))
}

/// Appends one GTD section (`Next Actions`, `Blocked`, or `Inbox`) for Org or Markdown output.
fn append_doc_gtd_section(
    out: &mut String,
    heading: &str,
    entries: &[MatchedEntry<'_>],
    blocker_docs: Option<&[&OrgDocument]>,
    md: bool,
) {
    let (h_pre, h_post, w, sep, em) = match md {
        true => ("## ", "\n", '`', " —", "*"),
        false => ("* ", "", '=', "", ""),
    };
    out.push_str(&format!("\n{h_pre}{heading}\n{h_post}"));
    if entries.is_empty() {
        out.push_str(&format!("- {em}None.{em}\n"));
        return;
    }
    for m in entries {
        let inline = format_entry_inline(m.entry(), md);
        let raw_note = blocker_docs.and_then(|refs| summarize_blockers(m, refs, md));
        let note = raw_note.map_or(String::new(), |n| format!(" {em}({n}){em}"));
        let (f, ln) = (m.doc.file.display(), m.entry().heading_line);
        out.push_str(&format!("- {w}{f}:{ln}{w}{sep} {inline}{note}\n"));
    }
}

/// Appends non-empty timeline day subsections (or an empty-window note) for Org or Markdown.
fn append_doc_timeline_days(out: &mut String, non_empty: &[&AgendaDay<'_>], md: bool) {
    let (pre, post, em) = match md {
        true => ("###", "\n", "*"),
        false => ("**", "", ""),
    };
    if non_empty.is_empty() {
        let msg = "No scheduled or deadline items in window.";
        out.push_str(&format!("- {em}{msg}{em}\n"));
        return;
    }
    for (i, d) in non_empty.iter().enumerate() {
        if md && i > 0 {
            out.push('\n');
        }
        let (y, m, day) = (d.year, d.month, d.day);
        let dow = day_of_week_abbrev(y, m, day);
        out.push_str(&format!("{pre} {y:04}-{m:02}-{day:02} {dow}\n{post}"));
        for item in &d.items {
            out.push_str(&format_doc_timeline_bullet(item, md));
        }
    }
}

/// Renders the timeline and optional composite GTD sections as Org (`false`) or Markdown (`true`).
pub fn render_agenda_doc(
    start_date: (u16, u8, u8),
    num_days: usize,
    days: &[AgendaDay<'_>],
    composite: Option<&CompositeAgenda<'_>>,
    doc_refs: &[&OrgDocument],
    md: bool,
) -> String {
    let range = format_horizon_range(start_date, num_days);
    let mut out = if md {
        format!("# Agenda ({range})\n\n## Timeline\n\n")
    } else {
        format!("#+TITLE: Agenda ({range})\n\n* Timeline\n")
    };
    let non_empty: Vec<&AgendaDay<'_>> = days.iter().filter(|d| !d.items.is_empty()).collect();
    append_doc_timeline_days(&mut out, &non_empty, md);
    if let Some(comp) = composite {
        append_doc_gtd_section(&mut out, "Next Actions", &comp.next_actions, None, md);
        append_doc_gtd_section(&mut out, "Blocked", &comp.blocked, Some(doc_refs), md);
        append_doc_gtd_section(&mut out, "Inbox", &comp.inbox, None, md);
    }
    out
}

// endregion

// region JSON Envelope Serialization

/// Serializes one [`AgendaItem`] into its `org-agenda-output.schema.json` object representation.
fn agenda_item_to_json(item: &AgendaItem<'_>) -> Value {
    let hm = item.timestamp.hour.zip(item.timestamp.minute);
    let time = hm.map(|(h, m)| format!("{h:02}:{m:02}"));
    let mut val = json!({
        "file": item.file.display().to_string(),
        "line": item.entry.heading_line,
        "locator": locator_for_entry(item.doc, item.entry_idx).to_string(),
        "keyword": item.entry.keyword,
        "priority": item.entry.priority.map(|p| p.to_string()),
        "title": item.entry.title,
        "tags": item.entry.tags,
        "kind": item.kind.json_kind(),
        "label": item.kind.human_label(),
        "timestamp": item.timestamp.to_string(),
        "time": time,
        "overdue_days": item.kind.overdue_days(),
        "warning_days_remaining": item.kind.warning_days_remaining(),
    });
    if let Some(hv) = item
        .habit
        .as_ref()
        .and_then(|h| serde_json::to_value(h).ok())
    {
        val["habit"] = hv;
    }
    val
}

/// Builds the full `org-agenda-output.schema.json` JSON value including `telemetry`.
pub fn build_agenda_json_value(
    start: (u16, u8, u8),
    num_days: usize,
    warning_days: u32,
    days: &[AgendaDay<'_>],
    composite: Option<&CompositeAgenda<'_>>,
    doc_refs: &[&OrgDocument],
    telemetry: Value,
) -> Value {
    let day_to_json = |d: &AgendaDay<'_>| {
        let items: Vec<Value> = d.items.iter().map(agenda_item_to_json).collect();
        let date = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
        let dow = day_of_week_abbrev(d.year, d.month, d.day);
        json!({ "date": date, "day_of_week": dow, "items": items })
    };
    let non_empty = days.iter().filter(|d| !d.items.is_empty());
    let days_timeline: Vec<Value> = non_empty.map(day_to_json).collect();

    let mut obj = serde_json::Map::new();
    let from = format!("{:04}-{:02}-{:02}", start.0, start.1, start.2);
    obj.insert("from".into(), Value::String(from));
    obj.insert("days".into(), json!(num_days));
    obj.insert("warning_days".into(), json!(warning_days));
    obj.insert("days_timeline".into(), Value::Array(days_timeline));
    if let Some(comp) = composite {
        let parse_arr = |raw: String| serde_json::from_str::<Value>(&raw).unwrap_or(json!([]));
        let next = parse_arr(render_json(&comp.next_actions, doc_refs));
        let blk = parse_arr(render_blocked_json(&comp.blocked, doc_refs));
        let inb = parse_arr(render_json(&comp.inbox, doc_refs));
        let comp_val = json!({ "next_actions": next, "blocked": blk, "inbox": inb });
        obj.insert("composite".into(), comp_val);
    }
    obj.insert("telemetry".into(), telemetry);
    Value::Object(obj)
}

// endregion
