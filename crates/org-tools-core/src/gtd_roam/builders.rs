// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Flexible locator resolution helpers and GTD inbox/project Org buffer builders.

use std::path::{Path, PathBuf};

use crate::capture::{
    current_capture_datetime, format_aligned_properties_drawer, mint_capture_id,
    resolve_roam_link_or_raw, slugify_title, TemplateVars,
};
use crate::document::OrgDocument;
use crate::entry::{add_entry, NewEntryOpts};
use crate::files::canonicalize_clean;
use crate::locator::{resolve_in_document, OrgLocator, ResolvedEntry};
use crate::property::{finalize_added_todo, resolve_tag_column, FinalizeTodoOptions};
use crate::source::SourceFile;

// region Flexible Locator Helpers

/// Parses a fileless shorthand (`custom-id:...`, `#...`, `olp:...`, `*Heading`) against `file`.
fn parse_default_file_shorthand(trimmed: &str, file: &Path) -> Option<Result<OrgLocator, String>> {
    if let Some(cid) = trimmed
        .strip_prefix("custom-id:")
        .or_else(|| trimmed.strip_prefix('#'))
    {
        return Some(
            OrgLocator::parse(&format!("{}::#{}", file.display(), cid.trim()))
                .map_err(|e| e.to_string()),
        );
    }
    if let Some(olp) = trimmed
        .strip_prefix("olp:")
        .or_else(|| trimmed.strip_prefix('*'))
    {
        let cleaned = olp.trim_start_matches('*').trim();
        return Some(
            OrgLocator::parse(&format!("{}::*/{cleaned}", file.display()))
                .map_err(|e| e.to_string()),
        );
    }
    None
}

/// Normalizes shorthand locator strings (`file.org::*Heading`, `file.org::olp:A/B`) into [`OrgLocator`].
pub fn parse_flexible_locator(
    raw: &str,
    default_file: Option<&Path>,
) -> Result<OrgLocator, String> {
    let trimmed = raw.trim();
    if let Ok(loc) = OrgLocator::parse(trimmed) {
        return Ok(loc);
    }
    if let Some((file_part, search_part)) = trimmed.split_once("::") {
        let file = if file_part.trim().is_empty() {
            default_file
                .ok_or_else(|| format!("missing file path in locator '{trimmed}'"))?
                .to_path_buf()
        } else {
            PathBuf::from(file_part.trim())
        };
        let search = search_part.trim();
        let cleaned = search
            .strip_prefix("olp:")
            .or_else(|| search.strip_prefix("*/"))
            .unwrap_or_else(|| search.trim_start_matches('*').trim_start());
        if !cleaned.is_empty() && !cleaned.starts_with('#') {
            return OrgLocator::parse(&format!("{}::*/{cleaned}", file.display()))
                .map_err(|e| e.to_string());
        }
    }
    if let Some(file) = default_file {
        if let Some(res) = parse_default_file_shorthand(trimmed, file) {
            return res;
        }
    }
    Err(format!("invalid locator format: {trimmed}"))
}

/// Resolves `locator` in `doc`, falling back to matching a single-segment `*Heading` at any depth.
pub fn resolve_flexible_in_document(
    locator: &OrgLocator,
    doc: &OrgDocument,
) -> Result<ResolvedEntry, String> {
    if let Ok(resolved) = resolve_in_document(locator, doc) {
        return Ok(resolved);
    }
    if let OrgLocator::OutlinePath { file, path } = locator {
        if path.len() == 1 {
            let target_lower = path[0].to_lowercase();
            if let Some((idx, entry)) = doc
                .entries
                .iter()
                .enumerate()
                .find(|(_, e)| e.title.to_lowercase() == target_lower)
            {
                return Ok(ResolvedEntry {
                    file: file.clone(),
                    line: entry.heading_line,
                    heading_text: entry.title.clone(),
                    level: entry.level,
                    entry_index: idx,
                });
            }
        }
    }
    Err(format!("entry not found: {locator}"))
}

/// Returns `true` when `raw` is a plain `.org` file path rather than a heading locator.
pub fn is_plain_org_path_target(raw: &str) -> bool {
    let t = raw.trim();
    !t.contains("::")
        && !["id:", "custom-id:", "olp:", "#", "*"]
            .iter()
            .any(|p| t.starts_with(p))
        && Path::new(t)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("org"))
}

/// Returns `true` when `path_a` and `path_b` resolve to the same canonical file on disk.
pub fn is_same_org_file(path_a: &Path, path_b: &Path) -> bool {
    path_a == path_b
        || matches!(
            (canonicalize_clean(path_a), canonicalize_clean(path_b)),
            (Ok(ca), Ok(cb)) if ca == cb
        )
}

// endregion

// region GTD Inbox & Project Buffer Builders

/// Parameters for appending a new `* TODO` entry to `<roam_dir>/<gtd_subdir>/inbox.org`.
pub struct GtdInboxEntrySpec<'a> {
    /// Entry heading title.
    pub title: &'a str,
    /// Optional owning project selector.
    pub project: Option<&'a str>,
    /// Optional body text.
    pub body: Option<String>,
    /// Optional GTD context (`@computer` or `computer`).
    pub context: Option<&'a str>,
    /// Optional `:EFFORT:` value.
    pub effort: Option<&'a str>,
    /// Optional `:ENERGY:` value.
    pub energy: Option<&'a str>,
    /// Optional `:WORKMODE:` value.
    pub workmode: Option<&'a str>,
    /// Optional `SCHEDULED:` date string.
    pub scheduled: Option<&'a str>,
    /// Optional `DEADLINE:` date string.
    pub deadline: Option<&'a str>,
    /// Vault name.
    pub vault_name: &'a str,
    /// Vault directory path.
    pub vault_dir: &'a Path,
    /// Minted entry `:ID:`.
    pub entry_id: &'a str,
    /// Whether SQLite cache lookups are disabled.
    pub no_cache: bool,
}

/// Appends a new `* TODO` entry to `existing_inbox` and returns `(content, heading_line)`.
pub fn build_gtd_inbox_entry_buffer(
    existing_inbox: &str,
    inbox_path: &Path,
    spec: &GtdInboxEntrySpec<'_>,
) -> Result<(String, usize), String> {
    let vars = TemplateVars::new(
        spec.entry_id,
        spec.title.trim(),
        &slugify_title(spec.title),
        spec.vault_name,
        current_capture_datetime(),
    );
    let mut tags = Vec::new();
    let mut props: Vec<(String, String)> = vec![
        ("ID".to_string(), spec.entry_id.to_string()),
        ("CAPTURED_AT".to_string(), vars.timestamp_inactive),
    ];
    if let Some(proj) = spec.project {
        props.push((
            "PROJECT".to_string(),
            resolve_roam_link_or_raw(spec.vault_name, spec.vault_dir, proj, spec.no_cache),
        ));
    }
    if let Some(ctx) = spec.context {
        let clean = ctx.trim().trim_start_matches('@');
        if !clean.is_empty() {
            tags.push(format!("@{clean}"));
            props.push(("CONTEXT".to_string(), clean.to_string()));
        }
    }
    for (k, v) in [
        ("EFFORT", spec.effort),
        ("ENERGY", spec.energy),
        ("WORKMODE", spec.workmode),
    ] {
        if let Some(val) = v.map(str::trim).filter(|s| !s.is_empty()) {
            props.push((k.to_string(), val.to_string()));
        }
    }
    let clean_date = |s: &str| s.trim().trim_matches(['<', '>']).to_string();
    let source = SourceFile::new(inbox_path, existing_inbox.to_string());
    let doc = OrgDocument::from_source(&source);
    let opts = NewEntryOpts {
        title: spec.title.trim().to_string(),
        level: 1,
        keyword: Some("TODO".to_string()),
        priority: None,
        tags,
        scheduled: spec.scheduled.map(clean_date),
        deadline: spec.deadline.map(clean_date),
    };
    let added = add_entry(&source, &doc, None, &opts);
    let fin_opts = FinalizeTodoOptions {
        properties: props,
        body: spec.body.clone(),
        tag_column: resolve_tag_column(None),
    };
    let fin = finalize_added_todo(false, &added.content, inbox_path, None, &fin_opts)?;
    Ok((fin.content, fin.heading_line))
}

/// Builds the level-0 project file buffer for `org roam gtd project`.
pub fn build_gtd_project_file_buffer(
    title: &str,
    category: Option<&str>,
    first_task: Option<&str>,
    proj_path: &Path,
    proj_id: &str,
) -> Result<String, String> {
    let mut props = vec![
        ("ID".to_string(), proj_id.to_string()),
        ("TYPE".to_string(), "project".to_string()),
        ("STATUS".to_string(), "active".to_string()),
    ];
    if let Some(cat) = category {
        props.push(("CATEGORY".to_string(), cat.trim().to_string()));
    }
    let mut out = format_aligned_properties_drawer(&props);
    out.push_str(&format!(
        "#+title: {}\n#+filetags: :project:agenda:\n\n* Description\n\n* Tasks\n",
        title.trim()
    ));
    if let Some(first) = first_task {
        let src = SourceFile::new(proj_path, out);
        let doc = OrgDocument::from_source(&src);
        let opts = NewEntryOpts {
            title: first.trim().to_string(),
            level: 2,
            keyword: Some("NEXT".to_string()),
            priority: None,
            tags: Vec::new(),
            scheduled: None,
            deadline: None,
        };
        let added = add_entry(&src, &doc, Some(1), &opts);
        let fin_opts = FinalizeTodoOptions {
            properties: vec![("ID".to_string(), mint_capture_id())],
            body: None,
            tag_column: resolve_tag_column(None),
        };
        out = finalize_added_todo(false, &added.content, proj_path, Some(1), &fin_opts)?.content;
    }
    Ok(out)
}

// endregion
