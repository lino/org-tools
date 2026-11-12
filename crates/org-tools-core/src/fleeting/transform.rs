// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Transformation primitives: permanent Zettel creation, GTD task conversion,
//! and source fleeting note lifecycle updates.

use std::fs;
use std::path::{Path, PathBuf};

use crate::capture::{format_aligned_properties_drawer, resolve_roam_link_or_raw, slugify_title};
use crate::document::OrgDocument;
use crate::gtd_roam::{build_gtd_inbox_entry_buffer, GtdInboxEntrySpec};
use crate::property::set_properties;
use crate::source::SourceFile;

use super::build::{clean_fleeting_title, update_level0_properties};
use super::scan::scan_fleeting_items;
use super::types::{
    FleetingItem, FleetingKind, FleetingStatusFilter, FleetingTaskSpec, PermanentZettelSpec,
};

// region Target Resolution & Zettel Transformation

/// Resolves a target string into a matching [`FleetingItem`].
pub fn resolve_fleeting_target(roots: &[PathBuf], target: &str) -> Result<FleetingItem, String> {
    let trimmed = target.trim();
    let clean_id = trimmed.strip_prefix("id:").unwrap_or(trimmed).trim();
    let items = scan_fleeting_items(roots, FleetingStatusFilter::All);

    if let Some(found) = items.iter().find(|i| {
        i.id.as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(clean_id))
    }) {
        return Ok(found.clone());
    }
    if let Some(found) = items.iter().find(|i| i.locator == trimmed) {
        return Ok(found.clone());
    }
    if let Some(found) = items.iter().find(|i| i.file == trimmed) {
        return Ok(found.clone());
    }
    if let Some(found) = items.iter().find(|i| i.title.eq_ignore_ascii_case(trimmed)) {
        return Ok(found.clone());
    }
    Err(format!("fleeting note not found: {trimmed}"))
}

/// Builds the permanent Zettel buffer and returns `(filename, content)`.
pub fn build_permanent_zettel_buffer(spec: &PermanentZettelSpec<'_>) -> (String, String) {
    let title = spec.title_override.unwrap_or(spec.item.title.as_str());
    let clean_title = clean_fleeting_title(title);
    let slug = spec
        .slug_override
        .map(str::to_string)
        .unwrap_or_else(|| slugify_title(&clean_title));

    let mut props = vec![
        ("ID".to_string(), spec.zettel_id.to_string()),
        ("TYPE".to_string(), "concept".to_string()),
    ];
    if let Some(ref m) = spec.item.attached_medium {
        props.push(("SOURCE_MEDIUM".to_string(), m.clone()));
    }
    if let Some(ref loc) = spec.item.medium_location {
        props.push(("MEDIUM_LOCATION".to_string(), loc.clone()));
    }

    let parent_raw = spec
        .parent_override
        .or(spec.item.attached_zettel.as_deref());
    let mut related_bullets = Vec::new();
    if let Some(p) = parent_raw {
        let resolved = resolve_roam_link_or_raw(spec.vault_name, spec.vault_path, p, spec.no_cache);
        props.push(("PARENT_ZETTEL".to_string(), resolved.clone()));
        if resolved.starts_with("[[id:") {
            related_bullets.push(format!("- {resolved}"));
        }
    }
    for l in spec.links {
        let res = resolve_roam_link_or_raw(spec.vault_name, spec.vault_path, l, spec.no_cache);
        related_bullets.push(format!("- {res}"));
    }

    let drawer = format_aligned_properties_drawer(&props);
    let body_text = spec.body_override.unwrap_or(&spec.item.raw_thoughts);
    let mut out = format!("{drawer}#+title: {clean_title}\n\n* Thesis\n\n* Core Concept\n");
    if !body_text.trim().is_empty() {
        out.push_str(&format!("\n{}\n", body_text.trim()));
    }
    out.push_str("\n* Context & Significance\n\n* Related Concepts & Links\n");
    for b in related_bullets {
        out.push_str(&format!("{b}\n"));
    }
    out.push_str("\n* Open Questions & Folgezettel\n");

    (format!("{slug}.org"), out)
}

/// Updates the source fleeting note after being reformed into a permanent Zettel.
pub fn update_source_after_zettel_reform(
    item: &FleetingItem,
    zettel_id: &str,
    zettel_title: &str,
    reformed_ts: &str,
) -> Result<String, String> {
    let source_path = Path::new(&item.file);
    let content = fs::read_to_string(source_path)
        .map_err(|e| format!("error reading {}: {e}", source_path.display()))?;
    let zettel_link = format!("[[id:{zettel_id}][{zettel_title}]]");

    match item.kind {
        FleetingKind::File => {
            let mut updated = update_level0_properties(
                &content,
                &[
                    ("STATUS", "reformed"),
                    ("PERMANENT_NOTE", &zettel_link),
                    ("REFORMED_AT", reformed_ts),
                ],
            );
            let section_header = "* Reformed Permanent Note";
            let section_block = format!("\n{section_header}\n\n- {zettel_link}\n");
            if let Some(pos) = updated.find(section_header) {
                let rest = &updated[pos..];
                if let Some(next_heading) = rest[section_header.len()..].find("\n* ") {
                    updated.replace_range(
                        pos..pos + section_header.len() + next_heading,
                        &section_block,
                    );
                } else {
                    updated.truncate(pos);
                    updated.push_str(&section_block);
                }
            } else {
                if !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push_str(&section_block);
            }
            Ok(updated)
        }
        FleetingKind::Heading => update_heading_after_mutation(
            &content,
            source_path,
            item.line,
            "DONE",
            &[
                ("STATUS", "reformed"),
                ("PERMANENT_NOTE", &zettel_link),
                ("REFORMED_AT", reformed_ts),
            ],
        ),
    }
}

// endregion

// region GTD Task Conversion & Discard

/// Appends a new `* TODO` task derived from `spec.item` to the target GTD buffer.
pub fn build_gtd_task_for_fleeting(spec: &FleetingTaskSpec<'_>) -> Result<(String, usize), String> {
    let title = spec.task_title.unwrap_or(&spec.item.title);
    let clean_title = clean_fleeting_title(title);
    let gtd_spec = GtdInboxEntrySpec {
        title: &clean_title,
        project: None,
        body: if spec.item.raw_thoughts.is_empty() {
            None
        } else {
            Some(spec.item.raw_thoughts.clone())
        },
        context: spec.context,
        effort: spec.effort,
        energy: None,
        workmode: None,
        scheduled: spec.scheduled,
        deadline: spec.deadline,
        vault_name: spec.vault_name,
        vault_dir: spec.vault_dir,
        entry_id: spec.task_id,
        no_cache: spec.no_cache,
    };
    let (combined, line_no) =
        build_gtd_inbox_entry_buffer(spec.inbox_content, spec.inbox_path, &gtd_spec)?;

    let fleeting_ref = spec
        .item
        .id
        .as_ref()
        .map(|uuid| format!("[[id:{uuid}][{}]]", spec.item.title))
        .unwrap_or_else(|| spec.item.title.clone());

    let source = SourceFile::new(spec.inbox_path, combined);
    let doc = OrgDocument::from_source(&source);
    let entry_idx = doc
        .entries
        .iter()
        .position(|e| e.heading_line == line_no)
        .ok_or_else(|| "failed locating added inbox task".to_string())?;

    let fin = set_properties(
        &source,
        &doc,
        entry_idx,
        &[("SOURCE_FLEETING".to_string(), fleeting_ref)],
    )?;
    Ok((fin.content, line_no))
}

/// Updates the source fleeting note after being converted into a GTD task.
pub fn update_source_after_task_convert(
    item: &FleetingItem,
    task_id: &str,
    task_title: &str,
    converted_ts: &str,
) -> Result<String, String> {
    let source_path = Path::new(&item.file);
    let content = fs::read_to_string(source_path)
        .map_err(|e| format!("error reading {}: {e}", source_path.display()))?;
    let task_link = format!("[[id:{task_id}][{task_title}]]");

    match item.kind {
        FleetingKind::File => Ok(update_level0_properties(
            &content,
            &[
                ("STATUS", "converted-to-task"),
                ("TASK_NOTE", &task_link),
                ("CONVERTED_AT", converted_ts),
            ],
        )),
        FleetingKind::Heading => update_heading_after_mutation(
            &content,
            source_path,
            item.line,
            "DONE",
            &[
                ("STATUS", "converted-to-task"),
                ("TASK_NOTE", &task_link),
                ("CONVERTED_AT", converted_ts),
            ],
        ),
    }
}

/// Updates the source fleeting note marking it discarded.
pub fn update_source_after_discard(
    item: &FleetingItem,
    discarded_ts: &str,
) -> Result<String, String> {
    let source_path = Path::new(&item.file);
    let content = fs::read_to_string(source_path)
        .map_err(|e| format!("error reading {}: {e}", source_path.display()))?;

    match item.kind {
        FleetingKind::File => Ok(update_level0_properties(
            &content,
            &[("STATUS", "discarded"), ("DISCARDED_AT", discarded_ts)],
        )),
        FleetingKind::Heading => update_heading_after_mutation(
            &content,
            source_path,
            item.line,
            "CANCELLED",
            &[("STATUS", "discarded"), ("DISCARDED_AT", discarded_ts)],
        ),
    }
}

/// Rewrites a heading's TODO keyword and updates its property drawer.
fn update_heading_after_mutation(
    content: &str,
    path: &Path,
    heading_line: usize,
    new_kw: &str,
    props: &[(&str, &str)],
) -> Result<String, String> {
    let source = SourceFile::new(path, content.to_string());
    let doc = OrgDocument::from_source(&source);
    let entry_idx = doc
        .entries
        .iter()
        .position(|e| e.heading_line == heading_line)
        .ok_or_else(|| format!("heading at line {heading_line} not found"))?;

    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    if heading_line <= lines.len() {
        let orig = &lines[heading_line - 1];
        let stars = orig.chars().take_while(|&c| c == '*').count();
        let after_stars = orig[stars..].trim_start();
        let title_without_kw = if let Some(kw) = &doc.entries[entry_idx].keyword {
            after_stars
                .strip_prefix(kw)
                .unwrap_or(after_stars)
                .trim_start()
        } else {
            after_stars
        };
        lines[heading_line - 1] = format!("{} {} {}", "*".repeat(stars), new_kw, title_without_kw);
    }
    let intermediate = lines.join("\n") + "\n";
    let intermediate_source = SourceFile::new(path, intermediate);
    let intermediate_doc = OrgDocument::from_source(&intermediate_source);

    let prop_pairs: Vec<(String, String)> = props
        .iter()
        .map(|&(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let res = set_properties(
        &intermediate_source,
        &intermediate_doc,
        entry_idx,
        &prop_pairs,
    )?;
    Ok(res.content)
}

// endregion
