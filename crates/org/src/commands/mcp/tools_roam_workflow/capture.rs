// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP `org_roam_capture` tool handler for headless note and entry creation.

use std::fs;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{
    append_daily_capture_log, current_capture_datetime, expand_template_vars, find_template,
    mint_capture_id, render_capture_entry, render_capture_file, resolve_effective_templates,
    resolve_roam_link_or_raw, resolve_target_vault, slugify_title, CaptureRenderInput,
    CaptureTargetKind, EffectiveCaptureTemplate, TemplateVars,
};
use org_tools_core::config::Config;
use org_tools_core::files::{join_slash_rel, write_file_atomic};

use super::super::confinement::{canonical_workspace_root, verify_confined_mutation_target};
use super::super::schema::{
    finish_tool_call, optional_bool, optional_str, optional_str_array, optional_str_or_array,
    require_non_empty_str,
};
use super::super::McpSession;
use crate::commands::MutationTelemetry;

/// Assembles capture template and variables for a headless capture invocation.
fn build_headless_capture_context(
    args: &Map<String, Value>,
    vault_cfg: &Config,
    vault_name: &str,
    vault_path: &Path,
    title: &str,
    no_cache: bool,
) -> Result<(TemplateVars, CaptureRenderInput, EffectiveCaptureTemplate), String> {
    let tpl_name = optional_str(args, "template")?;
    let note_type = optional_str(args, "type")?;
    let all_templates = resolve_effective_templates(vault_cfg);
    let selector = tpl_name
        .as_deref()
        .or(note_type.as_deref())
        .unwrap_or("zettel");
    let tpl = find_template(&all_templates, selector)
        .or_else(|| {
            if let Some(ref nt) = note_type {
                all_templates
                    .values()
                    .find(|t| t.roam_type.as_deref() == Some(nt.as_str()) || t.name == *nt)
            } else {
                None
            }
        })
        .cloned()
        .ok_or_else(|| format!("no capture template matches '{selector}'"))?;

    let slug = optional_str(args, "slug")?.unwrap_or_else(|| slugify_title(title));
    let tags = optional_str_or_array(args, "tags")?.unwrap_or_default();
    let aliases = optional_str_array(args, "aliases")?.unwrap_or_default();
    let refs = optional_str_array(args, "refs")?.unwrap_or_default();
    let links = optional_str_array(args, "links")?.unwrap_or_default();

    let dt = current_capture_datetime();
    let id = mint_capture_id();
    let mut vars = TemplateVars::new(&id, title, &slug, vault_name, dt);

    if let Some(body) = optional_str(args, "body")? {
        vars.body = body;
    }
    if let Some(medium) = optional_str(args, "medium")? {
        vars.medium = resolve_roam_link_or_raw(vault_name, vault_path, &medium, no_cache);
    }
    if let Some(location) = optional_str(args, "location")? {
        vars.location = location;
    }
    if let Some(parent) = optional_str(args, "parent")? {
        vars.parent = resolve_roam_link_or_raw(vault_name, vault_path, &parent, no_cache);
    }

    let mut related_links = Vec::new();
    for l in links {
        related_links.push(resolve_roam_link_or_raw(
            vault_name, vault_path, &l, no_cache,
        ));
    }

    let input = CaptureRenderInput {
        extra_tags: tags,
        aliases,
        refs,
        related_links,
    };

    Ok((vars, input, tpl))
}

/// Renders target file or heading entry content.
fn render_capture_content(
    tpl: &EffectiveCaptureTemplate,
    vars: &TemplateVars,
    input: &CaptureRenderInput,
    dest_path: &Path,
) -> Result<String, String> {
    match tpl.kind {
        CaptureTargetKind::File => render_capture_file(tpl, vars, input),
        CaptureTargetKind::Entry => {
            let existing = fs::read_to_string(dest_path).unwrap_or_default();
            render_capture_entry(&existing, dest_path, tpl, vars, input).map(|(c, _)| c)
        }
    }
}

/// Parameters governing atomic capture writes and warm cache synchronization.
struct CaptureCommitParams<'a> {
    norm_ws: &'a Path,
    is_default: bool,
    vault_path: &'a Path,
    vault_cfg: &'a Config,
    dest_path: &'a Path,
    content: &'a str,
    vars: &'a TemplateVars,
    tpl: &'a EffectiveCaptureTemplate,
    dry_run: bool,
}

/// Validates confinement, atomically writes capture output, and syncs caches.
fn commit_capture(
    session: &mut McpSession,
    p: &CaptureCommitParams<'_>,
    started: Instant,
) -> Result<(), (String, MutationTelemetry)> {
    let canon_vault = canonical_workspace_root(p.vault_path)
        .map_err(|e| (e, session.empty_telemetry(started)))?;
    let _ = verify_confined_mutation_target(p.dest_path, &canon_vault)
        .map_err(|e| (e, session.empty_telemetry(started)))?;

    if p.dry_run {
        return Ok(());
    }

    write_file_atomic(p.dest_path, p.content)
        .map_err(|e| (e.to_string(), session.empty_telemetry(started)))?;

    if p.vault_cfg.roam.log_to_daily {
        let hhmm = format!("{}:{}", &p.vars.timestamp[8..10], &p.vars.timestamp[10..12]);
        let rtype = p.tpl.roam_type.as_deref().unwrap_or(&p.tpl.name);
        let _ = append_daily_capture_log(
            p.vault_path,
            &p.vault_cfg.roam.daily_subdir,
            &p.vars.date,
            &hhmm,
            rtype,
            &p.vars.id,
            &p.vars.title,
        );
    }

    session
        .invalidate_workspace_paths(
            p.norm_ws,
            p.is_default,
            std::slice::from_ref(&p.dest_path.to_path_buf()),
        )
        .map_err(|e| (e, session.empty_telemetry(started)))?;

    if p.vault_path != p.norm_ws && WorkspaceCache::resolve_db_path(p.vault_path).is_file() {
        if let Ok((mut v_cache, _)) = WorkspaceCache::open_and_sync(p.vault_path) {
            let _ = v_cache.invalidate_paths(std::slice::from_ref(&p.dest_path.to_path_buf()));
        }
    }
    Ok(())
}

/// Executes `org_roam_capture` headlessly creating notes without an interactive editor.
pub(crate) fn exec_roam_capture(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (_display_ws, norm_ws, is_default) = session
            .prepare_workspace(
                args,
                &[
                    "title",
                    "workspace",
                    "roam_dir",
                    "template",
                    "type",
                    "slug",
                    "tags",
                    "aliases",
                    "refs",
                    "medium",
                    "location",
                    "parent",
                    "links",
                    "body",
                    "dry_run",
                ],
            )
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let title = require_non_empty_str(args, "title")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let dry_run = optional_bool(args, "dry_run", false)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let roam_dir_arg =
            optional_str(args, "roam_dir").map_err(|e| (e, session.empty_telemetry(started)))?;

        let (v_name, v_path, v_cfg) = resolve_target_vault(Some(&norm_ws), roam_dir_arg.as_deref())
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let (vars, input, tpl) = build_headless_capture_context(
            args,
            &v_cfg,
            &v_name,
            &v_path,
            &title,
            session.no_cache,
        )
        .map_err(|e| (e, session.empty_telemetry(started)))?;

        let expanded_target = expand_template_vars(&tpl.target, &vars);
        let dest_path = join_slash_rel(&v_path, &expanded_target);
        let content = render_capture_content(&tpl, &vars, &input, &dest_path)
            .map_err(|e| (e, session.empty_telemetry(started)))?;

        let commit_params = CaptureCommitParams {
            norm_ws: &norm_ws,
            is_default,
            vault_path: &v_path,
            vault_cfg: &v_cfg,
            dest_path: &dest_path,
            content: &content,
            vars: &vars,
            tpl: &tpl,
            dry_run,
        };
        commit_capture(session, &commit_params, started)?;

        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;

        Ok(json!({
            "file": dest_path.display().to_string(),
            "id": vars.id,
            "title": vars.title,
            "locator": format!("id:{}", vars.id),
            "roam_dir": v_name,
            "dry_run": dry_run,
            "telemetry": tel
        }))
    })())
}
