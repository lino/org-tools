// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org-roam` v2 GTD workflow primitives: dynamic `:agenda:` filetag synchronization
//! (`vulpea-para` parity), 4-stage owning project resolution, GTD filtering/ranking,
//! and format-agnostic `stdin` locator parsing.

mod builders;
mod filter;
mod stdin;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::document::OrgDocument;
use crate::files::canonicalize_clean;
use crate::roam::{lookup_prop_ci, split_roam_tokens};
use crate::source::SourceFile;

pub use builders::{
    build_gtd_inbox_entry_buffer, build_gtd_project_file_buffer, is_plain_org_path_target,
    is_same_org_file, parse_flexible_locator, resolve_flexible_in_document, GtdInboxEntrySpec,
};
pub use filter::{
    entry_contexts, entry_matches_next_filter, inherited_property, is_leaf_actionable_for_next,
    next_candidate_sort_key, parse_effort_minutes, NextGroupBy, NextQueryFilter,
};
pub use stdin::parse_piped_stdin_targets;

// region Constants & Types

/// Canonical base GTD filenames under `<roam_dir>/<gtd_subdir>/` that always carry `:agenda:`.
pub const BASE_GTD_FILENAMES: &[&str] = &[
    "inbox.org",
    "gtd.org",
    "projects.org",
    "habits.org",
    "calendar.org",
];

/// Owning GTD project metadata resolved for an entry via the 4-stage precedence chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProject {
    /// Human-readable project title or fallback category/file stem.
    pub name: String,
    /// Optional project `:ID:`, `:CUSTOM_ID:`, or `:PROJECT_ID:`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Additional searchable aliases and file stems for `--project` filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Whether this project was explicitly resolved via property, ancestor, or roam project file.
    pub is_explicit_project: bool,
}

impl ResolvedProject {
    /// Returns `true` when `query` matches this project's title, `:ID:`, alias, or file stem.
    pub fn matches_filter(&self, query: &str) -> bool {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return true;
        }
        let clean = trimmed.strip_prefix("id:").unwrap_or(trimmed).trim();
        let lower = clean.to_ascii_lowercase();
        if self.name.to_ascii_lowercase().contains(&lower) {
            return true;
        }
        if self
            .id
            .as_ref()
            .is_some_and(|id| id.to_ascii_lowercase().contains(&lower))
        {
            return true;
        }
        self.aliases
            .iter()
            .any(|a| a.to_ascii_lowercase().contains(&lower))
    }
}

// endregion

// region Dynamic :agenda: Filetag Synchronization

/// Returns `true` when `path` is one of [`BASE_GTD_FILENAMES`] under `<gtd_subdir>/`.
pub fn is_base_gtd_file(path: &Path, gtd_subdir: &str, roam_dir: Option<&Path>) -> bool {
    let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if !BASE_GTD_FILENAMES
        .iter()
        .any(|b| b.eq_ignore_ascii_case(fname))
    {
        return false;
    }
    let clean_sub = gtd_subdir.trim_matches(['/', '\\']);
    if clean_sub.is_empty() {
        return true;
    }
    if let Some(vault) = roam_dir {
        let expected = vault.join(clean_sub).join(fname);
        if path == expected {
            return true;
        }
        if let (Ok(cp), Ok(ce)) = (canonicalize_clean(path), canonicalize_clean(&expected)) {
            if cp == ce {
                return true;
            }
        }
    }
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .is_some_and(|parent| parent.eq_ignore_ascii_case(clean_sub))
}

/// Returns `true` when `doc` has at least one active (non-done) TODO heading or is a base GTD file.
pub fn qualifies_for_agenda_filetag(
    path: &Path,
    doc: &OrgDocument,
    gtd_subdir: &str,
    roam_dir: Option<&Path>,
) -> bool {
    if is_base_gtd_file(path, gtd_subdir, roam_dir) {
        return true;
    }
    doc.entries.iter().any(|entry| {
        entry
            .keyword
            .as_deref()
            .is_some_and(|kw| !doc.todo_keywords.is_done(kw))
    })
}

/// Rewrites or removes a `#+filetags:` / `#+FILETAGS:` line with updated `tags`.
fn format_filetags_line(original_line: &str, tags: &[String]) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    let prefix = original_line
        .split_once(':')
        .map(|(k, _)| format!("{k}:"))
        .unwrap_or_else(|| "#+filetags:".to_string());
    Some(format!("{prefix} :{}:", tags.join(":")))
}

/// Finds the 0-based line index of the first `#+filetags:` line prior to the first heading.
fn find_preamble_filetags_line(lines: &[&str]) -> Option<usize> {
    for (idx, &line) in lines.iter().enumerate() {
        if line.starts_with('*') && line.bytes().find(|&b| b != b'*') == Some(b' ') {
            break;
        }
        let trimmed = line.trim();
        if trimmed.len() >= 11 && trimmed[..11].eq_ignore_ascii_case("#+filetags:") {
            return Some(idx);
        }
    }
    None
}

/// Computes the 0-based line insertion index for a new `#+filetags: :agenda:` line.
fn find_filetags_insert_index(lines: &[&str]) -> usize {
    let mut after_drawer_or_title = 0;
    let mut in_level0_drawer = false;
    for (idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if idx == 0 && trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            in_level0_drawer = true;
            continue;
        }
        if in_level0_drawer {
            after_drawer_or_title = idx + 1;
            if trimmed.eq_ignore_ascii_case(":END:") {
                in_level0_drawer = false;
            }
            continue;
        }
        if line.starts_with('*') {
            break;
        }
        if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("#+title:") {
            after_drawer_or_title = idx + 1;
        }
    }
    after_drawer_or_title
}

/// Synchronizes the `:agenda:` filetag in `content` for `path`, returning `Some(new_content)` when changed.
pub fn sync_roam_agenda_filetag(
    path: &Path,
    content: &str,
    gtd_subdir: &str,
    roam_dir: Option<&Path>,
) -> Option<String> {
    let src = SourceFile::new(path, content.to_string());
    let doc = OrgDocument::from_source(&src);
    let qualifies = qualifies_for_agenda_filetag(path, &doc, gtd_subdir, roam_dir);
    let has_agenda = doc
        .filetags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("agenda"));
    if qualifies == has_agenda {
        return None;
    }

    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    if let Some(ft_idx) = find_preamble_filetags_line(&line_refs) {
        let mut tags = doc.filetags.clone();
        if qualifies {
            tags.push("agenda".to_string());
        } else {
            tags.retain(|t| !t.eq_ignore_ascii_case("agenda"));
        }
        match format_filetags_line(&lines[ft_idx], &tags) {
            Some(replaced) => lines[ft_idx] = replaced,
            None => {
                lines.remove(ft_idx);
            }
        }
    } else if qualifies {
        let insert_at = find_filetags_insert_index(&line_refs);
        lines.insert(insert_at, "#+filetags: :agenda:".to_string());
    } else {
        return None;
    }

    let mut out = lines.join("\n");
    if content.ends_with('\n') || !out.is_empty() {
        out.push('\n');
    }
    (out != content).then_some(out)
}

/// Resolves `(gtd_subdir, roam_dir)` if `path` belongs to a configured roam vault or roam file.
pub fn resolve_roam_agenda_context(
    path: &Path,
    content: &str,
    cfg: &Config,
) -> Option<(String, Option<PathBuf>)> {
    let canon_file = canonicalize_clean(path).unwrap_or_else(|_| path.to_path_buf());
    for (_, vault_path) in cfg.roam_directories() {
        let canon_vault = canonicalize_clean(&vault_path).unwrap_or_else(|_| vault_path.clone());
        if canon_file.starts_with(&canon_vault) || path.starts_with(&vault_path) {
            let v_cfg = Config::load_for_dir(&vault_path).unwrap_or_else(|_| cfg.clone());
            return Some((v_cfg.roam.gtd_subdir, Some(vault_path)));
        }
    }
    let src = SourceFile::new(path, content.to_string());
    let doc = OrgDocument::from_source(&src);
    let is_roam_like = lookup_prop_ci(&doc.file_properties, "TYPE").is_some()
        || doc
            .filetags
            .iter()
            .any(|t| t.eq_ignore_ascii_case("agenda") || t.eq_ignore_ascii_case("project"))
        || is_base_gtd_file(path, &cfg.roam.gtd_subdir, None);
    is_roam_like.then(|| (cfg.roam.gtd_subdir.clone(), None))
}

/// Automatically applies [`sync_roam_agenda_filetag`] when `path` is in a roam vault or carries roam metadata.
pub fn maybe_sync_agenda_filetag_for_mutation(
    path: &Path,
    content: &str,
    cfg: &Config,
) -> Option<String> {
    let (gtd_subdir, roam_dir) = resolve_roam_agenda_context(path, content, cfg)?;
    sync_roam_agenda_filetag(path, content, &gtd_subdir, roam_dir.as_deref())
}

// endregion

// region 4-Stage Owning Project Resolution

/// Parses `[[id:<uuid>][<title>]]` link syntax or returns `(raw, None)`.
fn parse_project_link_or_value(raw: &str) -> (String, Option<String>) {
    let trimmed = raw.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[id:")
        .and_then(|s| s.strip_suffix("]]"))
    {
        if let Some((id_part, title_part)) = inner.split_once("][") {
            return (
                title_part.trim().to_string(),
                Some(id_part.trim().to_string()),
            );
        }
        return (inner.trim().to_string(), Some(inner.trim().to_string()));
    }
    (trimmed.to_string(), None)
}

/// Stage 1: Checks `:PROJECT:` or `:PROJECT_ID:` on `entry_idx` or an ancestor heading.
fn resolve_project_from_properties(doc: &OrgDocument, entry_idx: usize) -> Option<ResolvedProject> {
    let mut cur = Some(entry_idx);
    while let Some(idx) = cur {
        let props = &doc.entries[idx].properties;
        let proj = lookup_prop_ci(props, "PROJECT")
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let proj_id = lookup_prop_ci(props, "PROJECT_ID")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(raw_proj) = proj {
            let (name, link_id) = parse_project_link_or_value(raw_proj);
            return Some(ResolvedProject {
                name,
                id: proj_id.or(link_id),
                aliases: Vec::new(),
                is_explicit_project: true,
            });
        }
        if let Some(pid) = proj_id {
            return Some(ResolvedProject {
                name: pid.clone(),
                id: Some(pid),
                aliases: Vec::new(),
                is_explicit_project: true,
            });
        }
        cur = doc.entries[idx].parent;
    }
    None
}

/// Stage 2: Checks nearest ancestor heading with `PROJECT` keyword, `:project:` tag, `:TYPE: project`, or level-1 in `projects.org`.
fn resolve_project_from_ancestor(doc: &OrgDocument, entry_idx: usize) -> Option<ResolvedProject> {
    let in_projects_file = doc
        .file
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|f| f.eq_ignore_ascii_case("projects.org"));
    let mut cur = doc.entries[entry_idx].parent;
    while let Some(idx) = cur {
        let anc = &doc.entries[idx];
        let is_proj_kw = anc
            .keyword
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case("PROJECT"));
        let is_proj_tag = anc.tags.iter().any(|t| t.eq_ignore_ascii_case("project"));
        let is_proj_type = lookup_prop_ci(&anc.properties, "TYPE")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("project"));
        if is_proj_kw || is_proj_tag || is_proj_type || (in_projects_file && anc.level == 1) {
            let id = lookup_prop_ci(&anc.properties, "ID")
                .or_else(|| lookup_prop_ci(&anc.properties, "CUSTOM_ID"))
                .map(|s| s.trim().to_string());
            let aliases = lookup_prop_ci(&anc.properties, "ROAM_ALIASES")
                .map(split_roam_tokens)
                .unwrap_or_default();
            return Some(ResolvedProject {
                name: anc.title.clone(),
                id,
                aliases,
                is_explicit_project: true,
            });
        }
        cur = anc.parent;
    }
    None
}

/// Stage 3: Checks whether `doc` is an enclosing `org-roam` project file node.
fn resolve_project_from_file_node(doc: &OrgDocument, gtd_subdir: &str) -> Option<ResolvedProject> {
    let is_type_proj = lookup_prop_ci(&doc.file_properties, "TYPE")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("project"));
    let is_tag_proj = doc
        .filetags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("project"));
    let clean_sub = gtd_subdir.trim_matches(['/', '\\']);
    let in_projects_dir = doc
        .file
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .is_some_and(|d| d.eq_ignore_ascii_case("projects") || d.eq_ignore_ascii_case(clean_sub));
    if !(is_type_proj || is_tag_proj || in_projects_dir) {
        return None;
    }
    let stem = doc
        .file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();
    let name = lookup_prop_ci(&doc.file_keywords, "TITLE")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stem.clone());
    let id = doc.file_id().map(str::to_string);
    let mut aliases = lookup_prop_ci(&doc.file_properties, "ROAM_ALIASES")
        .map(split_roam_tokens)
        .unwrap_or_default();
    if !aliases.iter().any(|a| a.eq_ignore_ascii_case(&stem)) {
        aliases.push(stem);
    }
    Some(ResolvedProject {
        name,
        id,
        aliases,
        is_explicit_project: true,
    })
}

/// Resolves the owning project for `entry_idx` in `doc` via the 4-stage GTD precedence chain.
pub fn resolve_entry_project(
    doc: &OrgDocument,
    entry_idx: usize,
    gtd_subdir: &str,
) -> ResolvedProject {
    if let Some(proj) = resolve_project_from_properties(doc, entry_idx) {
        return proj;
    }
    if let Some(proj) = resolve_project_from_ancestor(doc, entry_idx) {
        return proj;
    }
    if let Some(proj) = resolve_project_from_file_node(doc, gtd_subdir) {
        return proj;
    }
    let stem = doc
        .file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tasks")
        .to_string();
    let category = inherited_property(doc, entry_idx, "CATEGORY")
        .map(str::to_string)
        .or_else(|| {
            lookup_prop_ci(&doc.file_keywords, "CATEGORY")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| stem.clone());
    ResolvedProject {
        name: category,
        id: doc.file_id().map(str::to_string),
        aliases: vec![stem],
        is_explicit_project: false,
    }
}

// endregion
