// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Result enrichment helpers for `JsonEntry` items: body, links, digest, checkboxes, and metadata.

use org_tools_core::document::OrgDocument;
use org_tools_core::locator::FILE_NODE_ENTRY_INDEX;

use super::digest::{compute_canonical_digest, extract_clean_body, extract_links};
use crate::query::output::{build_json_entry, JsonEntry, MatchedEntry};

/// Applies body or link enrichment for a given include directive.
fn enrich_body_and_links(
    entry: &mut JsonEntry,
    slice_str: &str,
    is_file: bool,
    start_line: usize,
    directive: &str,
) {
    match directive {
        "body" => entry.body = Some(extract_clean_body(slice_str, is_file)),
        "links" => entry.links = Some(extract_links(slice_str, start_line)),
        _ => {}
    }
}

/// Applies digest, checkboxes, vulpea_meta, or scope for a given include directive.
fn enrich_meta_and_digest(
    entry: &mut JsonEntry,
    slice_str: &str,
    is_file: bool,
    doc: &OrgDocument,
    entry_idx: usize,
    ex_props: &[String],
    directive: &str,
) {
    match directive {
        "digest" => entry.digest = Some(compute_canonical_digest(slice_str, ex_props)),
        "checkboxes" => {
            if is_file {
                let content = std::fs::read_to_string(&doc.file).unwrap_or_default();
                entry.checkboxes = Some(org_tools_core::export_md::scan_file_node_checklists(
                    &org_tools_core::source::SourceFile::new(&doc.file, content),
                    doc,
                ));
            } else if let Some(e) = doc.entries.get(entry_idx) {
                entry.checkboxes = Some(e.checklist.clone());
            }
        }
        "vulpea_meta" => {
            if is_file {
                entry.vulpea_meta = Some(doc.vulpea_meta.clone());
            } else if let Some(e) = doc.entries.get(entry_idx) {
                entry.vulpea_meta = Some(e.vulpea_meta.clone());
            }
        }
        "scope" if entry.scope.is_none() => {
            entry.scope = Some(org_tools_core::scope::resolve_effective_scope(
                doc, entry_idx,
            ));
        }
        _ => {}
    }
}

/// Enriches a `JsonEntry` with optional body, links, digest, checkboxes, and vulpea-meta.
pub fn enrich_json_entry(
    entry: &mut JsonEntry,
    doc: &OrgDocument,
    entry_idx: usize,
    include: Option<&[String]>,
    exclude_props: Option<&[String]>,
) {
    let Some(inc) = include else {
        return;
    };
    let empty_exclude: &[String] = &[];
    let ex_props = exclude_props.unwrap_or(empty_exclude);

    let content = std::fs::read_to_string(&doc.file).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let is_file = entry_idx == FILE_NODE_ENTRY_INDEX;

    let slice_buf: String;
    let (slice_str, start_line): (&str, usize) = if is_file {
        (content.as_str(), 1)
    } else if let Some(e) = doc.entries.get(entry_idx) {
        let start = e.heading_line.saturating_sub(1).min(lines.len());
        let end = e.content_end_line.saturating_sub(1).min(lines.len());
        if start < end {
            slice_buf = lines[start..end].join("\n");
            (slice_buf.as_str(), e.heading_line)
        } else {
            ("", e.heading_line)
        }
    } else {
        ("", 1)
    };

    for item in inc {
        let lower = item.to_ascii_lowercase();
        enrich_body_and_links(entry, slice_str, is_file, start_line, &lower);
        enrich_meta_and_digest(entry, slice_str, is_file, doc, entry_idx, ex_props, &lower);
    }
}

/// Enriches a `JsonEntry` for `org query search` output with effective tags, filetags, and vulpea-meta.
pub fn enrich_search_item(
    m: &MatchedEntry<'_>,
    doc_refs: &[&OrgDocument],
    include: Option<&[String]>,
    exclude_properties: Option<&[String]>,
    rank: Option<f64>,
    snippet: Option<String>,
) -> JsonEntry {
    let mut item = build_json_entry(m, doc_refs);
    let eff = m.doc.effective_tags(m.entry_idx);
    item.effective_tags = if eff.is_empty() { None } else { Some(eff) };
    item.filetags = if m.doc.filetags.is_empty() {
        None
    } else {
        Some(m.doc.filetags.clone())
    };
    if m.is_file_node() {
        if !m.doc.vulpea_meta.is_empty() {
            item.vulpea_meta = Some(m.doc.vulpea_meta.clone());
        }
    } else if let Some(e) = m.doc.entries.get(m.entry_idx) {
        if !e.vulpea_meta.is_empty() {
            item.vulpea_meta = Some(e.vulpea_meta.clone());
        }
    }
    item.rank = rank;
    item.snippet = snippet;
    enrich_json_entry(&mut item, m.doc, m.entry_idx, include, exclude_properties);
    if item.scope.is_none() {
        item.scope = org_tools_core::scope::resolve_node_scope(m.doc, m.entry_idx);
    }
    item
}
