// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP `org_fleeting_list` tool handler for discovering pending or refined fleeting notes.

use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::config::Config;
use org_tools_core::fleeting::{scan_fleeting_items, FleetingStatusFilter};

use super::super::schema::{finish_tool_call, optional_str};
use super::super::McpSession;

/// Parses the optional status string into a [`FleetingStatusFilter`].
fn parse_fleeting_status(raw: Option<&str>) -> Result<FleetingStatusFilter, String> {
    match raw {
        None | Some("unrefined") => Ok(FleetingStatusFilter::Unrefined),
        Some("reformed") => Ok(FleetingStatusFilter::Reformed),
        Some("converted-to-task") => Ok(FleetingStatusFilter::ConvertedToTask),
        Some("discarded") => Ok(FleetingStatusFilter::Discarded),
        Some("all") => Ok(FleetingStatusFilter::All),
        Some(other) => Err(format!("invalid fleeting status filter: '{other}'")),
    }
}

/// Executes `org_fleeting_list` returning structured discovery of fleeting notes.
pub(crate) fn exec_fleeting_list(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (_display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, &["workspace", "roam_dir", "status"])
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let status_str =
            optional_str(args, "status").map_err(|e| (e, session.empty_telemetry(started)))?;
        let filter = parse_fleeting_status(status_str.as_deref())
            .map_err(|e| (e, session.empty_telemetry(started)))?;

        let cfg = Config::load_for_dir(&norm_ws)
            .map_err(|e| (e.to_string(), session.empty_telemetry(started)))?;
        let roots = if let Some(sel) =
            optional_str(args, "roam_dir").map_err(|e| (e, session.empty_telemetry(started)))?
        {
            let (_, p) = cfg.resolve_roam_dir(&sel).ok_or_else(|| {
                (
                    format!("unknown roam directory: {sel}"),
                    session.empty_telemetry(started),
                )
            })?;
            if !p.exists() {
                return Err((
                    format!("roam directory not found: {}", p.display()),
                    session.empty_telemetry(started),
                ));
            }
            vec![p]
        } else {
            let configured = cfg.roam_directories();
            let valid: Vec<_> = configured
                .into_iter()
                .map(|(_, p)| p)
                .filter(|p| p.exists())
                .collect();
            if valid.is_empty() {
                vec![norm_ws.clone()]
            } else {
                valid
            }
        };

        let items = scan_fleeting_items(&roots, filter);
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;

        Ok(json!({ "count": items.len(), "items": items, "telemetry": tel }))
    })())
}
