// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Roam-aware GTD document discovery, candidate ranking, and `human` / `json` / `--pick` renderers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::gtd_roam::{
    entry_contexts, entry_matches_next_filter, inherited_property, is_base_gtd_file,
    is_leaf_actionable_for_next, next_candidate_sort_key, resolve_entry_project, NextGroupBy,
    NextQueryFilter, ResolvedProject,
};
use org_tools_core::locator::locator_for_entry;
use org_tools_core::source::SourceFile;

use super::output::{
    append_named_group, build_json_entry, format_entry_line, format_ts, JsonEntry, MatchedEntry,
};

// region Roam-Aware Document Discovery

/// Loaded [`OrgDocument`] annotated with its optional `roam_dir` vault name and `gtd_subdir`.
pub(crate) struct LoadedGtdDoc {
    /// Parsed Org document.
    pub(crate) doc: OrgDocument,
    /// Configured `org-roam` vault name, if resolved from a named vault.
    pub(crate) roam_dir: Option<String>,
    /// Vault-specific `gtd_subdir` (default `"gtd"`).
    pub(crate) gtd_subdir: String,
    /// Effective deadline warning window in days.
    pub(crate) warning_days: u32,
}

/// Resolves `(vault_name_opt, scan_path, gtd_subdir, warning_days)` roots from CLI args and config.
#[allow(clippy::type_complexity)]
fn resolve_gtd_roots(
    paths: &[PathBuf],
    roam_dir: &[String],
) -> Result<Vec<(Option<String>, PathBuf, String, u32)>, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = Config::load_for_dir(&cwd).map_err(|e| e.to_string())?;

    if !roam_dir.is_empty() {
        let mut out = Vec::with_capacity(roam_dir.len());
        for sel in roam_dir {
            let Some((opt_name, path)) = cfg.resolve_roam_dir(sel) else {
                return Err(format!("unknown roam directory: {sel}"));
            };
            if !path.exists() {
                return Err(format!("roam directory not found: {}", path.display()));
            }
            let v_cfg = Config::load_for_dir(&path).unwrap_or_else(|_| cfg.clone());
            out.push((
                Some(opt_name.unwrap_or_else(|| sel.clone())),
                path,
                v_cfg.roam.gtd_subdir,
                v_cfg.agenda.deadline_warning_days,
            ));
        }
        return Ok(out);
    }
    if paths.is_empty() {
        let configured = cfg.roam_directories();
        if !configured.is_empty() {
            let existing: Vec<_> = configured
                .into_iter()
                .filter(|(_, p)| p.exists())
                .map(|(name, p)| {
                    let v_cfg = Config::load_for_dir(&p).unwrap_or_else(|_| cfg.clone());
                    (
                        Some(name),
                        p,
                        v_cfg.roam.gtd_subdir,
                        v_cfg.agenda.deadline_warning_days,
                    )
                })
                .collect();
            if !existing.is_empty() {
                return Ok(existing);
            }
        }
        return Ok(vec![(
            None,
            PathBuf::from("."),
            cfg.roam.gtd_subdir,
            cfg.agenda.deadline_warning_days,
        )]);
    }
    Ok(paths
        .iter()
        .cloned()
        .map(|p| {
            (
                None,
                p,
                cfg.roam.gtd_subdir.clone(),
                cfg.agenda.deadline_warning_days,
            )
        })
        .collect())
}

/// Resolves `.org` file paths across configured roam vaults or explicit `paths` (shared with `agenda`).
pub(crate) fn resolve_gtd_scan_paths(
    paths: &[PathBuf],
    roam_dir: &[String],
    agenda_only: bool,
) -> Result<Vec<PathBuf>, String> {
    let roots = resolve_gtd_roots(paths, roam_dir)?;
    let mut files = Vec::new();
    for (_, root, gtd_subdir, _) in &roots {
        if !root.exists() {
            return Err(format!("path not found: {}", root.display()));
        }
        for file in collect_org_files(std::slice::from_ref(root)) {
            if agenda_only {
                let Ok(src) = SourceFile::from_path(&file) else {
                    continue;
                };
                let doc = OrgDocument::from_source(&src);
                let has_agenda = doc
                    .filetags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case("agenda"));
                if !has_agenda && !is_base_gtd_file(&file, gtd_subdir, Some(root.as_path())) {
                    continue;
                }
            }
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    Ok(files)
}

/// Loads [`LoadedGtdDoc`] records for GTD queries, honoring `--roam-dir` and `--agenda-only`.
pub(crate) fn load_gtd_docs(
    paths: &[PathBuf],
    roam_dir: &[String],
    agenda_only: bool,
) -> Result<Vec<LoadedGtdDoc>, String> {
    let roots = resolve_gtd_roots(paths, roam_dir)?;
    let mut loaded = Vec::new();
    for (vault_name, root, gtd_subdir, warning_days) in roots {
        for file in collect_org_files(std::slice::from_ref(&root)) {
            let Ok(src) = SourceFile::from_path(&file) else {
                continue;
            };
            let doc = OrgDocument::from_source(&src);
            if agenda_only {
                let has_agenda = doc
                    .filetags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case("agenda"));
                if !has_agenda && !is_base_gtd_file(&file, &gtd_subdir, Some(root.as_path())) {
                    continue;
                }
            }
            loaded.push(LoadedGtdDoc {
                doc,
                roam_dir: vault_name.clone(),
                gtd_subdir: gtd_subdir.clone(),
                warning_days,
            });
        }
    }
    if loaded.is_empty() {
        return Err("no .org files found".to_string());
    }
    Ok(loaded)
}

// endregion

// region Candidate Evaluation & Renderers

/// A ranked next-action candidate paired with its resolved project and vault metadata.
pub(crate) struct NextCandidate<'a> {
    /// Enclosing document wrapper.
    pub(crate) loaded: &'a LoadedGtdDoc,
    /// Index in `loaded.doc.entries`.
    pub(crate) entry_idx: usize,
    /// Resolved owning GTD project.
    pub(crate) project: ResolvedProject,
}

/// Collects and deterministically ranks [`NextCandidate`] items across `loaded`.
pub(crate) fn collect_ranked_next_candidates<'a>(
    loaded: &'a [LoadedGtdDoc],
    doc_refs: &[&'a OrgDocument],
    filter: &NextQueryFilter,
    today: (u16, u8, u8),
    limit: Option<usize>,
) -> Vec<NextCandidate<'a>> {
    let mut candidates = Vec::new();
    for item in loaded {
        for idx in 0..item.doc.entries.len() {
            if !is_leaf_actionable_for_next(
                &item.doc,
                idx,
                doc_refs,
                today,
                filter.next_only,
                filter.include_future,
            ) {
                continue;
            }
            let project = resolve_entry_project(&item.doc, idx, &item.gtd_subdir);
            if entry_matches_next_filter(&item.doc, idx, &project, filter, today) {
                candidates.push(NextCandidate {
                    loaded: item,
                    entry_idx: idx,
                    project,
                });
            }
        }
    }
    candidates.sort_by(|a, b| {
        next_candidate_sort_key(&a.loaded.doc, a.entry_idx, today, a.loaded.warning_days).cmp(
            &next_candidate_sort_key(&b.loaded.doc, b.entry_idx, today, b.loaded.warning_days),
        )
    });
    if let Some(n) = limit {
        candidates.truncate(n);
    }
    candidates
}

/// Renders `--pick` recommendations in rich human-readable format.
pub(crate) fn render_pick_human(candidates: &[NextCandidate<'_>]) -> String {
    let mut out = String::new();
    for c in candidates {
        let doc = &c.loaded.doc;
        let entry = &doc.entries[c.entry_idx];
        let kw = entry.keyword.as_deref().unwrap_or("TODO");
        let pri = entry.priority.map_or(String::new(), |p| format!("[#{p}] "));
        let ctxs = entry_contexts(doc, c.entry_idx);
        let ctx_str = if ctxs.is_empty() {
            "-".to_string()
        } else {
            ctxs.join(", ")
        };
        let eff = inherited_property(doc, c.entry_idx, "EFFORT").unwrap_or("-");
        let energy = inherited_property(doc, c.entry_idx, "ENERGY").unwrap_or("-");
        let wm = inherited_property(doc, c.entry_idx, "WORKMODE").unwrap_or("-");
        let sched = entry
            .planning
            .scheduled
            .as_ref()
            .map_or_else(|| "-".to_string(), format_ts);
        let dead = entry
            .planning
            .deadline
            .as_ref()
            .map_or_else(|| "-".to_string(), format_ts);
        let loc = locator_for_entry(doc, c.entry_idx);
        out.push_str("Recommended Next Action:\n");
        out.push_str(&format!("  Title:     {kw} {pri}{}\n", entry.title));
        out.push_str(&format!("  Project:   {}\n", c.project.name));
        if let Some(ref v) = c.loaded.roam_dir {
            out.push_str(&format!("  Vault:     {v}\n"));
        }
        out.push_str(&format!("  Context:   {ctx_str}\n"));
        out.push_str(&format!("  Effort:    {eff}\n"));
        out.push_str(&format!("  Energy:    {energy}\n"));
        out.push_str(&format!("  Workmode:  {wm}\n"));
        out.push_str(&format!("  Scheduled: {sched}\n"));
        out.push_str(&format!("  Deadline:  {dead}\n"));
        out.push_str(&format!(
            "  File:      {}:{}\n",
            doc.file.display(),
            entry.heading_line
        ));
        out.push_str(&format!("  Locator:   {loc}\n"));
    }
    out
}

/// Renders `candidates` grouped by `group_by` in human-readable format.
pub(crate) fn render_next_grouped_human(
    candidates: &[NextCandidate<'_>],
    group_by: NextGroupBy,
) -> String {
    if group_by == NextGroupBy::None {
        return candidates
            .iter()
            .map(|c| {
                format!(
                    "{}\n",
                    format_entry_line(&MatchedEntry {
                        doc: &c.loaded.doc,
                        entry_idx: c.entry_idx
                    })
                )
            })
            .collect();
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut fallback: Vec<String> = Vec::new();
    for c in candidates {
        let doc = &c.loaded.doc;
        let line = format_entry_line(&MatchedEntry {
            doc,
            entry_idx: c.entry_idx,
        });
        let keys: Vec<String> = match group_by {
            NextGroupBy::Context => entry_contexts(doc, c.entry_idx),
            NextGroupBy::Project => vec![c.project.name.clone()],
            NextGroupBy::Vault => vec![c
                .loaded
                .roam_dir
                .clone()
                .unwrap_or_else(|| "default".to_string())],
            NextGroupBy::Priority => doc.entries[c.entry_idx]
                .priority
                .map(|p| vec![format!("[#{p}]")])
                .unwrap_or_default(),
            NextGroupBy::Energy => inherited_property(doc, c.entry_idx, "ENERGY")
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            NextGroupBy::Workmode => inherited_property(doc, c.entry_idx, "WORKMODE")
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            NextGroupBy::None => Vec::new(),
        };
        if keys.is_empty() {
            fallback.push(line);
        } else {
            for k in keys {
                groups.entry(k).or_default().push(line.clone());
            }
        }
    }
    let mut out = String::new();
    for (label, entries) in &groups {
        append_named_group(&mut out, label, entries);
    }
    if !fallback.is_empty() {
        let fb_label = match group_by {
            NextGroupBy::Context => "No context",
            NextGroupBy::Priority => "No priority",
            NextGroupBy::Energy => "Unspecified energy",
            NextGroupBy::Workmode => "Unspecified workmode",
            _ => "Other",
        };
        append_named_group(&mut out, fb_label, &fallback);
    }
    out
}

/// Converts ranked [`NextCandidate`] items into enriched [`JsonEntry`] records.
pub(crate) fn build_next_json_entries(
    candidates: &[NextCandidate<'_>],
    doc_refs: &[&OrgDocument],
) -> Vec<JsonEntry> {
    candidates
        .iter()
        .map(|c| {
            let mut item = build_json_entry(
                &MatchedEntry {
                    doc: &c.loaded.doc,
                    entry_idx: c.entry_idx,
                },
                doc_refs,
            );
            item.roam_dir = c.loaded.roam_dir.clone();
            item.project = Some(c.project.name.clone());
            item.project_id = c.project.id.clone();
            item
        })
        .collect()
}

// endregion
