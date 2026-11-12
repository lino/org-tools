// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Initial capture buffer scaffolding and level-0 drawer mutation helpers.

use crate::capture::{
    format_aligned_properties_drawer, resolve_roam_link_or_raw, slugify_title, TemplateVars,
};

use super::types::FleetingCaptureSpec;

// region Title & Level-0 Properties Helpers

/// Extracted properties range tuple `(start_line_idx, end_line_idx, properties_vec)`.
type ExtractedProperties = (usize, usize, Vec<(String, String)>);

/// Strips leading case-insensitive `"Fleeting: "` or `"Refine Zettel: "` prefixes.
pub fn clean_fleeting_title(raw: &str) -> String {
    let t = raw.trim();
    for prefix in &["fleeting:", "refine zettel:"] {
        if t.len() >= prefix.len() && t[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return t[prefix.len()..].trim().to_string();
        }
    }
    t.to_string()
}

/// Parses existing level-0 properties from file lines.
fn extract_level0_properties(lines: &[&str]) -> Option<ExtractedProperties> {
    let mut start_idx = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            start_idx = Some(i);
            break;
        }
        break;
    }
    let start = start_idx?;
    let mut end = start;
    let mut props = Vec::new();
    for (i, &line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            end = i;
            break;
        }
        if let Some(rest) = trimmed.strip_prefix(':') {
            if let Some((k, v)) = rest.split_once(':') {
                props.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    Some((start, end, props))
}

/// Rewrites or prepends level-0 `:PROPERTIES:` drawer with updated `pairs`.
pub fn update_level0_properties(content: &str, pairs: &[(&str, &str)]) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut props: Vec<(String, String)> = Vec::new();
    let mut replace_range = None;

    if let Some((start, end, existing)) = extract_level0_properties(&lines) {
        props = existing;
        replace_range = Some((start, end));
    }

    for &(k, v) in pairs {
        if let Some(pos) = props.iter().position(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            props[pos] = (k.to_string(), v.to_string());
        } else {
            props.push((k.to_string(), v.to_string()));
        }
    }

    let drawer = format_aligned_properties_drawer(&props);
    if let Some((start, end)) = replace_range {
        let mut out = String::new();
        for &line in &lines[..start] {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&drawer);
        for &line in &lines[end + 1..] {
            out.push_str(line);
            out.push('\n');
        }
        out
    } else {
        format!("{drawer}\n{content}")
    }
}

// endregion

// region Fleeting Capture Scaffold Builder

/// Assembles the initial Org buffer for `org fleeting capture`.
pub fn build_fleeting_capture_file(spec: &FleetingCaptureSpec<'_>) -> (String, String) {
    let slug = slugify_title(spec.title);
    let vars = TemplateVars::new(spec.note_id, spec.title, &slug, spec.vault_name, spec.dt);

    let mut props = vec![
        ("ID".to_string(), spec.note_id.to_string()),
        ("TYPE".to_string(), "fleeting".to_string()),
        ("STATUS".to_string(), "unrefined".to_string()),
        ("CAPTURED_AT".to_string(), vars.timestamp_inactive),
    ];

    let mut bullets = Vec::new();
    if let Some(m) = spec.medium {
        let resolved = resolve_roam_link_or_raw(spec.vault_name, spec.vault_path, m, spec.no_cache);
        props.push(("ATTACHED_MEDIUM".to_string(), resolved.clone()));
        bullets.push(format!("- Attached Medium: {resolved}"));
    }
    if let Some(loc) = spec.location {
        props.push(("MEDIUM_LOCATION".to_string(), loc.to_string()));
        bullets.push(format!("- Medium Location: {loc}"));
    }
    if let Some(z) = spec.zettel {
        let resolved = resolve_roam_link_or_raw(spec.vault_name, spec.vault_path, z, spec.no_cache);
        props.push(("ATTACHED_ZETTEL".to_string(), resolved.clone()));
        bullets.push(format!("- Attached Zettel: {resolved}"));
    }
    if let Some(src) = spec.source_file {
        props.push(("SOURCE_FILE".to_string(), src.to_string()));
        bullets.push(format!("- Source File: {src}"));
    }

    let drawer = format_aligned_properties_drawer(&props);
    let mut out = format!(
        "{drawer}#+title: {}\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\n",
        spec.title
    );
    if let Some(b) = spec.body.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("\n{}\n", b.trim()));
    }
    out.push_str("\n* Context & Source\n");
    for b in bullets {
        out.push_str(&format!("{b}\n"));
    }

    let fname = format!("{}-{slug}.org", vars.timestamp);
    (fname, out)
}

// endregion
