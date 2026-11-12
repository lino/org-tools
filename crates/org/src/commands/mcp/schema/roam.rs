// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP tool schema descriptors and argument extraction for Roam and GTD workflows.

use serde_json::{json, Map, Value};

use super::tool_descriptor;

// region Constants & Tool Identifiers

/// MCP tool name capturing notes headlessly via built-in or configured templates.
pub(crate) const TOOL_ROAM_CAPTURE: &str = "org_roam_capture";

/// MCP tool name listing unrefined or processed fleeting notes.
pub(crate) const TOOL_FLEETING_LIST: &str = "org_fleeting_list";

/// MCP tool name transforming a fleeting note into a Zettel, GTD task, or discarded state.
pub(crate) const TOOL_FLEETING_TRANSFORM: &str = "org_fleeting_transform";

// endregion

// region Roam Workflow Tool Descriptors

/// Builds the upgraded `org_query_next` descriptor supporting multi-vault GTD filtering.
pub(crate) fn query_next_descriptor() -> Value {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    let limit_prop = json!({ "type": "integer", "minimum": 1, "description": "Maximum number of entries to return." });
    let roam_dir_prop = json!({
        "oneOf": [
            { "type": "string" },
            { "type": "array", "items": { "type": "string" } }
        ],
        "description": "Optional named vault selector or list of vault names/directories."
    });
    let tags_prop = json!({
        "oneOf": [
            { "type": "string" },
            { "type": "array", "items": { "type": "string" } }
        ],
        "description": "Optional required tags filter (comma-delimited string or array of strings)."
    });

    tool_descriptor(
        super::TOOL_QUERY_NEXT,
        "List actionable GTD next actions with multi-vault routing, energy, effort, and pick options.",
        json!({
            "workspace": ws_prop,
            "roam_dir": roam_dir_prop,
            "paths": { "type": "array", "items": { "type": "string" }, "description": "Optional explicit paths to scan." },
            "project": { "type": "string", "description": "Filter by owning GTD project title, :ID:, alias, or file stem." },
            "context": { "type": "string", "description": "Filter by @context tag or :CONTEXT: property (e.g. @code)." },
            "energy": { "type": "string", "description": "Filter by :ENERGY: level (low, medium, high)." },
            "workmode": { "type": "string", "description": "Filter by :WORKMODE: (deep, shallow, any)." },
            "max_effort": { "type": "string", "description": "Maximum :EFFORT: duration (e.g. '1:00', '45m')." },
            "min_effort": { "type": "string", "description": "Minimum :EFFORT: duration (e.g. '15m')." },
            "priority": { "type": "string", "description": "Priority filter (A, B, C, or >=B)." },
            "tags": tags_prop,
            "goal": { "type": "string", "description": "Filter by :GOAL: property substring or ID." },
            "due": { "type": "string", "description": "Filter by planning due date (overdue, today, soon, or YYYY-MM-DD)." },
            "next_only": { "type": "boolean", "description": "Only return entries whose keyword is explicitly NEXT." },
            "pick": { "type": "boolean", "description": "Recommend the single highest-ranked next action (or top limit)." },
            "limit": limit_prop
        }),
        &[],
    )
}

/// Builds the `org_roam_capture` tool descriptor.
fn roam_capture_descriptor() -> Value {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    let tags_prop = json!({
        "oneOf": [
            { "type": "string" },
            { "type": "array", "items": { "type": "string" } }
        ],
        "description": "Optional filetags or heading tags (comma string or array of strings)."
    });
    tool_descriptor(
        TOOL_ROAM_CAPTURE,
        "Headlessly capture a new Zettel, fleeting note, GTD project, or task without an interactive editor.",
        json!({
            "title": { "type": "string", "description": "Note or entry title." },
            "workspace": ws_prop,
            "roam_dir": { "type": "string", "description": "Target org-roam vault name or directory path." },
            "template": { "type": "string", "description": "Template name from [capture.templates.<name>]." },
            "type": { "type": "string", "description": "Built-in note type (zettel, concept, fleeting, project, inbox)." },
            "slug": { "type": "string", "description": "Explicit filename slug override." },
            "tags": tags_prop,
            "aliases": { "type": "array", "items": { "type": "string" }, "description": "Repeatable :ROAM_ALIASES: tokens." },
            "refs": { "type": "array", "items": { "type": "string" }, "description": "Repeatable :ROAM_REFS: tokens." },
            "medium": { "type": "string", "description": "Source medium node query or citation string." },
            "location": { "type": "string", "description": "Source location string (page, chapter, or section)." },
            "parent": { "type": "string", "description": "Parent Zettel node query for Folgezettel linking." },
            "links": { "type": "array", "items": { "type": "string" }, "description": "Related node queries linked under Related Concepts." },
            "body": { "type": "string", "description": "Inline note body text." },
            "dry_run": { "type": "boolean", "description": "Preview capture without writing files to disk." }
        }),
        &["title"],
    )
}

/// Builds the `org_fleeting_list` tool descriptor.
fn fleeting_list_descriptor() -> Value {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    tool_descriptor(
        TOOL_FLEETING_LIST,
        "List pending or refined Luhmann fleeting notes across workspace vaults.",
        json!({
            "workspace": ws_prop,
            "roam_dir": { "type": "string", "description": "Target roam vault selector." },
            "status": {
                "type": "string",
                "enum": ["unrefined", "reformed", "converted-to-task", "discarded", "all"],
                "description": "Filter by fleeting note lifecycle status (defaults to unrefined)."
            }
        }),
        &[],
    )
}

/// Builds the `org_fleeting_transform` tool descriptor.
fn fleeting_transform_descriptor() -> Value {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    tool_descriptor(
        TOOL_FLEETING_TRANSFORM,
        "Headlessly transform a fleeting note into a permanent Zettel, GTD inbox task, or discarded state.",
        json!({
            "target": { "type": "string", "description": "Target fleeting note locator, title, or :ID:." },
            "action": {
                "type": "string",
                "enum": ["to-zettel", "to-task", "discard"],
                "description": "Transformation action to perform."
            },
            "workspace": ws_prop,
            "roam_dir": { "type": "string", "description": "Target roam vault selector." },
            "dry_run": { "type": "boolean", "description": "Preview transformation without writing files." },
            "title": { "type": "string", "description": "Title override for reformed Zettel or task." },
            "slug": { "type": "string", "description": "Filename slug override for reformed Zettel." },
            "parent": { "type": "string", "description": "Parent Zettel node query for Folgezettel link." },
            "links": { "type": "array", "items": { "type": "string" }, "description": "Related node queries linked to reformed Zettel." },
            "body": { "type": "string", "description": "Body content override for reformed Zettel." },
            "to": { "type": "string", "description": "Destination file path for task conversion (defaults to <roam_dir>/<gtd_subdir>/inbox.org)." },
            "context": { "type": "string", "description": "@context tag override for converted task." },
            "effort": { "type": "string", "description": ":EFFORT: duration for converted task." },
            "scheduled": { "type": "string", "description": "SCHEDULED timestamp for converted task." },
            "deadline": { "type": "string", "description": "DEADLINE timestamp for converted task." },
            "delete": { "type": "boolean", "description": "Delete fleeting note file on discard (default: false)." }
        }),
        &["target", "action"],
    )
}

/// Returns the 3 roam capture and fleeting workflow tool descriptors.
pub(crate) fn roam_workflow_descriptors() -> Vec<Value> {
    vec![
        roam_capture_descriptor(),
        fleeting_list_descriptor(),
        fleeting_transform_descriptor(),
    ]
}

// endregion

// region Argument Helpers

/// Extracts an optional string or array of strings argument `key` from `args`.
pub(crate) fn optional_str_or_array(
    args: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(val) = args.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let items: Vec<String> = trimmed
            .split([',', ':'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        return Ok(if items.is_empty() { None } else { Some(items) });
    }
    if let Some(arr) = val.as_array() {
        if arr.is_empty() {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let Some(s) = item.as_str().filter(|s| !s.trim().is_empty()) else {
                return Err(format!("argument '{key}' must contain non-empty strings"));
            };
            out.push(s.to_string());
        }
        return Ok(Some(out));
    }
    Err(format!(
        "argument '{key}' must be a string or array of strings"
    ))
}

// endregion
