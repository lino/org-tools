// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Leaf actionability, multi-dimensional GTD filtering, effort duration parsing,
//! and deterministic urgency ranking for `org [query] next`.

use serde::{Deserialize, Serialize};

use crate::document::{OrgDocument, OrgEntry};
use crate::edna::{self, EdnaContext};
use crate::planning::date_to_days;
use crate::roam::lookup_prop_ci;
use crate::rules::timestamp::{is_valid_date, OrgTimestamp};

use super::ResolvedProject;

// region Public Types

/// Grouping dimension for `org [query] next --group-by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NextGroupBy {
    /// Group by `@context` tag or `:CONTEXT:` property (default).
    #[default]
    Context,
    /// Group by resolved owning project name.
    Project,
    /// Group by configured `org-roam` vault directory alias.
    Vault,
    /// Group by priority cookie (`[#A]`, `[#B]`, `[#C]`, or `No priority`).
    Priority,
    /// Group by `:ENERGY:` property (`low`, `medium`, `high`).
    Energy,
    /// Group by `:WORKMODE:` property (`deep`, `shallow`, `any`).
    Workmode,
    /// Flat ungrouped list in deterministic urgency order.
    None,
}

/// Multi-dimensional filter criteria for `org [query] next`.
#[derive(Debug, Clone, Default)]
pub struct NextQueryFilter {
    /// Case-insensitive project title, `:ID:`, alias, or file-stem filter (`--project|-p`).
    pub project: Option<String>,
    /// Context filter matching `@context` tag or `:CONTEXT:` property (`--context|-c`).
    pub context: Option<String>,
    /// Energy filter matching `:ENERGY:` property (`--energy|-e`).
    pub energy: Option<String>,
    /// Workmode filter matching `:WORKMODE:` property (`--workmode|-w`).
    pub workmode: Option<String>,
    /// Maximum `:EFFORT:` duration in minutes (`--max-effort`).
    pub max_effort_mins: Option<u32>,
    /// Minimum `:EFFORT:` duration in minutes (`--min-effort`).
    pub min_effort_mins: Option<u32>,
    /// Priority filter (`A`, `B`, `C`, or `>=B`, `<=B`) (`--priority`).
    pub priority: Option<String>,
    /// Required inherited/file tags (`--tag`).
    pub tags: Vec<String>,
    /// Goal filter matching `:GOAL:` property (`--goal`).
    pub goal: Option<String>,
    /// Due filter (`overdue`, `today`, `soon`, or `YYYY-MM-DD`) (`--due`).
    pub due: Option<String>,
    /// Restrict results to entries with explicit `NEXT` keyword (`--next-only`).
    pub next_only: bool,
    /// Include entries scheduled in the future (`SCHEDULED: > today`) (`--include-future`).
    pub include_future: bool,
    /// Deadline warning window in days for `--due soon` and urgency ranking.
    pub warning_days: u32,
}

// endregion

// region Effort & Property Lookup Helpers

/// Parses an Org `:EFFORT:` duration (`H:MM`, `MMm`, `Hh`, or integer minutes) into minutes.
pub fn parse_effort_minutes(raw: &str) -> Option<u32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((h_str, m_str)) = trimmed.split_once(':') {
        let hours = h_str.trim().parse::<u32>().ok()?;
        let mins = m_str.trim().parse::<u32>().ok()?;
        return Some(hours.saturating_mul(60).saturating_add(mins));
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(m_str) = lower
        .strip_suffix("min")
        .or_else(|| lower.strip_suffix('m'))
    {
        if let Some((h_part, m_part)) = m_str.split_once('h') {
            let h = h_part.trim().parse::<u32>().ok()?;
            let m = m_part.trim().parse::<u32>().ok()?;
            return Some(h.saturating_mul(60).saturating_add(m));
        }
        return m_str.trim().parse::<u32>().ok();
    }
    if let Some(h_str) = lower.strip_suffix('h') {
        let h = h_str.trim().parse::<u32>().ok()?;
        return Some(h.saturating_mul(60));
    }
    trimmed.parse::<u32>().ok()
}

/// Looks up a property on `entry_idx`, walking up ancestor headings and level-0 file properties.
pub fn inherited_property<'a>(
    doc: &'a OrgDocument,
    entry_idx: usize,
    key: &str,
) -> Option<&'a str> {
    let mut cur = Some(entry_idx);
    while let Some(idx) = cur {
        if let Some(val) = lookup_prop_ci(&doc.entries[idx].properties, key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        cur = doc.entries[idx].parent;
    }
    lookup_prop_ci(&doc.file_properties, key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Extracts all `@context` tags and `:CONTEXT:` tokens for `entry_idx` normalized as `@<name>`.
pub fn entry_contexts(doc: &OrgDocument, entry_idx: usize) -> Vec<String> {
    let mut out = Vec::new();
    for tag in doc.inherited_tags(entry_idx) {
        if tag.starts_with('@') && !out.iter().any(|c: &String| c.eq_ignore_ascii_case(tag)) {
            out.push(tag.to_string());
        }
    }
    if let Some(ctx_prop) = inherited_property(doc, entry_idx, "CONTEXT") {
        for raw_tok in ctx_prop
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let norm = if raw_tok.starts_with('@') {
                raw_tok.to_string()
            } else {
                format!("@{raw_tok}")
            };
            if !out.iter().any(|c| c.eq_ignore_ascii_case(&norm)) {
                out.push(norm);
            }
        }
    }
    out
}

// endregion

// region Leaf Actionability

/// Returns `true` when `entry` is a GTD project container heading or has open child TODOs.
fn is_project_or_parent_container(doc: &OrgDocument, entry: &OrgEntry) -> bool {
    if entry
        .keyword
        .as_deref()
        .is_some_and(|kw| kw.eq_ignore_ascii_case("PROJECT"))
    {
        return true;
    }
    if lookup_prop_ci(&entry.properties, "TYPE")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("project"))
    {
        return true;
    }
    if entry.tags.iter().any(|t| t.eq_ignore_ascii_case("project")) {
        return true;
    }
    entry.children.iter().any(|&ci| {
        doc.entries[ci]
            .keyword
            .as_deref()
            .is_some_and(|kw| !doc.todo_keywords.is_done(kw))
    })
}

/// Returns `true` when `entry_idx` is a leaf actionable next action eligible for `org [query] next`.
pub fn is_leaf_actionable_for_next(
    doc: &OrgDocument,
    entry_idx: usize,
    all_docs: &[&OrgDocument],
    today: (u16, u8, u8),
    next_only: bool,
    include_future: bool,
) -> bool {
    let entry = &doc.entries[entry_idx];
    let Some(kw) = entry.keyword.as_deref() else {
        return false;
    };
    if doc.todo_keywords.is_done(kw) {
        return false;
    }
    if next_only && !kw.eq_ignore_ascii_case("NEXT") {
        return false;
    }
    let kw_upper = kw.to_ascii_uppercase();
    if kw_upper.contains("WAIT")
        || kw_upper == "HOLD"
        || kw_upper.contains("BLOCK")
        || lookup_prop_ci(&entry.properties, "WAITING_FOR").is_some()
    {
        return false;
    }
    if is_project_or_parent_container(doc, entry) {
        return false;
    }
    if !include_future {
        if let Some(ref sched) = entry.planning.scheduled {
            if date_to_days(sched.year, sched.month, sched.day)
                > date_to_days(today.0, today.1, today.2)
            {
                return false;
            }
        }
    }
    !edna::is_blocked(&EdnaContext {
        all_docs,
        doc,
        entry_idx,
    })
}

// endregion

// region Multi-Dimensional Filtering & Urgency Ranking

/// Checks whether `entry.priority` satisfies `spec` (`A`, `B`, `C`, `>=B`, `<=B`, etc.).
fn matches_priority_spec(entry_pri: Option<char>, spec: &str) -> bool {
    let trimmed = spec.trim().to_ascii_uppercase();
    let Some(pri) = entry_pri.map(|c| c.to_ascii_uppercase()) else {
        return false;
    };
    if let Some(rest) = trimmed.strip_prefix(">=") {
        return rest.chars().next().is_some_and(|t| pri <= t);
    }
    if let Some(rest) = trimmed.strip_prefix("<=") {
        return rest.chars().next().is_some_and(|t| pri >= t);
    }
    if let Some(rest) = trimmed.strip_prefix('>') {
        return rest.chars().next().is_some_and(|t| pri < t);
    }
    if let Some(rest) = trimmed.strip_prefix('<') {
        return rest.chars().next().is_some_and(|t| pri > t);
    }
    let clean = trimmed
        .trim_start_matches('=')
        .trim_matches(['[', '#', ']']);
    clean.chars().next().is_some_and(|t| pri == t)
}

/// Checks whether `entry`'s `DEADLINE:` or `SCHEDULED:` satisfies `--due <overdue|today|soon|YYYY-MM-DD>`.
fn matches_due_spec(
    entry: &OrgEntry,
    due_spec: &str,
    today: (u16, u8, u8),
    warning_days: u32,
) -> bool {
    let today_days = date_to_days(today.0, today.1, today.2);
    let ts_days: Vec<i64> = [
        entry.planning.deadline.as_ref(),
        entry.planning.scheduled.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(|ts| date_to_days(ts.year, ts.month, ts.day))
    .collect();
    if ts_days.is_empty() {
        return false;
    }
    let lower = due_spec.trim().to_ascii_lowercase();
    match lower.as_str() {
        "overdue" => ts_days.iter().any(|&d| d < today_days),
        "today" => ts_days.iter().any(|&d| d <= today_days),
        "soon" => {
            let horizon = today_days + i64::from(warning_days.max(7));
            ts_days.iter().any(|&d| d <= horizon)
        }
        other => {
            let parts: Vec<&str> = other.split('-').collect();
            if let [ys, ms, ds] = parts.as_slice() {
                if let (Ok(y), Ok(m), Ok(d)) =
                    (ys.parse::<u16>(), ms.parse::<u8>(), ds.parse::<u8>())
                {
                    if is_valid_date(y, m, d) {
                        let bound = date_to_days(y, m, d);
                        return ts_days.iter().any(|&t| t <= bound);
                    }
                }
            }
            false
        }
    }
}

/// Checks whether `entry_idx` satisfies effort, energy, workmode, priority, tag, goal, and due filters.
pub fn entry_matches_next_filter(
    doc: &OrgDocument,
    entry_idx: usize,
    project: &ResolvedProject,
    filter: &NextQueryFilter,
    today: (u16, u8, u8),
) -> bool {
    if let Some(ref proj_q) = filter.project {
        if !project.matches_filter(proj_q) {
            return false;
        }
    }
    if let Some(ref ctx_q) = filter.context {
        let needle = ctx_q.trim().trim_start_matches('@');
        let matched = entry_contexts(doc, entry_idx)
            .iter()
            .any(|c| c.trim_start_matches('@').eq_ignore_ascii_case(needle));
        if !matched {
            return false;
        }
    }
    if let Some(ref energy_q) = filter.energy {
        if !inherited_property(doc, entry_idx, "ENERGY")
            .is_some_and(|v| v.eq_ignore_ascii_case(energy_q.trim()))
        {
            return false;
        }
    }
    if let Some(ref wm_q) = filter.workmode {
        if !inherited_property(doc, entry_idx, "WORKMODE")
            .is_some_and(|v| v.eq_ignore_ascii_case(wm_q.trim()))
        {
            return false;
        }
    }
    if filter.max_effort_mins.is_some() || filter.min_effort_mins.is_some() {
        let Some(eff_mins) =
            inherited_property(doc, entry_idx, "EFFORT").and_then(parse_effort_minutes)
        else {
            return false;
        };
        if filter.max_effort_mins.is_some_and(|max| eff_mins > max)
            || filter.min_effort_mins.is_some_and(|min| eff_mins < min)
        {
            return false;
        }
    }
    let entry = &doc.entries[entry_idx];
    if let Some(ref pri_spec) = filter.priority {
        if !matches_priority_spec(entry.priority, pri_spec) {
            return false;
        }
    }
    if !filter.tags.is_empty() {
        let inherited = doc.inherited_tags(entry_idx);
        if !filter
            .tags
            .iter()
            .all(|t| inherited.iter().any(|it| it.eq_ignore_ascii_case(t.trim())))
        {
            return false;
        }
    }
    if let Some(ref goal_q) = filter.goal {
        let needle = goal_q.trim().to_ascii_lowercase();
        if !inherited_property(doc, entry_idx, "GOAL")
            .is_some_and(|v| v.to_ascii_lowercase().contains(&needle))
        {
            return false;
        }
    }
    if let Some(ref due_q) = filter.due {
        if !matches_due_spec(entry, due_q, today, filter.warning_days) {
            return false;
        }
    }
    true
}

/// Computes `(urgency_bucket, earliest_date_days)` from `deadline` and `scheduled`.
fn planning_urgency_key(
    deadline: Option<&OrgTimestamp>,
    scheduled: Option<&OrgTimestamp>,
    today: (u16, u8, u8),
    warning_days: u32,
) -> (u8, i64) {
    let today_days = date_to_days(today.0, today.1, today.2);
    let warn_horizon = today_days + i64::from(warning_days.max(7));
    let dates: Vec<i64> = [deadline, scheduled]
        .into_iter()
        .flatten()
        .map(|ts| date_to_days(ts.year, ts.month, ts.day))
        .collect();
    let Some(&earliest) = dates.iter().min() else {
        return (4, i64::MAX);
    };
    let bucket = if earliest < today_days {
        0
    } else if earliest == today_days {
        1
    } else if earliest <= warn_horizon {
        2
    } else {
        3
    };
    (bucket, earliest)
}

/// Computes the deterministic urgency ranking key for `entry_idx` in `doc`.
///
/// Order: `NEXT` > `TODO`, planning urgency (`overdue` -> `today` -> `soon` -> `unscheduled`),
/// priority (`[#A]` > `[#B]`/default > `[#C]`), shortest `:EFFORT:`, then `(file, line)`.
pub fn next_candidate_sort_key(
    doc: &OrgDocument,
    entry_idx: usize,
    today: (u16, u8, u8),
    warning_days: u32,
) -> (u8, u8, i64, u8, u32, String, usize) {
    let entry = &doc.entries[entry_idx];
    let next_rank = u8::from(
        !entry
            .keyword
            .as_deref()
            .is_some_and(|kw| kw.eq_ignore_ascii_case("NEXT")),
    );
    let (urg_bucket, earliest_days) = planning_urgency_key(
        entry.planning.deadline.as_ref(),
        entry.planning.scheduled.as_ref(),
        today,
        warning_days,
    );
    let pri_rank = match entry.priority.map(|c| c.to_ascii_uppercase()) {
        Some('A') => 0,
        Some('B') | None => 1,
        Some('C') => 2,
        Some(_) => 3,
    };
    let effort_rank = inherited_property(doc, entry_idx, "EFFORT")
        .and_then(parse_effort_minutes)
        .unwrap_or(u32::MAX);
    (
        next_rank,
        urg_bucket,
        earliest_days,
        pri_rank,
        effort_rank,
        doc.file.display().to_string(),
        entry.heading_line,
    )
}

// endregion
