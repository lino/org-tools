// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Protected-block and inline-verbatim-aware `[[id:...]]` link scanner.

use crate::document::OrgDocument;
use crate::roam::{is_roam_excluded, lookup_prop_ci, RoamLink};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::heading::parse_heading;
use crate::source::SourceFile;

/// A raw `[[id:...]]` link occurrence with exact 1-based line and column positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdLink {
    /// Target `:ID:` value with any `::search` suffix stripped and trimmed.
    pub target_id: String,
    /// Optional link description text from `[[id:<target>][<description>]]`.
    pub description: Option<String>,
    /// One-based line number where `[[` starts.
    pub line: usize,
    /// One-based column number where `[[` starts.
    pub column: usize,
    /// Trimmed source line containing the link.
    pub context: String,
}

/// Returns `true` when `rest` is empty or starts with horizontal whitespace.
fn starts_with_ws_or_eol(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')
}

/// Returns `true` when `raw_line` is an Org `#` comment line or `:` fixed-width line.
fn is_comment_or_fixed_width(raw_line: &str) -> bool {
    let trimmed = raw_line.trim_start();
    trimmed.strip_prefix('#').is_some_and(starts_with_ws_or_eol)
        || trimmed.strip_prefix(':').is_some_and(starts_with_ws_or_eol)
}

/// Returns `true` when `prev` can precede an Org inline `=...=` or `~...~` marker.
fn is_emphasis_pre(prev: Option<char>) -> bool {
    prev.is_none_or(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '-' | '(' | '{' | '\'' | '"' | '*' | '/' | '_' | '+' | '|' | '['
            )
    })
}

/// Returns `true` when `next` can follow a closing Org inline `=...=` or `~...~` marker.
fn is_emphasis_post(next: Option<char>) -> bool {
    next.is_none_or(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '-' | '.'
                    | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '\''
                    | ')'
                    | '}'
                    | ']'
                    | '"'
                    | '\\'
                    | '*'
                    | '/'
                    | '_'
                    | '+'
                    | '|'
            )
    })
}

/// Advances past an Org inline `=...=` or `~...~` span starting at `pos` when valid.
fn skip_org_verbatim(line: &str, pos: usize, marker: char) -> Option<usize> {
    if !is_emphasis_pre(line[..pos].chars().next_back()) {
        return None;
    }
    let body = line.get(pos + 1..)?;
    let first_inner = body.chars().next()?;
    if first_inner.is_whitespace() || first_inner == marker {
        return None;
    }

    for (rel, ch) in body.char_indices() {
        if ch != marker {
            continue;
        }
        let close_pos = pos + 1 + rel;
        let prev_ok = line[..close_pos]
            .chars()
            .next_back()
            .is_some_and(|p| !p.is_whitespace());
        let next_char = line.get(close_pos + 1..).and_then(|s| s.chars().next());
        if prev_ok && is_emphasis_post(next_char) {
            return Some(close_pos + 1);
        }
    }
    None
}

/// Advances past a backtick inline code span (`` `...` ``) starting at `pos` when closed on the line.
fn skip_backtick_span(line: &str, pos: usize) -> Option<usize> {
    let rest = line.get(pos..)?;
    let tick_len = rest.bytes().take_while(|&b| b == b'`').count();
    if tick_len == 0 {
        return None;
    }
    let fence = &rest[..tick_len];
    let after_open = &rest[tick_len..];
    let close_rel = after_open.find(fence)?;
    Some(pos + tick_len + close_rel + tick_len)
}

/// Advances past any inline verbatim/code span starting at byte offset `pos`.
fn try_skip_inline_code(line: &str, pos: usize) -> Option<usize> {
    match line.as_bytes().get(pos)? {
        b'=' => skip_org_verbatim(line, pos, '='),
        b'~' => skip_org_verbatim(line, pos, '~'),
        b'`' => skip_backtick_span(line, pos),
        _ => None,
    }
}

/// Parses an Org bracket link `[[path]]` or `[[path][desc]]` at `pos`, returning `((raw_path, desc), next_pos)`.
fn parse_bracket_link(line: &str, pos: usize) -> Option<((&str, Option<&str>), usize)> {
    let after_open = line.get(pos..)?.strip_prefix("[[")?;
    let target_close = after_open.find(']')?;
    let raw_path = &after_open[..target_close];
    if raw_path.contains('[') {
        return None;
    }
    let tail = &after_open[target_close..];

    if let Some(rest) = tail.strip_prefix("]]") {
        let consumed = line.len() - rest.len();
        return Some(((raw_path, None), consumed));
    }
    let desc_tail = tail.strip_prefix("][")?;
    let desc_close = desc_tail.find("]]")?;
    let desc = &desc_tail[..desc_close];
    let consumed = pos + 2 + target_close + 2 + desc_close + 2;
    Some(((raw_path, Some(desc)), consumed))
}

/// Extracts a clean `:ID:` target from `raw_path` when it uses the `id:` scheme.
fn extract_id_target(raw_path: &str) -> Option<String> {
    let trimmed = raw_path.trim();
    if !trimmed
        .get(..3)
        .is_some_and(|p| p.eq_ignore_ascii_case("id:"))
    {
        return None;
    }
    let after_scheme = &trimmed[3..];
    let without_search = after_scheme
        .split_once("::")
        .map_or(after_scheme, |(base, _)| base)
        .trim();
    (!without_search.is_empty()).then(|| without_search.to_string())
}

/// Scans a single unprotected line and appends extracted [`RawIdLink`] occurrences into `out`.
fn scan_line_links(
    source: &SourceFile,
    raw_line: &str,
    line_offset: usize,
    out: &mut Vec<RawIdLink>,
) {
    let mut pos = 0;
    let context = raw_line.trim().to_string();
    while pos < raw_line.len() {
        if let Some(next_pos) = try_skip_inline_code(raw_line, pos) {
            pos = next_pos;
            continue;
        }
        if let Some(((raw_path, desc), next_pos)) = parse_bracket_link(raw_line, pos) {
            if let Some(target_id) = extract_id_target(raw_path) {
                let (line, column) = source.line_col(line_offset + pos);
                out.push(RawIdLink {
                    target_id,
                    description: desc.map(str::to_string),
                    line,
                    column,
                    context: context.clone(),
                });
            }
            pos = next_pos;
            continue;
        }
        let step = raw_line[pos..].chars().next().map_or(1, char::len_utf8);
        pos += step;
    }
}

/// Updates `in_prop_drawer` state and returns `true` when the current line must be skipped as part of `:PROPERTIES:`.
fn step_property_drawer_state(raw_line: &str, in_prop_drawer: &mut bool) -> bool {
    let trimmed = raw_line.trim();
    if *in_prop_drawer {
        if parse_heading(raw_line).is_some() {
            *in_prop_drawer = false;
            return false;
        }
        if trimmed.eq_ignore_ascii_case(":END:") {
            *in_prop_drawer = false;
        }
        return true;
    }
    if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
        *in_prop_drawer = true;
        return true;
    }
    false
}

/// Scans unprotected lines of `source` for `[[id:...]]` links, skipping comments, drawers, and inline verbatim spans.
pub fn scan_id_links(source: &SourceFile) -> Vec<RawIdLink> {
    let protected = protected_regions(&source.content);
    let mut out = Vec::new();
    let mut in_prop_drawer = false;
    let mut offset = 0;

    for (line_idx, line) in source.content.split('\n').enumerate() {
        let raw = line.strip_suffix('\r').unwrap_or(line);
        if !is_protected(line_idx, &protected)
            && !step_property_drawer_state(raw, &mut in_prop_drawer)
            && !is_comment_or_fixed_width(raw)
        {
            scan_line_links(source, raw, offset, &mut out);
        }
        offset += line.len() + 1;
    }
    out
}

/// Finds the nearest enclosing non-excluded heading or file-level `:ID:` containing `line_num`.
fn enclosing_roam_node_id(doc: &OrgDocument, line_num: usize) -> Option<String> {
    let deepest_idx = doc
        .entries
        .iter()
        .rposition(|entry| entry.heading_line <= line_num);
    let mut cur = deepest_idx;
    while let Some(idx) = cur {
        let entry = &doc.entries[idx];
        if !is_roam_excluded(&entry.properties) {
            if let Some(id) = lookup_prop_ci(&entry.properties, "ID")
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(id.to_string());
            }
        }
        cur = entry.parent;
    }
    if !is_roam_excluded(&doc.file_properties) {
        return lookup_prop_ci(&doc.file_properties, "ID")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    None
}

/// Extracts [`RoamLink`] edges from `source` and attributes each link to its enclosing non-excluded node in `doc`.
pub fn extract_roam_links(source: &SourceFile, doc: &OrgDocument) -> Vec<RoamLink> {
    let source_file = doc.file.display().to_string();
    scan_id_links(source)
        .into_iter()
        .map(|raw| RoamLink {
            source_id: enclosing_roam_node_id(doc, raw.line),
            source_file: source_file.clone(),
            target_id: raw.target_id,
            description: raw.description,
            line: raw.line,
            context: raw.context,
        })
        .collect()
}
