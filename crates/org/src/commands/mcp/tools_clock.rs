// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP handlers for `org_clock_in` and `org_clock_out`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::files::collect_org_files;

use super::confinement::{
    canonical_workspace_root, confine_and_rewrite_locator, verify_confined_mutation_target,
    verify_existing_confined_org_file,
};
use super::schema::{
    finish_tool_call, optional_bool, optional_str, require_non_empty_str, tool_descriptor,
};
use super::McpSession;
use crate::clock::mutate::{
    execute_clock_in_on_files, parse_clock_locator, terminate_active_clock,
};

// region Constants & Schema Descriptors

/// MCP tool name starting a clock on a target heading entry.
pub(super) const TOOL_CLOCK_IN: &str = "org_clock_in";

/// MCP tool name closing an active clock on a target or workspace heading entry.
pub(super) const TOOL_CLOCK_OUT: &str = "org_clock_out";

/// Builds the `tools/list` schema descriptors for `org_clock_in` and `org_clock_out`.
pub(super) fn clock_tool_descriptors() -> Vec<Value> {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    vec![
        tool_descriptor(
            TOOL_CLOCK_IN,
            "Start an active CLOCK entry in the target heading's :LOGBOOK: drawer.",
            json!({
                "locator": { "type": "string", "description": "OrgLocator identifying the target heading entry." },
                "at": { "type": "string", "description": "Optional start timestamp (YYYY-MM-DDTHH:MM, YYYY-MM-DD HH:MM, or [YYYY-MM-DD Day HH:MM])." },
                "auto_out": { "type": "boolean", "description": "Automatically clock out any existing running clock at the same timestamp." },
                "dry_run": { "type": "boolean", "description": "Preview clock-in mutation without writing files." },
                "workspace": ws_prop
            }),
            &["locator"],
        ),
        tool_descriptor(
            TOOL_CLOCK_OUT,
            "Close an active running CLOCK entry and record its elapsed H:MM duration.",
            json!({
                "locator": { "type": "string", "description": "Optional OrgLocator identifying the heading entry with an open clock." },
                "at": { "type": "string", "description": "Optional end timestamp (YYYY-MM-DDTHH:MM, YYYY-MM-DD HH:MM, or [YYYY-MM-DD Day HH:MM])." },
                "dry_run": { "type": "boolean", "description": "Preview clock-out mutation without writing files." },
                "workspace": ws_prop
            }),
            &[],
        ),
    ]
}

// endregion

// region Confinement & Execution Helpers

/// Collects `.org` files beneath `display_ws` that remain strictly confined inside `canon_ws`.
fn collect_confined_workspace_files(
    display_ws: &Path,
    canon_ws: &Path,
) -> Result<Vec<PathBuf>, String> {
    let files: Vec<PathBuf> = collect_org_files(&[display_ws.to_path_buf()])
        .into_iter()
        .filter(|p| {
            verify_existing_confined_org_file(p, canon_ws, "").is_ok()
                && verify_confined_mutation_target(p, canon_ws).is_ok()
        })
        .collect();
    if files.is_empty() {
        return Err("no confined .org files found in workspace".to_string());
    }
    Ok(files)
}

/// Parses and confines an `OrgLocator` string relative to `(display_ws, canon_ws)`.
fn confine_clock_locator(raw: &str, display_ws: &Path, canon_ws: &Path) -> Result<String, String> {
    let parsed = parse_clock_locator(raw)?;
    let confined = confine_and_rewrite_locator(&parsed, display_ws, canon_ws)?;
    Ok(confined.to_string())
}

/// Executes `org_clock_in` over MCP `stdio`, enforcing confinement and invalidating `MabuseCache`.
pub(super) fn exec_clock_in(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let allowed = &["locator", "at", "auto_out", "dry_run", "workspace"];
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, allowed)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let canon_ws = canonical_workspace_root(&norm_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let raw_loc = require_non_empty_str(args, "locator")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let confined_loc = confine_clock_locator(&raw_loc, &display_ws, &canon_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let at = optional_str(args, "at").map_err(|e| (e, session.empty_telemetry(started)))?;
        let auto_out = optional_bool(args, "auto_out", false)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let dry_run = optional_bool(args, "dry_run", false)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let files = collect_confined_workspace_files(&display_ws, &canon_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;

        let outcome =
            execute_clock_in_on_files(&files, &confined_loc, at.as_deref(), auto_out, dry_run);
        if let (Ok((_, ref modified)), false) = (&outcome, dry_run) {
            session
                .invalidate_workspace_paths(&norm_ws, is_default, modified)
                .map_err(|e| (e, session.empty_telemetry(started)))?;
        }
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;
        let (mut mutation, _) = outcome.map_err(|msg| (msg, tel))?;
        mutation.telemetry = session.counts_telemetry(
            started,
            (
                mutation.telemetry.files_scanned,
                mutation.telemetry.cache_hits,
                mutation.telemetry.cache_misses,
            ),
        );
        Ok(serde_json::to_value(mutation).unwrap_or_default())
    })())
}

/// Validates and confines an optional `locator` argument for `org_clock_out`.
fn parse_optional_clock_locator(
    args: &Map<String, Value>,
    display_ws: &Path,
    canon_ws: &Path,
) -> Result<Option<String>, String> {
    match optional_str(args, "locator")? {
        Some(ref s) if s.trim().is_empty() => Err("argument 'locator' must not be empty".into()),
        Some(ref s) => confine_clock_locator(s, display_ws, canon_ws).map(Some),
        None => Ok(None),
    }
}

/// Executes `org_clock_out` over MCP `stdio`, enforcing confinement and invalidating `MabuseCache`.
pub(super) fn exec_clock_out(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let allowed = &["locator", "at", "dry_run", "workspace"];
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, allowed)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let canon_ws = canonical_workspace_root(&norm_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let confined_loc = parse_optional_clock_locator(args, &display_ws, &canon_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let at = optional_str(args, "at").map_err(|e| (e, session.empty_telemetry(started)))?;
        let dry_run = optional_bool(args, "dry_run", false)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let files = collect_confined_workspace_files(&display_ws, &canon_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let outcome = terminate_active_clock(
            &files,
            confined_loc.as_deref(),
            at.as_deref(),
            false,
            dry_run,
        );
        if let (Ok((_, ref modified)), false) = (&outcome, dry_run) {
            session
                .invalidate_workspace_paths(&norm_ws, is_default, modified)
                .map_err(|e| (e, session.empty_telemetry(started)))?;
        }
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;
        let (mut mutation, _) = outcome.map_err(|msg| (msg, tel))?;
        mutation.telemetry = session.counts_telemetry(
            started,
            (
                mutation.telemetry.files_scanned,
                mutation.telemetry.cache_hits,
                mutation.telemetry.cache_misses,
            ),
        );
        Ok(serde_json::to_value(mutation).unwrap_or_default())
    })())
}

// endregion
