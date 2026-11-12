// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Format-agnostic `stdin` locator parser for Unix command pipelines.
//!
//! Accepts default `human` query lines (`<file>:<line>: * ...`, 2-space indented
//! grouped lines, and `Locator: <loc>` lines from `org next --pick`), `--format json`
//! payloads, and `--format locator` strings without requiring `--format locator`.

use std::path::Path;

use serde_json::Value;

// region Constants

/// Leading prefixes identifying raw Org-mode document lines rather than piped query output.
const RAW_ORG_PREFIXES: &[&str] = &[
    "* ",
    "** ",
    "*** ",
    "#+",
    ":PROPERTIES:",
    ":END:",
    "SCHEDULED:",
    "DEADLINE:",
    "CLOSED:",
];

/// Metadata prefixes emitted by `org next --pick` and `org query` human views that are not targets.
const IGNORED_HUMAN_PREFIXES: &[&str] = &[
    "Recommended Next Action",
    "Title:",
    "Project:",
    "Vault:",
    "Context:",
    "Effort:",
    "Energy:",
    "Workmode:",
    "Scheduled:",
    "Deadline:",
    "File:",
    "Blocked by:",
    "Waiting for:",
    "Children:",
];

// endregion

// region JSON Payload Extraction

/// Extracts a single locator string from a JSON object (`locator`, `file`+`line`, or `id`).
fn extract_locator_from_json_obj(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(loc) = obj
        .get("locator")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(loc.to_string());
    }
    if let (Some(file), Some(line)) = (
        obj.get("file").and_then(Value::as_str),
        obj.get("line").and_then(Value::as_u64),
    ) {
        return Some(format!("{file}::{line}"));
    }
    if let Some(id) = obj
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(format!("id:{id}"));
    }
    None
}

/// Recursively collects locator strings from a parsed JSON value (`array` or `object` envelope).
fn collect_json_locators(val: &Value, out: &mut Vec<String>) {
    match val {
        Value::Array(items) => {
            for item in items {
                collect_json_locators(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(loc) = extract_locator_from_json_obj(map) {
                if !out.contains(&loc) {
                    out.push(loc);
                }
                return;
            }
            for key in ["nodes", "items", "entries", "next_actions", "days"] {
                if let Some(nested) = map.get(key) {
                    collect_json_locators(nested, out);
                }
            }
        }
        _ => {}
    }
}

/// Attempts to parse `trimmed` as a JSON array or envelope and extract target locators.
fn try_parse_json_stdin(trimmed: &str) -> Option<Vec<String>> {
    if !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }
    let val: Value = serde_json::from_str(trimmed).ok()?;
    let mut out = Vec::new();
    collect_json_locators(&val, &mut out);
    Some(out)
}

// endregion

// region Human & Locator Line Extraction

/// Returns `true` when `line` is a group header such as `@computer (2)` or `Project Apollo (1)`.
fn is_group_header_line(trimmed: &str) -> bool {
    if !trimmed.ends_with(')') {
        return false;
    }
    let Some((prefix, count_part)) = trimmed.rsplit_once(" (") else {
        return false;
    };
    let count_digits = count_part.trim_end_matches(')');
    !prefix.is_empty()
        && !count_digits.is_empty()
        && count_digits.bytes().all(|b| b.is_ascii_digit())
        && !Path::new(trimmed).exists()
}

/// Parses `<file>:<line>: ...` or `<file>:<line>` into `<file>::<line>`.
fn parse_file_colon_line(trimmed: &str) -> Option<String> {
    // Scan colon positions to support Windows drive letters (`C:\...`) and `.org:<line>`.
    for (idx, _) in trimmed.match_indices(':') {
        let file_part = trimmed[..idx].trim();
        if file_part.is_empty() || file_part == "id" || file_part == "olp" {
            continue;
        }
        let after_colon = &trimmed[idx + 1..];
        if after_colon.starts_with(':') {
            continue;
        }
        let digit_len = after_colon
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digit_len == 0 {
            continue;
        }
        let rest = &after_colon[digit_len..];
        if !(rest.is_empty() || rest.starts_with(':')) {
            continue;
        }
        if let Ok(line_no) = after_colon[..digit_len].parse::<usize>() {
            if line_no > 0
                && (file_part.to_ascii_lowercase().ends_with(".org")
                    || Path::new(file_part).exists())
            {
                return Some(format!("{file_part}::{line_no}"));
            }
        }
    }
    None
}

/// Extracts a target locator from a single non-empty text line, or `None` for ignorable headers.
fn parse_single_stdin_line(line: &str) -> Result<Option<String>, String> {
    // 4-space indented lines under `  Blocked by:` are blocker details, not primary matches.
    if line.starts_with("    ") || line.starts_with('\t') {
        return Ok(None);
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Some(loc) = trimmed.strip_prefix("Locator:") {
        let clean = loc.trim();
        return Ok((!clean.is_empty()).then(|| clean.to_string()));
    }
    if IGNORED_HUMAN_PREFIXES
        .iter()
        .any(|p| trimmed.starts_with(p))
        || is_group_header_line(trimmed)
    {
        return Ok(None);
    }
    if RAW_ORG_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
        return Err(format!(
            "org: stdin looks like org-mode content, not locator targets\nfirst offending line: {trimmed}\nhint: pipe locators (e.g. from `org query search`), not org file content"
        ));
    }
    if trimmed.starts_with("id:") || trimmed.contains("::") {
        return Ok(Some(trimmed.to_string()));
    }
    if let Some(converted) = parse_file_colon_line(trimmed) {
        return Ok(Some(converted));
    }
    Ok(Some(trimmed.to_string()))
}

/// Parses piped `stdin` text (`human`, `json`, or `locator` format) into deduplicated target strings.
pub fn parse_piped_stdin_targets(raw: &str) -> Result<Vec<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(json_locs) = try_parse_json_stdin(trimmed) {
        return Ok(json_locs);
    }
    let mut targets = Vec::new();
    for line in raw.lines() {
        if let Some(loc) = parse_single_stdin_line(line)? {
            if !targets.contains(&loc) {
                targets.push(loc);
            }
        }
    }
    Ok(targets)
}

// endregion
