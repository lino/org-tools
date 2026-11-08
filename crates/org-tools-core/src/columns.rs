// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `#+COLUMNS:` and `:COLUMNS:` tabular property view parser and bottom-up rollup engine.
//!
//! Spec: [§7.5 Column View](https://orgmode.org/manual/Column-View.html).

use std::collections::{BTreeMap, HashMap};
use std::iter::Sum;

use serde::{Deserialize, Serialize};

use crate::document::OrgDocument;
use crate::locator::locator_for_entry;

/// Default Org-mode column view format when no `#+COLUMNS:` or `:COLUMNS:` is specified.
pub const DEFAULT_COLUMNS_FORMAT: &str = "%25ITEM %TODO %3PRIORITY %TAGS";

/// Summary rollup operator computed bottom-up from child entries to parent entries (§7.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummaryType {
    /// Numeric addition rollup (`{+}`).
    #[serde(rename = "+")]
    NumericSum,
    /// Duration addition rollup in `H:MM` format (`{:}`).
    #[serde(rename = ":")]
    DurationSum,
    /// Completion ratio rollup (`[done/total]`) from child TODO states or checklists (`{X}`).
    #[serde(rename = "X")]
    Checkbox,
}

/// A single compiled column specification from `#+COLUMNS:` or `:COLUMNS:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnSpec {
    /// Canonical uppercase property identifier (e.g. `"ITEM"`, `"EFFORT"`).
    pub property: String,
    /// Column header display title (defaults to `property` when `(TITLE)` is omitted).
    pub title: String,
    /// Optional explicit display width in characters from `%[WIDTH]`.
    pub width: Option<usize>,
    /// Optional bottom-up summary rollup operator (`{+}`, `{:}`, `{X}`).
    pub summary: Option<SummaryType>,
}

/// A single evaluated heading row in a [`ColumnView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnRow {
    /// Canonical [`OrgLocator`](crate::locator::OrgLocator) string for the entry.
    pub locator: String,
    /// Path to the Org source file containing the entry.
    pub file: String,
    /// One-based line number of the heading.
    pub line: usize,
    /// Heading outline level (`1` = top-level `*`).
    pub level: usize,
    /// Evaluated column cell values keyed by canonical uppercase property name.
    pub values: BTreeMap<String, String>,
}

/// Complete tabular column view with compiled header specifications and evaluated rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnView {
    /// Compiled column header specifications in display order.
    pub columns: Vec<ColumnSpec>,
    /// Evaluated heading rows in outline order.
    pub rows: Vec<ColumnRow>,
}

/// Splits a column format string at top-level `%` markers outside `(...)` and `{...}`.
fn split_column_tokens(spec: &str) -> Vec<&str> {
    let (mut tokens, mut start, mut depth) = (Vec::new(), None, 0usize);
    for (idx, ch) in spec.char_indices() {
        match ch {
            '(' | '{' => depth += 1,
            ')' | '}' => depth = depth.saturating_sub(1),
            '%' if depth == 0 => {
                if let Some(s) = start.replace(idx) {
                    tokens.push(spec[s..idx].trim());
                }
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push(spec[s..].trim());
    }
    tokens
}

/// Extracts a leading `(inner)` or `{inner}` segment and returns `(Some(inner), remainder)`.
fn extract_delimited(input: &str, open: char, close: char) -> (Option<&str>, &str) {
    if let Some(rest) = input.strip_prefix(open) {
        if let Some(end) = rest.find(close) {
            return (Some(rest[..end].trim()), &rest[end + close.len_utf8()..]);
        }
    }
    (None, input)
}

/// Parses a single `%[WIDTH]PROPERTY[(TITLE)][{SUMMARY-TYPE}]` token.
fn parse_single_column_token(token: &str) -> Option<ColumnSpec> {
    let rest = token.strip_prefix('%')?.trim();
    let dlen = rest.bytes().take_while(u8::is_ascii_digit).count();
    let width = (dlen > 0).then(|| rest[..dlen].parse().ok()).flatten();
    let tail = &rest[dlen..];
    let pend = tail
        .find(|c: char| c == '(' || c == '{' || c.is_whitespace())
        .unwrap_or(tail.len());
    let raw_prop = tail[..pend].trim();
    if raw_prop.is_empty() {
        return None;
    }
    let property = raw_prop.to_ascii_uppercase();
    let (title, after_t) = extract_delimited(tail[pend..].trim(), '(', ')');
    let (sum_raw, _) = extract_delimited(after_t.trim_start(), '{', '}');
    let summary = match sum_raw.map(str::trim) {
        Some("+") => Some(SummaryType::NumericSum),
        Some(":") => Some(SummaryType::DurationSum),
        Some("X" | "x" | "X/" | "x/" | "X%" | "x%") => Some(SummaryType::Checkbox),
        _ => None,
    };
    let title = title
        .filter(|t| !t.is_empty())
        .map_or_else(|| property.clone(), str::to_string);
    Some(ColumnSpec {
        property,
        title,
        width,
        summary,
    })
}

/// Parses a `#+COLUMNS:` or `:COLUMNS:` format string into ordered [`ColumnSpec`] items.
pub fn parse_columns_format(spec: &str) -> Vec<ColumnSpec> {
    let cols: Vec<ColumnSpec> = split_column_tokens(spec)
        .into_iter()
        .filter_map(parse_single_column_token)
        .collect();
    if cols.is_empty() && spec != DEFAULT_COLUMNS_FORMAT {
        return parse_columns_format(DEFAULT_COLUMNS_FORMAT);
    }
    cols
}

/// Looks up `key` in `map` case-insensitively and returns the non-empty trimmed value.
fn lookup_non_empty<'a>(map: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// Resolves the effective column format string for `doc` and optional `root_entry_idx`.
pub fn resolve_columns_format(
    doc: &OrgDocument,
    root_entry_idx: Option<usize>,
    cli_override: Option<&str>,
) -> String {
    if let Some(spec) = cli_override.map(str::trim).filter(|s| !s.is_empty()) {
        return spec.to_string();
    }
    let mut cur = root_entry_idx;
    while let Some(entry) = cur.and_then(|idx| doc.entries.get(idx)) {
        if let Some(spec) = lookup_non_empty(&entry.properties, "COLUMNS") {
            return spec.to_string();
        }
        cur = entry.parent;
    }
    lookup_non_empty(&doc.file_keywords, "COLUMNS")
        .or_else(|| lookup_non_empty(&doc.file_properties, "COLUMNS"))
        .or_else(|| lookup_non_empty(&doc.default_properties, "COLUMNS"))
        .or_else(|| {
            root_entry_idx.is_none().then(|| {
                doc.entries
                    .iter()
                    .filter(|e| e.parent.is_none())
                    .find_map(|e| lookup_non_empty(&e.properties, "COLUMNS"))
            })?
        })
        .unwrap_or(DEFAULT_COLUMNS_FORMAT)
        .to_string()
}

/// Parses unit-suffixed duration tokens (`2h`, `45m`, `1h 30m`, `1.5h`) into minutes.
fn parse_unit_duration(trimmed: &str) -> Option<i64> {
    let (mut rest, mut total_mins, mut matched) = (trimmed, 0.0f64, false);
    while !rest.is_empty() {
        rest = rest.trim_start();
        let nlen = rest
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'.')
            .count();
        if nlen == 0 {
            return None;
        }
        let val: f64 = rest[..nlen].parse().ok()?;
        rest = rest[nlen..].trim_start();
        let ulen = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let unit = rest[..ulen].to_ascii_lowercase();
        rest = &rest[ulen..];
        let factor = match unit.as_str() {
            "h" | "hr" | "hrs" | "hour" | "hours" => 60.0,
            "m" | "min" | "mins" | "minute" | "minutes" | "" if rest.trim().is_empty() => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 1.0,
            "d" | "day" | "days" => 1440.0,
            _ => return None,
        };
        total_mins += val * factor;
        matched = true;
    }
    matched.then(|| total_mins.round() as i64)
}

/// Parses a duration string (`H:MM`, `Nh`, `Nm`, or `Nh Nm`) into total minutes.
pub fn parse_duration_minutes(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((h, rest)) = trimmed.split_once(':') {
        let m = rest.split(':').next()?;
        return Some(h.trim().parse::<i64>().ok()? * 60 + m.trim().parse::<i64>().ok()?);
    }
    parse_unit_duration(trimmed)
}

/// Formats a duration in minutes as `H:MM` (e.g. `135` -> `"2:15"`).
pub fn format_duration_hmm(minutes: i64) -> String {
    format!("{}:{:02}", minutes / 60, (minutes % 60).abs())
}

/// Formats a floating-point sum as an integer when exact, or trimmed decimal otherwise.
fn format_numeric(val: f64) -> String {
    if (val - val.round()).abs() < 1e-9 {
        return format!("{}", val.round() as i64);
    }
    format!("{val:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// Formats a slice of tag strings as `:tag1:tag2:`, or `""` when empty.
fn format_tag_list<S: AsRef<str>>(tags: &[S]) -> String {
    let items: Vec<&str> = tags.iter().map(AsRef::as_ref).collect();
    if items.is_empty() {
        String::new()
    } else {
        format!(":{}:", items.join(":"))
    }
}

/// Evaluates the raw (unrolled) property or special property `prop` on `doc.entries[idx]`.
fn raw_entry_property(doc: &OrgDocument, idx: usize, prop: &str) -> String {
    let e = &doc.entries[idx];
    let fmt_ts = |ts: &Option<_>| ts.as_ref().map_or_else(String::new, ToString::to_string);
    match prop {
        "ITEM" => e.title.clone(),
        "TODO" => e.keyword.clone().unwrap_or_default(),
        "PRIORITY" => e.priority.map(|c| c.to_string()).unwrap_or_default(),
        "TAGS" => format_tag_list(&e.tags),
        "ALLTAGS" => format_tag_list(&doc.inherited_tags(idx)),
        "SCHEDULED" => fmt_ts(&e.planning.scheduled),
        "DEADLINE" => fmt_ts(&e.planning.deadline),
        "CLOSED" => fmt_ts(&e.planning.closed),
        other => doc.property(idx, other).unwrap_or_default().to_string(),
    }
}

/// Computes rolled-up `CLOCKSUM` minutes for all entries bottom-up.
fn compute_clocksum_minutes(doc: &OrgDocument) -> Vec<i64> {
    let mut mins = vec![0i64; doc.entries.len()];
    for idx in (0..doc.entries.len()).rev() {
        let own: i64 = doc.entries[idx]
            .clocks
            .iter()
            .filter(|c| c.end.is_some())
            .filter_map(|c| c.duration_minutes)
            .sum();
        let child_sum: i64 = doc.entries[idx].children.iter().map(|&c| mins[c]).sum();
        mins[idx] = own + child_sum;
    }
    mins
}

/// Evaluates a generic bottom-up additive summary column (`{+}` or `{:}`).
fn eval_additive_column<T: Copy + Sum>(
    doc: &OrgDocument,
    prop: &str,
    parse_fn: fn(&str) -> Option<T>,
    fmt_fn: fn(T) -> String,
) -> Vec<String> {
    let mut vals: Vec<Option<T>> = vec![None; doc.entries.len()];
    let mut out = vec![String::new(); doc.entries.len()];
    for idx in (0..doc.entries.len()).rev() {
        let child_vals: Vec<T> = doc.entries[idx]
            .children
            .iter()
            .filter_map(|&c| vals[c])
            .collect();
        if !child_vals.is_empty() {
            let sum: T = child_vals.into_iter().sum();
            vals[idx] = Some(sum);
            out[idx] = fmt_fn(sum);
        } else {
            let raw = raw_entry_property(doc, idx, prop);
            vals[idx] = parse_fn(&raw);
            out[idx] = raw;
        }
    }
    out
}

/// Counts `(done, total)` across direct `children` that carry a TODO keyword.
fn child_todo_progress(doc: &OrgDocument, children: &[usize]) -> (usize, usize) {
    children.iter().fold((0, 0), |(done, total), &c| {
        match doc.entries[c].keyword.as_deref() {
            Some(kw) => (done + usize::from(doc.todo_keywords.is_done(kw)), total + 1),
            None => (done, total),
        }
    })
}

/// Evaluates a `{X}` completion summary column (`[done/total]`) across `doc.entries` bottom-up.
fn eval_checkbox_column(doc: &OrgDocument, prop: &str) -> Vec<String> {
    let mut counts: Vec<Option<(usize, usize)>> = vec![None; doc.entries.len()];
    let mut out = vec![String::new(); doc.entries.len()];
    for idx in (0..doc.entries.len()).rev() {
        let entry = &doc.entries[idx];
        let (todo_done, todo_total) = child_todo_progress(doc, &entry.children);
        let (sub_done, sub_total) = entry
            .children
            .iter()
            .filter_map(|&c| counts[c])
            .fold((0, 0), |(d, t), (cd, ct)| (d + cd, t + ct));
        if todo_total > 0 {
            counts[idx] = Some((todo_done, todo_total));
            out[idx] = format!("[{todo_done}/{todo_total}]");
        } else if sub_total > 0 {
            counts[idx] = Some((sub_done, sub_total));
            out[idx] = format!("[{sub_done}/{sub_total}]");
        } else if !entry.checklist.is_empty() {
            let s = entry.checklist_summary();
            counts[idx] = Some((s.checked, s.total));
            out[idx] = format!("[{}/{}]", s.checked, s.total);
        } else {
            let raw = raw_entry_property(doc, idx, prop);
            counts[idx] = match raw.trim() {
                "[X]" | "[x]" => Some((1, 1)),
                "[ ]" | "[-]" => Some((0, 1)),
                _ => None,
            };
            out[idx] = raw;
        }
    }
    out
}

/// Evaluates `col` across all entries in `doc`.
fn evaluate_column(doc: &OrgDocument, col: &ColumnSpec, clocksums: &[i64]) -> Vec<String> {
    let p = col.property.as_str();
    if p == "CLOCKSUM" {
        return clocksums
            .iter()
            .map(|&m| {
                if m > 0 {
                    format_duration_hmm(m)
                } else {
                    String::new()
                }
            })
            .collect();
    }
    match col.summary {
        Some(SummaryType::NumericSum) => {
            eval_additive_column(doc, p, |s| s.trim().parse::<f64>().ok(), format_numeric)
        }
        Some(SummaryType::DurationSum) => {
            eval_additive_column(doc, p, parse_duration_minutes, format_duration_hmm)
        }
        Some(SummaryType::Checkbox) => eval_checkbox_column(doc, p),
        None => (0..doc.entries.len())
            .map(|i| raw_entry_property(doc, i, p))
            .collect(),
    }
}

/// Collects entry indices in outline order for either the full document or `root_entry_idx`.
fn collect_subtree_indices(doc: &OrgDocument, root_entry_idx: Option<usize>) -> Vec<usize> {
    let Some(root_idx) = root_entry_idx else {
        return (0..doc.entries.len()).collect();
    };
    let Some(root) = doc.entries.get(root_idx) else {
        return Vec::new();
    };
    std::iter::once(root_idx)
        .chain(
            ((root_idx + 1)..doc.entries.len())
                .take_while(|&idx| doc.entries[idx].level > root.level),
        )
        .collect()
}

/// Evaluates `doc` (or the subtree at `root_entry_idx`) into a [`ColumnView`] with parent rollups.
pub fn build_column_view(
    doc: &OrgDocument,
    root_entry_idx: Option<usize>,
    cli_override: Option<&str>,
) -> ColumnView {
    let columns = parse_columns_format(&resolve_columns_format(doc, root_entry_idx, cli_override));
    let clocksums = compute_clocksum_minutes(doc);
    let cols_eval: Vec<Vec<String>> = columns
        .iter()
        .map(|col| evaluate_column(doc, col, &clocksums))
        .collect();
    let file = doc.file.display().to_string();
    let rows = collect_subtree_indices(doc, root_entry_idx)
        .into_iter()
        .map(|idx| {
            let values = columns
                .iter()
                .enumerate()
                .map(|(ci, col)| (col.property.clone(), cols_eval[ci][idx].clone()))
                .collect();
            ColumnRow {
                locator: locator_for_entry(doc, idx).to_string(),
                file: file.clone(),
                line: doc.entries[idx].heading_line,
                level: doc.entries[idx].level,
                values,
            }
        })
        .collect();
    ColumnView { columns, rows }
}
