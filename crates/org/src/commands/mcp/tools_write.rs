// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mutating MCP tool handlers (`org_update_state`, `org_fmt_file`) with warm cache invalidation.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::edna::{
    execute_trigger_cascade, seed_for_entry, TriggeredMutation, WorkspaceOverlay,
};
use org_tools_core::files::collect_org_files;
use org_tools_core::locator::{resolve_in_document, resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::output::{render_diagnostics, OutputFormat};
use org_tools_core::runner::Runner;
use org_tools_core::source::SourceFile;
use org_tools_core::state::{set_state, SetStateResult};

use super::confinement::{
    canonical_workspace_root, confine_and_rewrite_locator, resolve_confined_org_path,
    verify_confined_mutation_target, verify_existing_confined_org_file,
};
use super::schema::{finish_tool_call, optional_bool, require_non_empty_str};
use super::McpSession;
use crate::commands::MutationDetail;

/// Resolves confined `locator` within `display_ws` / `canon_ws`, checking warm `MabuseCache` first.
fn resolve_state_target(
    session: &McpSession,
    is_default: bool,
    display_ws: &Path,
    canon_ws: &Path,
    locator: &OrgLocator,
    locator_label: &str,
) -> Result<ResolvedEntry, String> {
    if let (Some(cache), OrgLocator::Id(uuid)) = (session.warm_cache_for(is_default), locator) {
        if let Ok(nodes) = cache.resolve_roam_nodes(display_ws, &format!("id:{uuid}"), false) {
            if let Some(first) = nodes.first() {
                let cand = Path::new(&first.file);
                verify_existing_confined_org_file(cand, canon_ws, locator_label)?;
                if let Ok(src) = SourceFile::from_path(cand) {
                    let doc = OrgDocument::from_source(&src);
                    if let Ok(mut resolved) = resolve_in_document(locator, &doc) {
                        resolved.file = PathBuf::from(&first.file);
                        return Ok(resolved);
                    }
                }
            }
        }
    }
    let resolved =
        resolve_locator(locator, &[display_ws.to_path_buf()]).map_err(|e| e.to_string())?;
    verify_existing_confined_org_file(&resolved.file, canon_ws, locator_label)?;
    Ok(resolved)
}

/// Collects [`MutationDetail`] entries for headings whose TODO keyword changed in `file`.
fn collect_primary_state_details(
    file: &Path,
    before: &OrgDocument,
    after_content: &str,
) -> Vec<MutationDetail> {
    let after = OrgDocument::from_source(&SourceFile::new(file, after_content.to_string()));
    let file_str = file.display().to_string();
    after
        .entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let prev = before.entries.get(idx)?;
            (prev.keyword != entry.keyword).then(|| MutationDetail {
                file: file_str.clone(),
                line: entry.heading_line,
                action: "state-changed".to_string(),
                detail: Some(format!(
                    "{} -> {}",
                    prev.keyword.as_deref().unwrap_or("<none>"),
                    entry.keyword.as_deref().unwrap_or("<none>")
                )),
            })
        })
        .collect()
}

/// Converts triggered cascade mutations into `(combined_details, cascaded_details)` vectors.
fn merge_cascade_details(
    mut details: Vec<MutationDetail>,
    mutations: Vec<TriggeredMutation>,
) -> (Vec<MutationDetail>, Vec<MutationDetail>) {
    let cascaded: Vec<MutationDetail> = mutations
        .into_iter()
        .map(|m| MutationDetail {
            file: m.file.display().to_string(),
            line: m.line,
            action: m.action,
            detail: m.detail,
        })
        .collect();
    for item in &cascaded {
        details.push(MutationDetail {
            file: item.file.clone(),
            line: item.line,
            action: item.action.clone(),
            detail: item.detail.clone(),
        });
    }
    (details, cascaded)
}

/// Modified file paths, affected count, combined details, and cascaded details from a state update.
type CascadeApplyOutcome = (
    Vec<PathBuf>,
    usize,
    Vec<MutationDetail>,
    Vec<MutationDetail>,
);

/// Builds a [`WorkspaceOverlay`] populated strictly with confined `.org` files inside `canon_ws`.
fn load_confined_overlay(
    display_ws: &Path,
    canon_ws: &Path,
    primary_file: &Path,
) -> WorkspaceOverlay {
    let mut overlay = WorkspaceOverlay::default();
    for path in collect_org_files(&[display_ws.to_path_buf()]) {
        if verify_existing_confined_org_file(&path, canon_ws, "").is_ok() {
            overlay.ensure_loaded(&path);
        }
    }
    overlay.ensure_loaded(primary_file);
    overlay
}

/// Evaluates `:TRIGGER:` cascades inside a confined [`WorkspaceOverlay`], verifying every output path.
fn run_confined_cascade(
    file: &Path,
    doc: &OrgDocument,
    primary: SetStateResult,
    details: Vec<MutationDetail>,
    display_ws: &Path,
    canon_ws: &Path,
    dry_run: bool,
) -> Result<CascadeApplyOutcome, String> {
    let mut overlay = load_confined_overlay(display_ws, canon_ws, file);
    overlay.set_content(file, primary.content.clone());
    let post_doc = OrgDocument::from_source(&SourceFile::new(file, primary.content));
    let seeds = primary
        .newly_done_indices
        .iter()
        .copied()
        .filter(|&idx| doc.property(idx, "TRIGGER").is_some())
        .map(|idx| seed_for_entry(&post_doc, idx))
        .collect();
    let cascade = execute_trigger_cascade(&mut overlay, seeds, true);

    let modified: Vec<PathBuf> = overlay
        .modified_files()
        .into_iter()
        .map(|(p, _)| p.to_path_buf())
        .collect();
    for target_path in &modified {
        verify_confined_mutation_target(target_path, canon_ws)?;
    }
    if !dry_run {
        overlay.commit_to_disk()?;
    }
    let (all_details, cascaded) = merge_cascade_details(details, cascade.mutations);
    Ok((
        modified,
        primary.changed + cascade.entries_affected,
        all_details,
        cascaded,
    ))
}

/// Applies a state transition (plus `:TRIGGER:` cascade), verifying confinement before writing.
fn apply_state_and_cascade(
    file: &Path,
    doc: &OrgDocument,
    primary: SetStateResult,
    display_ws: &Path,
    canon_ws: &Path,
    dry_run: bool,
) -> Result<CascadeApplyOutcome, String> {
    verify_confined_mutation_target(file, canon_ws)?;
    let details = collect_primary_state_details(file, doc, &primary.content);
    let has_triggers = primary
        .newly_done_indices
        .iter()
        .any(|&idx| doc.property(idx, "TRIGGER").is_some());
    if has_triggers {
        return run_confined_cascade(file, doc, primary, details, display_ws, canon_ws, dry_run);
    }
    if !dry_run {
        fs::write(file, &primary.content)
            .map_err(|e| format!("error writing {}: {e}", file.display()))?;
    }
    Ok((
        vec![file.to_path_buf()],
        primary.changed,
        details,
        Vec::new(),
    ))
}

/// Validated workspace and locator parameters for `org_update_state`.
struct StateCallCtx {
    /// Caller-facing workspace directory.
    display_ws: PathBuf,
    /// Normalized workspace directory.
    norm_ws: PathBuf,
    /// Canonical workspace directory.
    canon_ws: PathBuf,
    /// Whether `norm_ws` is the session's default workspace.
    is_default: bool,
    /// Confined and workspace-rewritten locator.
    locator: OrgLocator,
    /// Raw locator string passed by caller.
    locator_str: String,
    /// Target TODO keyword string.
    state: String,
    /// Whether disk writes are suppressed.
    dry_run: bool,
}

/// Parses and confines all parameters for `org_update_state`.
fn parse_state_call_ctx(
    session: &McpSession,
    args: &Map<String, Value>,
) -> Result<StateCallCtx, String> {
    let allowed = &["locator", "state", "dry_run", "workspace"];
    let (display_ws, norm_ws, is_default) = session.prepare_workspace(args, allowed)?;
    let locator_str = require_non_empty_str(args, "locator")?;
    let state = require_non_empty_str(args, "state")?;
    let dry_run = optional_bool(args, "dry_run", false)?;
    let canon_ws = canonical_workspace_root(&norm_ws)?;
    let raw_locator = OrgLocator::parse(&locator_str).map_err(|e| e.to_string())?;
    let locator = confine_and_rewrite_locator(&raw_locator, &display_ws, &canon_ws)?;
    Ok(StateCallCtx {
        display_ws,
        norm_ws,
        canon_ws,
        is_default,
        locator,
        locator_str,
        state,
        dry_run,
    })
}

/// Validates and executes the core confined mutation pass for `org_update_state`.
fn execute_state_mutation_pass(
    session: &McpSession,
    ctx: &StateCallCtx,
) -> Result<CascadeApplyOutcome, String> {
    let resolved = resolve_state_target(
        session,
        ctx.is_default,
        &ctx.display_ws,
        &ctx.canon_ws,
        &ctx.locator,
        &ctx.locator_str,
    )?;
    if resolved.is_file_node() {
        return Err(format!(
            "cannot set TODO state on file-level node {}",
            ctx.locator_str
        ));
    }
    let source = SourceFile::from_path(&resolved.file)
        .map_err(|e| format!("error reading {}: {e}", resolved.file.display()))?;
    let doc = OrgDocument::from_source(&source);
    if !doc.todo_keywords.contains(&ctx.state) {
        return Err(format!(
            "'{}' is not a recognized TODO keyword in {}",
            ctx.state,
            resolved.file.display()
        ));
    }
    let Some(primary) = set_state(
        &source,
        &doc,
        Some(&[resolved.entry_index]),
        &ctx.state,
        true,
    ) else {
        return Ok((Vec::new(), 0, Vec::new(), Vec::new()));
    };
    apply_state_and_cascade(
        &resolved.file,
        &doc,
        primary,
        &ctx.display_ws,
        &ctx.canon_ws,
        ctx.dry_run,
    )
}

/// Executes `org_update_state`, invalidating and refreshing `MabuseCache` after disk mutations.
pub(super) fn exec_update_state(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let ctx = parse_state_call_ctx(session, args)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let outcome = execute_state_mutation_pass(session, &ctx);
        if let (Ok((ref paths, _, _, _)), false) = (&outcome, ctx.dry_run) {
            session
                .invalidate_workspace_paths(&ctx.norm_ws, ctx.is_default, paths)
                .map_err(|e| (e, session.empty_telemetry(started)))?;
        }
        let mut tel = session
            .sync_telemetry(&ctx.norm_ws, ctx.is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;

        let (paths, entries_affected, details, cascaded) = match outcome {
            Ok(v) => v,
            Err(msg) => return Err((msg, tel)),
        };
        Ok(json!({
            "command": "set-state",
            "files_modified": paths.len(),
            "entries_affected": entries_affected,
            "dry_run": ctx.dry_run,
            "details": details,
            "cascaded": cascaded,
            "errors": [],
            "telemetry": tel
        }))
    })())
}

/// Runs `check_with_workspace` or `format_with_workspace` on confined `.org` `target`.
fn execute_fmt_on_file(
    target: &Path,
    display_ws: &Path,
    canon_ws: &Path,
    norm_ws: &Path,
    check_only: bool,
) -> Result<(bool, Vec<Value>), String> {
    let source = SourceFile::from_path(target)
        .map_err(|e| format!("error reading {}: {e}", target.display()))?;
    let workspace_docs: Vec<OrgDocument> = collect_org_files(&[display_ws.to_path_buf()])
        .into_iter()
        .filter(|p| verify_existing_confined_org_file(p, canon_ws, "").is_ok())
        .filter_map(|p| SourceFile::from_path(&p).ok())
        .map(|s| OrgDocument::from_source(&s))
        .collect();
    let runner = Runner::new(Config::load(norm_ws));

    if check_only {
        let diags = runner.check_with_workspace(&source, &workspace_docs);
        let changed = diags.iter().any(|d| d.fix.is_some());
        let json_diags: Vec<Value> =
            serde_json::from_str(&render_diagnostics(&diags, OutputFormat::Json))
                .unwrap_or_default();
        return Ok((changed, json_diags));
    }

    let (formatted, diags) = runner.format_with_workspace(&source, &workspace_docs);
    let changed = formatted != source.content;
    if changed {
        fs::write(target, &formatted)
            .map_err(|e| format!("error writing {}: {e}", target.display()))?;
    }
    let json_diags: Vec<Value> =
        serde_json::from_str(&render_diagnostics(&diags, OutputFormat::Json)).unwrap_or_default();
    Ok((changed, json_diags))
}

/// Parses and confines all parameters for `org_fmt_file`.
fn parse_fmt_call_args(
    session: &McpSession,
    args: &Map<String, Value>,
) -> Result<(PathBuf, PathBuf, PathBuf, bool, PathBuf, bool), String> {
    let (display_ws, norm_ws, is_default) =
        session.prepare_workspace(args, &["path", "check_only", "workspace"])?;
    let raw_path = require_non_empty_str(args, "path")?;
    let check_only = optional_bool(args, "check_only", false)?;
    let canon_ws = canonical_workspace_root(&norm_ws)?;
    let target = resolve_confined_org_path(&raw_path, &display_ws, &canon_ws)?;
    Ok((
        display_ws, norm_ws, canon_ws, is_default, target, check_only,
    ))
}

/// Executes `org_fmt_file` on a confined `.org` file and refreshes `MabuseCache` when changed.
pub(super) fn exec_fmt_file(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (display_ws, norm_ws, canon_ws, is_default, target, check_only) =
            parse_fmt_call_args(session, args)
                .map_err(|msg| (msg, session.empty_telemetry(started)))?;

        let outcome = execute_fmt_on_file(&target, &display_ws, &canon_ws, &norm_ws, check_only);
        if let (Ok((true, _)), false) = (&outcome, check_only) {
            session
                .invalidate_workspace_paths(&norm_ws, is_default, std::slice::from_ref(&target))
                .map_err(|e| (e, session.empty_telemetry(started)))?;
        }
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;

        let (changed, diagnostics) = match outcome {
            Ok(v) => v,
            Err(msg) => return Err((msg, tel)),
        };
        Ok(json!({
            "file": target.display().to_string(),
            "changed": changed,
            "check_only": check_only,
            "diagnostics": diagnostics,
            "telemetry": tel
        }))
    })())
}
