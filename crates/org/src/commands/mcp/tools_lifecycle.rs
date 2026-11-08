// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lifecycle MCP tool handler (`org_refile`) with workspace confinement and cache invalidation.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::document::OrgDocument;
use org_tools_core::locator::{resolve_in_document, OrgLocator, ResolvedEntry};
use org_tools_core::property::resolve_tag_column;
use org_tools_core::refile::{RefileDestination, RefileOptions, RefileOutcome};
use org_tools_core::source::SourceFile;

use super::confinement::{
    canonical_workspace_root, resolve_confined_org_path, verify_confined_mutation_target,
    verify_existing_confined_org_file,
};
use super::schema::{
    finish_tool_call, optional_bool, require_non_empty_str, tool_descriptor, TOOL_REFILE,
};
use super::McpSession;
use crate::commands::refile::{
    commit_refile_writes, execute_refile_pass, is_plain_org_path_target, resolve_flexible_locator,
};
use crate::commands::MutationDetail;

/// Builds the `tools/list` JSON Schema descriptor for `org_refile`.
pub(super) fn refile_tool_descriptor() -> Value {
    tool_descriptor(
        TOOL_REFILE,
        "Atomically move an Org subtree across files or under a new heading with star-level and F007 tag alignment.",
        json!({
            "source_locator": { "type": "string", "description": "Source heading locator (id:<uuid>, custom-id:<id>, olp:<path>, *Heading, or <file>::<target>)." },
            "target": { "type": "string", "description": "Destination heading locator or workspace-relative .org file path." },
            "as_root": { "type": "boolean", "description": "Refile as a level-1 root heading at the end of the target file (default: false)." },
            "dry_run": { "type": "boolean", "description": "Preview the refile mutation without writing files (default: false)." },
            "workspace": { "type": "string", "description": "Optional workspace root directory override." }
        }),
        &["source_locator", "target"],
    )
}

/// Validated workspace and argument context for an `org_refile` MCP tool call.
struct RefileCallCtx {
    /// Caller-facing workspace directory.
    display_ws: PathBuf,
    /// Normalized workspace directory.
    norm_ws: PathBuf,
    /// Canonical workspace directory.
    canon_ws: PathBuf,
    /// Whether `norm_ws` is the session's default workspace.
    is_default: bool,
    /// Raw source locator string.
    source_locator: String,
    /// Raw destination locator or workspace-relative `.org` file path.
    target: String,
    /// Whether to refile as a level-1 root heading.
    as_root: bool,
    /// Whether disk writes are suppressed.
    dry_run: bool,
}

/// Parses and validates arguments for `org_refile`.
fn parse_refile_call_ctx(
    session: &McpSession,
    args: &Map<String, Value>,
) -> Result<RefileCallCtx, String> {
    let allowed = &[
        "source_locator",
        "target",
        "as_root",
        "dry_run",
        "workspace",
    ];
    let (display_ws, norm_ws, is_default) = session.prepare_workspace(args, allowed)?;
    let source_locator = require_non_empty_str(args, "source_locator")?;
    let target = require_non_empty_str(args, "target")?;
    let as_root = optional_bool(args, "as_root", false)?;
    let dry_run = optional_bool(args, "dry_run", false)?;
    let canon_ws = canonical_workspace_root(&norm_ws)?;
    Ok(RefileCallCtx {
        display_ws,
        norm_ws,
        canon_ws,
        is_default,
        source_locator,
        target,
        as_root,
        dry_run,
    })
}

/// Rewrites any `<file>::<rest>` locator prefix to a confined path inside `canon_ws`.
fn confine_locator_str(raw: &str, display_ws: &Path, canon_ws: &Path) -> Result<String, String> {
    let trimmed = raw.trim();
    if let Some((file_part, rest)) = trimmed.split_once("::") {
        if !file_part.starts_with("id:")
            && !file_part.starts_with("custom-id:")
            && !file_part.starts_with("olp:")
        {
            let confined = resolve_confined_org_path(file_part, display_ws, canon_ws)?;
            return Ok(format!("{}::{rest}", confined.display()));
        }
    }
    Ok(trimmed.to_string())
}

/// Resolves a confined locator within `display_ws`, checking warm `MabuseCache` first for `id:`.
fn resolve_confined_refile_locator(
    session: &McpSession,
    ctx: &RefileCallCtx,
    raw_locator: &str,
    default_file: Option<&Path>,
) -> Result<ResolvedEntry, String> {
    let rewritten = confine_locator_str(raw_locator, &ctx.display_ws, &ctx.canon_ws)?;
    if let Some(uuid) = rewritten.strip_prefix("id:").map(str::trim) {
        if let Some(cache) = session.warm_cache_for(ctx.is_default) {
            if let Ok(nodes) =
                cache.resolve_roam_nodes(&ctx.display_ws, &format!("id:{uuid}"), false)
            {
                if let Some(first) = nodes.first() {
                    let cand = Path::new(&first.file);
                    verify_existing_confined_org_file(cand, &ctx.canon_ws, raw_locator)?;
                    if let Ok(src) = SourceFile::from_path(cand) {
                        let doc = OrgDocument::from_source(&src);
                        let loc = OrgLocator::Id(uuid.to_string());
                        if let Ok(mut resolved) = resolve_in_document(&loc, &doc) {
                            resolved.file = PathBuf::from(&first.file);
                            return Ok(resolved);
                        }
                    }
                }
            }
        }
    }
    let resolved = resolve_flexible_locator(
        &rewritten,
        std::slice::from_ref(&ctx.display_ws),
        default_file,
    )?;
    verify_existing_confined_org_file(&resolved.file, &ctx.canon_ws, raw_locator)?;
    Ok(resolved)
}

/// Resolves `ctx.target` into a confined `(dest_file, RefileDestination)` pair.
fn resolve_confined_destination(
    session: &McpSession,
    ctx: &RefileCallCtx,
    source_file: &Path,
) -> Result<(PathBuf, RefileDestination), String> {
    if is_plain_org_path_target(&ctx.target) {
        let dest_file =
            resolve_confined_org_path(ctx.target.trim(), &ctx.display_ws, &ctx.canon_ws)?;
        return Ok((dest_file, RefileDestination::FileRoot));
    }
    let resolved = resolve_confined_refile_locator(session, ctx, &ctx.target, Some(source_file))?;
    if resolved.is_file_node() || ctx.as_root {
        Ok((resolved.file, RefileDestination::FileRoot))
    } else {
        Ok((
            resolved.file,
            RefileDestination::UnderEntry(resolved.entry_index),
        ))
    }
}

/// Executes the confined refile pass and commits atomic file writes unless `ctx.dry_run` is set.
fn execute_confined_refile(
    session: &McpSession,
    ctx: &RefileCallCtx,
) -> Result<(RefileOutcome, PathBuf, Vec<PathBuf>), String> {
    let source_res = resolve_confined_refile_locator(session, ctx, &ctx.source_locator, None)?;
    if source_res.is_file_node() {
        return Err(format!(
            "cannot refile file-level node '{}'; specify a heading locator",
            ctx.source_locator
        ));
    }
    let (dest_file, dest_target) = resolve_confined_destination(session, ctx, &source_res.file)?;
    verify_confined_mutation_target(&source_res.file, &ctx.canon_ws)?;
    verify_confined_mutation_target(&dest_file, &ctx.canon_ws)?;

    let opts = RefileOptions {
        as_root: ctx.as_root,
        tag_column: resolve_tag_column(None),
    };
    let (outcome, modified_files) = execute_refile_pass(
        &source_res.file,
        source_res.entry_index,
        &dest_file,
        dest_target,
        opts,
    )?;
    if !ctx.dry_run {
        commit_refile_writes(&source_res.file, &dest_file, &outcome)?;
    }
    Ok((outcome, dest_file, modified_files))
}

/// Executes `org_refile`, enforcing workspace confinement and refreshing warm `MabuseCache`.
pub(super) fn exec_refile(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let ctx = parse_refile_call_ctx(session, args)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let outcome_res = execute_confined_refile(session, &ctx);
        if let (Ok((_, _, ref paths)), false) = (&outcome_res, ctx.dry_run) {
            session
                .invalidate_workspace_paths(&ctx.norm_ws, ctx.is_default, paths)
                .map_err(|e| (e, session.empty_telemetry(started)))?;
        }
        let mut tel = session
            .sync_telemetry(&ctx.norm_ws, ctx.is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;

        let (outcome, dest_file, modified_files) = match outcome_res {
            Ok(v) => v,
            Err(msg) => return Err((msg, tel)),
        };
        let detail = MutationDetail {
            file: dest_file.display().to_string(),
            line: outcome.new_line,
            action: "refiled".to_string(),
            detail: Some(format!(
                "{} -> {} (level {} -> {})",
                outcome.title, outcome.new_locator, outcome.old_level, outcome.new_level
            )),
        };
        Ok(json!({
            "command": "refile",
            "files_modified": modified_files.len(),
            "entries_affected": 1,
            "dry_run": ctx.dry_run,
            "details": [detail],
            "errors": [],
            "telemetry": tel
        }))
    })())
}
