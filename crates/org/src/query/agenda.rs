// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Agenda view engine projecting scheduled, deadline, overdue, warning, and body timestamps.
//!
//! Spec: [§8.3 Deadlines and Scheduling](https://orgmode.org/manual/Deadlines-and-Scheduling.html),
//! [§8.3.2 Repeated tasks](https://orgmode.org/manual/Repeated-tasks.html),
//! [§11 Agenda Views](https://orgmode.org/manual/Agenda-Views.html),
//! [Syntax: Planning](https://orgmode.org/worg/org-syntax.html#Planning),
//! [Syntax: Timestamps](https://orgmode.org/worg/org-syntax.html#Timestamps).

use std::path::Path;

use org_tools_core::config::DEFAULT_DEADLINE_WARNING_DAYS;
use org_tools_core::document::{OrgDocument, OrgEntry};
pub use org_tools_core::logbook::{build_habit_summary, HabitSummary};
use org_tools_core::planning::{
    date_to_days, day_of_week_abbrev, days_to_date, effective_deadline_warning_days,
    extract_entry_active_body_timestamps, is_first_only_warning, project_repeating_dates,
};
use org_tools_core::rules::timestamp::{is_valid_date, OrgTimestamp};
use org_tools_core::source::SourceFile;

// region Public Types

/// Reason an [`OrgEntry`] appears on a given [`AgendaDay`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaKind {
    /// Entry scheduled on this calendar day (base or projected repeater occurrence).
    Scheduled,
    /// Entry deadline falling on this calendar day (base or projected repeater occurrence).
    Deadline,
    /// Uncompleted past `SCHEDULED:` item shown on Day 0 (`days_overdue >= 1`).
    OverdueScheduled(i64),
    /// Uncompleted past `DEADLINE:` item shown on Day 0 (`days_overdue >= 1`).
    OverdueDeadline(i64),
    /// Upcoming uncompleted `DEADLINE:` pre-warning shown on Day 0 (`days_until >= 1`).
    DeadlineWarning(i64),
    /// Unprotected active `<YYYY-MM-DD ...>` timestamp in the heading or entry body.
    Timestamp,
}

impl AgendaKind {
    /// Formats the canonical Emacs-style human status label for this agenda item kind.
    pub fn human_label(self) -> String {
        match self {
            Self::Scheduled => "Scheduled".to_string(),
            Self::Deadline => "DEADLINE".to_string(),
            Self::OverdueScheduled(d) => format!("Sched. {d}x: Scheduled"),
            Self::OverdueDeadline(d) => format!("In -{d} d.: DEADLINE"),
            Self::DeadlineWarning(d) => format!("In {d} d.: DEADLINE"),
            Self::Timestamp => "Timestamp".to_string(),
        }
    }

    /// Returns the snake_case discriminator string matching `org-agenda-output.schema.json`.
    pub fn json_kind(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Deadline => "deadline",
            Self::OverdueScheduled(_) => "overdue_scheduled",
            Self::OverdueDeadline(_) => "overdue_deadline",
            Self::DeadlineWarning(_) => "deadline_warning",
            Self::Timestamp => "timestamp",
        }
    }

    /// Returns the positive overdue day count when this item is overdue, or `None`.
    pub fn overdue_days(self) -> Option<i64> {
        match self {
            Self::OverdueScheduled(d) | Self::OverdueDeadline(d) => Some(d),
            _ => None,
        }
    }

    /// Returns the positive days remaining until due date for a deadline warning, or `None`.
    pub fn warning_days_remaining(self) -> Option<i64> {
        match self {
            Self::DeadlineWarning(d) => Some(d),
            _ => None,
        }
    }
}

/// One projected or direct agenda occurrence linked to its enclosing [`OrgDocument`] and [`OrgEntry`].
#[derive(Debug, Clone)]
pub struct AgendaItem<'a> {
    /// Reference to the enclosing parsed [`OrgDocument`].
    pub doc: &'a OrgDocument,
    /// Zero-based index of `entry` within `doc.entries`.
    pub entry_idx: usize,
    /// Reference to the matched [`OrgEntry`].
    pub entry: &'a OrgEntry,
    /// Path to the Org source file containing `entry`.
    pub file: &'a Path,
    /// Reason this item appears on this day.
    pub kind: AgendaKind,
    /// Base or repeater-projected active [`OrgTimestamp`].
    pub timestamp: OrgTimestamp,
    /// Optional `org-habit` repeater and 21-day consistency summary.
    pub habit: Option<HabitSummary>,
}

/// One calendar date within the requested agenda horizon and its matched items.
#[derive(Debug, Clone)]
pub struct AgendaDay<'a> {
    /// Four-digit proleptic Gregorian year.
    pub year: u16,
    /// Calendar month (`1..=12`).
    pub month: u8,
    /// Day of month (`1..=31`).
    pub day: u8,
    /// Ordered agenda items occurring or surfaced on this day.
    pub items: Vec<AgendaItem<'a>>,
}

/// Configuration inputs controlling [`build_agenda_with_options`].
#[derive(Debug, Clone, Copy)]
pub struct AgendaBuildOptions<'a> {
    /// Optional raw [`SourceFile`] slice aligned 1-to-1 with `docs` for body timestamp scanning.
    pub sources: Option<&'a [SourceFile]>,
    /// Inclusive `(year, month, day)` start date (Day 0).
    pub start_date: (u16, u8, u8),
    /// Number of calendar days to include in the timeline window.
    pub num_days: usize,
    /// Global fallback deadline warning window in days (`[agenda].deadline_warning_days`).
    pub warning_days: u32,
}

// endregion

// region Agenda Projection Engine

/// Per-entry projection context bundling shared document, window, and completion state.
struct EntryScanCtx<'a, 'b> {
    /// Enclosing [`OrgDocument`].
    doc: &'a OrgDocument,
    /// Entry index inside `doc.entries`.
    idx: usize,
    /// Epoch day number of `opts.start_date` (Day 0).
    start_days: i64,
    /// Caller-supplied agenda build options.
    opts: &'b AgendaBuildOptions<'b>,
    /// Whether this entry carries a `DONE`-state TODO keyword.
    is_done: bool,
    /// Optional `org-habit` summary computed for this entry.
    habit: Option<HabitSummary>,
}

impl<'a> EntryScanCtx<'a, '_> {
    /// Appends an [`AgendaItem`] onto `days[day_idx]`.
    fn push(&self, days: &mut [AgendaDay<'a>], day_idx: usize, kind: AgendaKind, ts: OrgTimestamp) {
        days[day_idx].items.push(AgendaItem {
            doc: self.doc,
            entry_idx: self.idx,
            entry: &self.doc.entries[self.idx],
            file: &self.doc.file,
            kind,
            timestamp: ts,
            habit: self.habit.clone(),
        });
    }
}

/// Builds a multi-day agenda from `docs` using [`DEFAULT_DEADLINE_WARNING_DAYS`] (`14` days).
#[allow(dead_code)]
pub fn build_agenda<'a>(
    docs: &'a [OrgDocument],
    start_date: (u16, u8, u8),
    num_days: usize,
) -> Vec<AgendaDay<'a>> {
    let warning_days = DEFAULT_DEADLINE_WARNING_DAYS;
    build_agenda_with_options(
        docs,
        AgendaBuildOptions {
            sources: None,
            start_date,
            num_days,
            warning_days,
        },
    )
}

/// Builds a multi-day agenda projecting repeaters, overdue items, deadline warnings, and body timestamps.
pub fn build_agenda_with_options<'a>(
    docs: &'a [OrgDocument],
    opts: AgendaBuildOptions<'_>,
) -> Vec<AgendaDay<'a>> {
    let start_days = date_to_days(opts.start_date.0, opts.start_date.1, opts.start_date.2);
    let mut days: Vec<AgendaDay<'a>> = (0..opts.num_days as i64)
        .map(|off| {
            let (year, month, day) = days_to_date(start_days + off);
            AgendaDay {
                year,
                month,
                day,
                items: Vec::new(),
            }
        })
        .collect();
    if days.is_empty() {
        return days;
    }
    for (doc_idx, doc) in docs.iter().enumerate() {
        let src_opt = opts.sources.and_then(|srcs| srcs.get(doc_idx));
        for (idx, entry) in doc.entries.iter().enumerate() {
            let is_done = entry
                .keyword
                .as_deref()
                .is_some_and(|kw| doc.todo_keywords.is_done(kw));
            let habit = build_habit_summary(src_opt, doc, idx, opts.start_date);
            let ctx = EntryScanCtx {
                doc,
                idx,
                start_days,
                opts: &opts,
                is_done,
                habit,
            };
            collect_entry_agenda_items(&ctx, &mut days, entry, src_opt);
        }
    }
    days
}

/// Clones `ts` onto a projected `(year, month, day)` occurrence with updated weekday name.
fn projected_timestamp(ts: &OrgTimestamp, year: u16, month: u8, day: u8) -> OrgTimestamp {
    OrgTimestamp {
        year,
        month,
        day,
        dayname: Some(day_of_week_abbrev(year, month, day).to_string()),
        ..ts.clone()
    }
}

/// Returns `true` when `ts` is an active timestamp with a valid calendar date.
fn is_active_valid_ts(ts: &OrgTimestamp) -> bool {
    ts.active && is_valid_date(ts.year, ts.month, ts.day)
}

/// Projects scheduled, deadline, and active body timestamps for one `entry` into `days`.
fn collect_entry_agenda_items<'a>(
    ctx: &EntryScanCtx<'a, '_>,
    days: &mut [AgendaDay<'a>],
    entry: &OrgEntry,
    src_opt: Option<&SourceFile>,
) {
    let plan = &entry.planning;
    if let Some(ts) = plan.scheduled.as_ref().filter(|t| is_active_valid_ts(t)) {
        record_planning_item(ctx, days, ts, false);
    }
    if let Some(ts) = plan.deadline.as_ref().filter(|t| is_active_valid_ts(t)) {
        record_planning_item(ctx, days, ts, true);
        if !ctx.is_done {
            record_deadline_warning(ctx, days, ts);
        }
    }
    if let Some(src) = src_opt {
        for body_ts in extract_entry_active_body_timestamps(src, ctx.doc, ctx.idx) {
            record_body_timestamp(ctx, days, &body_ts.timestamp);
        }
    }
}

/// Inserts base, Day-0 overdue, and projected repeater occurrences for `SCHEDULED:` or `DEADLINE:`.
fn record_planning_item<'a>(
    ctx: &EntryScanCtx<'a, '_>,
    days: &mut [AgendaDay<'a>],
    ts: &OrgTimestamp,
    is_deadline: bool,
) {
    let offset = date_to_days(ts.year, ts.month, ts.day) - ctx.start_days;
    let (on_day, overdue) = if is_deadline {
        (AgendaKind::Deadline, AgendaKind::OverdueDeadline(-offset))
    } else {
        (AgendaKind::Scheduled, AgendaKind::OverdueScheduled(-offset))
    };
    if (0..days.len() as i64).contains(&offset) {
        ctx.push(days, offset as usize, on_day, ts.clone());
    } else if offset < 0 && !ctx.is_done {
        ctx.push(days, 0, overdue, ts.clone());
    }
    if !ctx.is_done {
        for (py, pm, pd) in project_repeating_dates(ts, ctx.opts.start_date, ctx.opts.num_days) {
            let proj_idx = (date_to_days(py, pm, pd) - ctx.start_days) as usize;
            ctx.push(days, proj_idx, on_day, projected_timestamp(ts, py, pm, pd));
        }
    }
}

/// Inserts a Day-0 [`AgendaKind::DeadlineWarning`] item when `ts` falls inside its warning window.
fn record_deadline_warning<'a>(
    ctx: &EntryScanCtx<'a, '_>,
    days: &mut [AgendaDay<'a>],
    ts: &OrgTimestamp,
) {
    let lead_days = effective_deadline_warning_days(ts, ctx.opts.warning_days);
    if lead_days <= 0 {
        return;
    }
    let offset = date_to_days(ts.year, ts.month, ts.day) - ctx.start_days;
    if offset >= 1 && offset <= lead_days {
        ctx.push(days, 0, AgendaKind::DeadlineWarning(offset), ts.clone());
        return;
    }
    if offset <= 0 && ts.repeater.is_some() && !is_first_only_warning(ts) {
        let tomorrow = days_to_date(ctx.start_days + 1);
        if let Some(&(py, pm, pd)) =
            project_repeating_dates(ts, tomorrow, lead_days as usize).first()
        {
            let rem = date_to_days(py, pm, pd) - ctx.start_days;
            let proj = projected_timestamp(ts, py, pm, pd);
            ctx.push(days, 0, AgendaKind::DeadlineWarning(rem), proj);
        }
    }
}

/// Inserts base and projected repeater occurrences for an unprotected active body timestamp.
fn record_body_timestamp<'a>(
    ctx: &EntryScanCtx<'a, '_>,
    days: &mut [AgendaDay<'a>],
    ts: &OrgTimestamp,
) {
    let offset = date_to_days(ts.year, ts.month, ts.day) - ctx.start_days;
    if offset >= 0 && (offset as usize) < days.len() {
        ctx.push(days, offset as usize, AgendaKind::Timestamp, ts.clone());
    }
    if !ctx.is_done {
        for (py, pm, pd) in project_repeating_dates(ts, ctx.opts.start_date, ctx.opts.num_days) {
            let proj_idx = (date_to_days(py, pm, pd) - ctx.start_days) as usize;
            let proj = projected_timestamp(ts, py, pm, pd);
            ctx.push(days, proj_idx, AgendaKind::Timestamp, proj);
        }
    }
}

/// Public wrapper around [`date_to_days`] used by the query predicate evaluator.
pub fn date_to_days_pub(year: u16, month: u8, day: u8) -> i64 {
    date_to_days(year, month, day)
}

// endregion

// region Human Timeline Renderer

/// Renders the agenda timeline in Emacs-like human-readable text format.
pub fn render_agenda_human(days: &[AgendaDay<'_>]) -> String {
    let mut out = String::new();
    for d in days.iter().filter(|d| !d.items.is_empty()) {
        let (y, m, day) = (d.year, d.month, d.day);
        let dow = day_of_week_abbrev(y, m, day);
        out.push_str(&format!("{y:04}-{m:02}-{day:02} {dow}\n"));
        for item in &d.items {
            out.push_str(&format_human_agenda_item(item));
        }
    }
    out
}

/// Formats one [`AgendaItem`] row inside [`render_agenda_human`].
fn format_human_agenda_item(item: &AgendaItem<'_>) -> String {
    let e = item.entry;
    let kw = e
        .keyword
        .as_deref()
        .map_or(String::new(), |k| format!("{k} "));
    let pri = e.priority.map_or(String::new(), |p| format!("[#{p}] "));
    let tags = if e.tags.is_empty() {
        String::new()
    } else {
        format!(" :{}:", e.tags.join(":"))
    };
    let hm = item.timestamp.hour.zip(item.timestamp.minute);
    let time = hm.map_or(String::new(), |(h, m)| format!("{h:02}:{m:02} "));
    let hab = item
        .habit
        .as_ref()
        .map_or(String::new(), |h| format!(" [{}]", h.consistency_graph));
    let (file, ln, title, lbl) = (
        item.file.display(),
        e.heading_line,
        &e.title,
        item.kind.human_label(),
    );
    format!("  {file}:{ln}: {kw}{pri}{title}{tags}  {time}{lbl}{hab}\n")
}

// endregion

// region Tests

/// Unit tests for multi-day agenda projection and human timeline rendering.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies proleptic Gregorian date roundtrips and weekday abbreviations.
    #[test]
    fn date_roundtrip_and_weekday_names() {
        for (y, m, d, dow) in [(2024, 1, 1, "Mon"), (2026, 3, 21, "Sat")] {
            assert_eq!(days_to_date(date_to_days_pub(y, m, d)), (y, m, d));
            assert_eq!(day_of_week_abbrev(y, m, d), dow);
        }
    }

    /// Verifies basic scheduled, deadline, and Day-0 deadline warning projection.
    #[test]
    fn build_agenda_basic_and_warning_suppression() {
        let text = "* TODO Task\nSCHEDULED: <2024-06-16 Sun 09:00>\n* TODO DL\nDEADLINE: <2024-06-17 Mon>\n";
        let source = SourceFile::new("test.org", text.to_string());
        let docs = [OrgDocument::from_source(&source)];
        let days = build_agenda(&docs, (2024, 6, 15), 7);
        assert_eq!(days[0].items[0].kind, AgendaKind::DeadlineWarning(2));
        assert_eq!(days[1].items[0].kind, AgendaKind::Scheduled);
        assert_eq!(days[2].items[0].kind, AgendaKind::Deadline);

        let opts = AgendaBuildOptions {
            sources: Some(std::slice::from_ref(&source)),
            start_date: (2024, 6, 15),
            num_days: 7,
            warning_days: 0,
        };
        assert!(build_agenda_with_options(&docs, opts)[0].items.is_empty());
    }

    /// Verifies overdue scheduled and deadline tasks appear on Day 0 while completed past tasks do not.
    #[test]
    fn build_agenda_overdue_tasks() {
        let content = concat!(
            "* TODO Overdue Sched\nSCHEDULED: <2024-06-14 Fri>\n",
            "* TODO Overdue Deadline\nDEADLINE: <2024-06-12 Wed>\n",
            "* DONE Completed Past\nSCHEDULED: <2024-06-14 Fri>\n",
        );
        let source = SourceFile::new("test.org", content.to_string());
        let docs = [OrgDocument::from_source(&source)];
        let days = build_agenda(&docs, (2024, 6, 15), 7);
        assert_eq!(days[0].items.len(), 2);
        assert_eq!(days[0].items[0].kind, AgendaKind::OverdueScheduled(1));
        assert_eq!(days[0].items[1].kind, AgendaKind::OverdueDeadline(3));
        let rendered = render_agenda_human(&days);
        assert!(rendered.contains("Sched. 1x: Scheduled") && !rendered.contains("Completed Past"));
    }
}

// endregion
