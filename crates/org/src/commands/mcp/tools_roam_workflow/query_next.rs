// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP `org_query_next` tool handler with multi-vault GTD next action filtering.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::gtd_roam::{parse_effort_minutes, NextQueryFilter};
use org_tools_core::source::SourceFile;

use super::super::confinement::{canonical_workspace_root, verify_confined_dir};
use super::super::schema::{
    finish_tool_call, optional_bool, optional_positive_usize, optional_str, optional_str_array,
    optional_str_or_array,
};
use super::super::McpSession;
use crate::date;
use crate::query::next_render::{
    build_next_json_entries, collect_ranked_next_candidates, LoadedGtdDoc,
};

/// Resolves search roots for `org_query_next` from arguments or multi-vault config.
fn resolve_next_roots(
    norm_ws: &Path,
    args: &Map<String, Value>,
) -> Result<Vec<(Option<String>, PathBuf)>, String> {
    let cfg = Config::load_for_dir(norm_ws).map_err(|e| e.to_string())?;
    let configured = cfg.roam_directories();
    let canon_ws = canonical_workspace_root(norm_ws)?;
    let vault_paths: Vec<PathBuf> = configured.iter().map(|(_, p)| p.clone()).collect();

    let raw_roam_dirs = optional_str_or_array(args, "roam_dir")?;
    let Some(selectors) = raw_roam_dirs else {
        if !configured.is_empty() {
            return Ok(configured
                .into_iter()
                .filter(|(_, p)| p.exists())
                .map(|(name, p)| (Some(name), p))
                .collect());
        }
        return Ok(vec![(None, norm_ws.to_path_buf())]);
    };

    let mut roots = Vec::new();
    for sel in selectors {
        let (opt_name, path) = if let Some((n, p)) = cfg.resolve_roam_dir(&sel) {
            (n, p)
        } else {
            let direct = PathBuf::from(&sel);
            let candidate = if direct.is_absolute() {
                direct
            } else {
                norm_ws.join(&direct)
            };
            (None, candidate)
        };
        let confined = verify_confined_dir(&path, &canon_ws, &vault_paths)?;
        roots.push((opt_name.or(Some(sel)), confined));
    }
    Ok(roots)
}

/// Parses the query next filters from tool parameters.
fn parse_query_next_filter(
    args: &Map<String, Value>,
    default_warning_days: u32,
) -> Result<NextQueryFilter, String> {
    Ok(NextQueryFilter {
        project: optional_str(args, "project")?,
        context: optional_str(args, "context")?,
        energy: optional_str(args, "energy")?,
        workmode: optional_str(args, "workmode")?,
        max_effort_mins: optional_str(args, "max_effort")?
            .as_deref()
            .and_then(parse_effort_minutes),
        min_effort_mins: optional_str(args, "min_effort")?
            .as_deref()
            .and_then(parse_effort_minutes),
        priority: optional_str(args, "priority")?,
        tags: optional_str_or_array(args, "tags")?.unwrap_or_default(),
        goal: optional_str(args, "goal")?,
        due: optional_str(args, "due")?,
        next_only: optional_bool(args, "next_only", false)?,
        include_future: false,
        warning_days: default_warning_days,
    })
}

/// Scans and parses all `.org` files within a root directory.
fn query_single_root(
    vault_name: Option<&str>,
    root: &Path,
    explicit_paths: Option<&[String]>,
    v_cfg: &Config,
) -> Vec<LoadedGtdDoc> {
    let files: Vec<PathBuf> = if let Some(explicit) = explicit_paths {
        explicit
            .iter()
            .map(|p| root.join(p))
            .filter(|p| p.is_file())
            .collect()
    } else {
        collect_org_files(std::slice::from_ref(&root.to_path_buf()))
    };
    let mut docs = Vec::new();
    for file in files {
        let Ok(src) = SourceFile::from_path(&file) else {
            continue;
        };
        let doc = OrgDocument::from_source(&src);
        docs.push(LoadedGtdDoc {
            doc,
            roam_dir: vault_name.map(str::to_string),
            gtd_subdir: v_cfg.roam.gtd_subdir.clone(),
            warning_days: v_cfg.agenda.deadline_warning_days,
        });
    }
    docs
}

/// Executes `org_query_next` across multi-vault directories with confinement.
pub(crate) fn exec_query_next(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (_display_ws, norm_ws, is_default) = session
            .prepare_workspace(
                args,
                &[
                    "workspace",
                    "roam_dir",
                    "paths",
                    "project",
                    "context",
                    "energy",
                    "workmode",
                    "max_effort",
                    "min_effort",
                    "priority",
                    "tags",
                    "goal",
                    "due",
                    "next_only",
                    "pick",
                    "limit",
                ],
            )
            .map_err(|e| (e, session.empty_telemetry(started)))?;

        let explicit_paths =
            optional_str_array(args, "paths").map_err(|e| (e, session.empty_telemetry(started)))?;
        let roots = resolve_next_roots(&norm_ws, args)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let base_cfg = Config::load_for_dir(&norm_ws).unwrap_or_default();
        let filter = parse_query_next_filter(args, base_cfg.agenda.deadline_warning_days)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let pick = optional_bool(args, "pick", false)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let limit = optional_positive_usize(args, "limit")
            .map_err(|e| (e, session.empty_telemetry(started)))?;

        let mut loaded = Vec::new();
        for (v_name, root) in &roots {
            let v_cfg = Config::load_for_dir(root).unwrap_or_else(|_| base_cfg.clone());
            loaded.extend(query_single_root(
                v_name.as_deref(),
                root,
                explicit_paths.as_deref(),
                &v_cfg,
            ));
        }

        let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
        let cap = if pick {
            Some(limit.unwrap_or(1))
        } else {
            limit
        };
        let candidates =
            collect_ranked_next_candidates(&loaded, &doc_refs, &filter, date::current_date(), cap);
        let next_actions = build_next_json_entries(&candidates, &doc_refs);

        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;

        Ok(json!({
            "count": next_actions.len(),
            "next_actions": next_actions,
            "telemetry": tel
        }))
    })())
}
