// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP `org_fleeting_transform` tool handler for converting or discarding fleeting notes.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::capture::{current_capture_datetime, resolve_target_vault, TemplateVars};
use org_tools_core::config::Config;
use org_tools_core::files::write_file_atomic;
use org_tools_core::fleeting::{
    resolve_fleeting_target, update_source_after_discard, FleetingItem,
};

use super::super::confinement::{
    canonical_workspace_root, verify_confined_mutation_target,
    verify_confined_mutation_target_in_roots,
};
use super::super::schema::{finish_tool_call, optional_bool, optional_str, require_non_empty_str};
use super::super::McpSession;
use super::fleeting_build::{build_task_transform, build_zettel_transform};
use crate::commands::MutationTelemetry;

// region Resolution Helpers

/// Resolves search roots and configured vault paths for fleeting transformation.
fn resolve_fleeting_roots(
    norm_ws: &Path,
    v_path: &Path,
    v_cfg: &Config,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut search_roots = vec![v_path.to_path_buf(), norm_ws.to_path_buf()];
    let mut vault_roots = vec![v_path.to_path_buf()];
    for (_, p) in v_cfg.roam_directories() {
        if p.exists() {
            if !search_roots.contains(&p) {
                search_roots.push(p.clone());
            }
            if !vault_roots.contains(&p) {
                vault_roots.push(p);
            }
        }
    }
    (search_roots, vault_roots)
}

/// Resolves and validates the destination GTD inbox file path.
fn resolve_inbox_path(
    args: &Map<String, Value>,
    norm_ws: &Path,
    canon_ws: &Path,
    v_path: &Path,
    v_cfg: &Config,
    vault_roots: &[PathBuf],
) -> Result<PathBuf, String> {
    if let Some(dest) = optional_str(args, "to")? {
        let dest_path = PathBuf::from(&dest);
        let candidate = if dest_path.is_absolute() {
            dest_path
        } else {
            norm_ws.join(&dest_path)
        };
        verify_confined_mutation_target_in_roots(&candidate, canon_ws, vault_roots)
    } else {
        let default_inbox = v_path.join(&v_cfg.roam.gtd_subdir).join("inbox.org");
        let canon_vault = canonical_workspace_root(v_path)?;
        verify_confined_mutation_target(&default_inbox, &canon_vault)
    }
}

// endregion

// region Action Dispatchers

/// Context passed into action dispatchers during fleeting transformation.
struct TransformCtx<'a> {
    norm_ws: &'a Path,
    canon_ws: &'a Path,
    is_default: bool,
    args: &'a Map<String, Value>,
    item: &'a FleetingItem,
    v_name: &'a str,
    v_path: &'a Path,
    v_cfg: &'a Config,
    vault_roots: &'a [PathBuf],
    dry_run: bool,
    started: Instant,
}

/// Executes the `to-zettel` action on a fleeting note.
fn exec_transform_to_zettel(
    session: &mut McpSession,
    ctx: &TransformCtx<'_>,
) -> Result<Value, (String, MutationTelemetry)> {
    let (z_id, z_title, z_dest, z_content, updated_src) = build_zettel_transform(
        ctx.args,
        ctx.item,
        ctx.v_name,
        ctx.v_path,
        ctx.v_cfg,
        session.no_cache,
    )
    .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;

    let canon_vault = canonical_workspace_root(ctx.v_path)
        .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    let verified_dest = verify_confined_mutation_target(&z_dest, &canon_vault)
        .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    let src_path = PathBuf::from(&ctx.item.file);
    let verified_src =
        verify_confined_mutation_target_in_roots(&src_path, ctx.canon_ws, ctx.vault_roots)
            .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;

    if !ctx.dry_run {
        write_file_atomic(&verified_dest, &z_content)
            .map_err(|e| (e.to_string(), session.empty_telemetry(ctx.started)))?;
        write_file_atomic(&verified_src, &updated_src)
            .map_err(|e| (e.to_string(), session.empty_telemetry(ctx.started)))?;
        session
            .invalidate_workspace_paths(
                ctx.norm_ws,
                ctx.is_default,
                &[verified_dest.clone(), verified_src.clone()],
            )
            .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    }
    let mut tel = session
        .sync_telemetry(ctx.norm_ws, ctx.is_default, ctx.started)
        .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    tel.duration_us = ctx.started.elapsed().as_micros() as u64;

    Ok(json!({
        "action": "to-zettel",
        "target": ctx.item.id.as_deref().unwrap_or(&ctx.item.title),
        "zettel_file": verified_dest.display().to_string(),
        "zettel_id": z_id,
        "zettel_title": z_title,
        "source_file": verified_src.display().to_string(),
        "dry_run": ctx.dry_run,
        "telemetry": tel
    }))
}

/// Executes the `to-task` action on a fleeting note.
fn exec_transform_to_task(
    session: &mut McpSession,
    ctx: &TransformCtx<'_>,
) -> Result<Value, (String, MutationTelemetry)> {
    let inbox_path = resolve_inbox_path(
        ctx.args,
        ctx.norm_ws,
        ctx.canon_ws,
        ctx.v_path,
        ctx.v_cfg,
        ctx.vault_roots,
    )
    .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;

    let (task_id, t_title, new_inbox, updated_src) = build_task_transform(
        ctx.args,
        ctx.item,
        ctx.v_name,
        ctx.v_path,
        ctx.v_cfg,
        &inbox_path,
        session.no_cache,
    )
    .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;

    let src_path = PathBuf::from(&ctx.item.file);
    let verified_src =
        verify_confined_mutation_target_in_roots(&src_path, ctx.canon_ws, ctx.vault_roots)
            .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;

    if !ctx.dry_run {
        write_file_atomic(&inbox_path, &new_inbox)
            .map_err(|e| (e.to_string(), session.empty_telemetry(ctx.started)))?;
        write_file_atomic(&verified_src, &updated_src)
            .map_err(|e| (e.to_string(), session.empty_telemetry(ctx.started)))?;
        session
            .invalidate_workspace_paths(
                ctx.norm_ws,
                ctx.is_default,
                &[inbox_path.clone(), verified_src.clone()],
            )
            .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    }
    let mut tel = session
        .sync_telemetry(ctx.norm_ws, ctx.is_default, ctx.started)
        .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    tel.duration_us = ctx.started.elapsed().as_micros() as u64;

    Ok(json!({
        "action": "to-task",
        "target": ctx.item.id.as_deref().unwrap_or(&ctx.item.title),
        "task_id": task_id,
        "task_title": t_title,
        "inbox_file": inbox_path.display().to_string(),
        "source_file": verified_src.display().to_string(),
        "dry_run": ctx.dry_run,
        "telemetry": tel
    }))
}

/// Executes the `discard` action on a fleeting note.
fn exec_transform_discard(
    session: &mut McpSession,
    ctx: &TransformCtx<'_>,
) -> Result<Value, (String, MutationTelemetry)> {
    let delete_file = optional_bool(ctx.args, "delete", false)
        .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    let src_path = PathBuf::from(&ctx.item.file);
    let verified_src =
        verify_confined_mutation_target_in_roots(&src_path, ctx.canon_ws, ctx.vault_roots)
            .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;

    if !ctx.dry_run {
        if delete_file {
            let _ = fs::remove_file(&verified_src);
        } else {
            let dt = current_capture_datetime();
            let vars = TemplateVars::new("temp", "temp", "temp", "temp", dt);
            let updated_src = update_source_after_discard(ctx.item, &vars.timestamp_inactive)
                .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
            write_file_atomic(&verified_src, &updated_src)
                .map_err(|e| (e.to_string(), session.empty_telemetry(ctx.started)))?;
        }
        session
            .invalidate_workspace_paths(
                ctx.norm_ws,
                ctx.is_default,
                std::slice::from_ref(&verified_src),
            )
            .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    }
    let mut tel = session
        .sync_telemetry(ctx.norm_ws, ctx.is_default, ctx.started)
        .map_err(|e| (e, session.empty_telemetry(ctx.started)))?;
    tel.duration_us = ctx.started.elapsed().as_micros() as u64;

    Ok(json!({
        "action": "discard",
        "target": ctx.item.id.as_deref().unwrap_or(&ctx.item.title),
        "source_file": verified_src.display().to_string(),
        "deleted": delete_file,
        "dry_run": ctx.dry_run,
        "telemetry": tel
    }))
}

// endregion

/// Executes `org_fleeting_transform` headlessly converting or discarding notes.
pub(crate) fn exec_fleeting_transform(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (_display_ws, norm_ws, is_default) = session
            .prepare_workspace(
                args,
                &[
                    "target",
                    "action",
                    "workspace",
                    "roam_dir",
                    "dry_run",
                    "title",
                    "slug",
                    "parent",
                    "links",
                    "body",
                    "to",
                    "context",
                    "effort",
                    "scheduled",
                    "deadline",
                    "delete",
                ],
            )
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let target = require_non_empty_str(args, "target")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let action = require_non_empty_str(args, "action")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let dry_run = optional_bool(args, "dry_run", false)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let roam_dir_arg =
            optional_str(args, "roam_dir").map_err(|e| (e, session.empty_telemetry(started)))?;

        let (v_name, v_path, v_cfg) = resolve_target_vault(Some(&norm_ws), roam_dir_arg.as_deref())
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let (search_roots, vault_roots) = resolve_fleeting_roots(&norm_ws, &v_path, &v_cfg);
        let item = resolve_fleeting_target(&search_roots, &target)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let canon_ws = canonical_workspace_root(&norm_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;

        let ctx = TransformCtx {
            norm_ws: &norm_ws,
            canon_ws: &canon_ws,
            is_default,
            args,
            item: &item,
            v_name: &v_name,
            v_path: &v_path,
            v_cfg: &v_cfg,
            vault_roots: &vault_roots,
            dry_run,
            started,
        };

        match action.as_str() {
            "to-zettel" => exec_transform_to_zettel(session, &ctx),
            "to-task" => exec_transform_to_task(session, &ctx),
            "discard" => exec_transform_discard(session, &ctx),
            other => Err((
                format!("invalid fleeting transform action: '{other}'"),
                session.empty_telemetry(started),
            )),
        }
    })())
}
