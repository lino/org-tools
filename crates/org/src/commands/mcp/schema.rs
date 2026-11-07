// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP protocol constants, `tools/list` JSON Schemas, argument validators, and envelopes.

use serde_json::{json, Map, Value};

use crate::commands::MutationTelemetry;

// region Constants & Tool Identifiers

/// Default MCP protocol revision negotiated when the client omits `protocolVersion`.
pub(super) const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Canonical `serverInfo.name` advertised during MCP `initialize`.
pub(super) const SERVER_NAME: &str = "org-tools";

/// JSON-RPC 2.0 parse error code (`-32700`).
pub(super) const ERR_PARSE_ERROR: i64 = -32700;

/// JSON-RPC 2.0 invalid request error code (`-32600`).
pub(super) const ERR_INVALID_REQUEST: i64 = -32600;

/// JSON-RPC 2.0 method not found error code (`-32601`).
pub(super) const ERR_METHOD_NOT_FOUND: i64 = -32601;

/// JSON-RPC 2.0 invalid params error code (`-32602`).
pub(super) const ERR_INVALID_PARAMS: i64 = -32602;

/// MCP tool name resolving `org-roam` v2 nodes by ID, title, alias, or locator.
pub(super) const TOOL_ROAM_NODE: &str = "org_roam_node";

/// MCP tool name listing incoming `[[id:...]]` backlinks for a target node.
pub(super) const TOOL_ROAM_BACKLINKS: &str = "org_roam_backlinks";

/// MCP tool name searching headings and FTS5 BM25 bodies across the workspace.
pub(super) const TOOL_ROAM_SEARCH: &str = "org_roam_search";

/// MCP tool name listing actionable GTD next tasks with optional `@context` filter.
pub(super) const TOOL_QUERY_NEXT: &str = "org_query_next";

/// MCP tool name listing entries blocked by unresolved `org-edna` `:BLOCKER:` rules.
pub(super) const TOOL_QUERY_BLOCKED: &str = "org_query_blocked";

/// MCP tool name extracting a bounded subtree or file node in `json`, `org`, or `md`.
pub(super) const TOOL_SHOW: &str = "org_show";

/// MCP tool name transitioning TODO states and executing `:TRIGGER:` cascades.
pub(super) const TOOL_UPDATE_STATE: &str = "org_update_state";

/// MCP tool name checking or formatting an Org file in-place within the workspace.
pub(super) const TOOL_FMT_FILE: &str = "org_fmt_file";

/// Ordered registry of all supported MCP tool names.
pub(super) const ALL_TOOL_NAMES: &[&str] = &[
    TOOL_ROAM_NODE,
    TOOL_ROAM_BACKLINKS,
    TOOL_ROAM_SEARCH,
    TOOL_QUERY_NEXT,
    TOOL_QUERY_BLOCKED,
    TOOL_SHOW,
    TOOL_UPDATE_STATE,
    TOOL_FMT_FILE,
];

// endregion

// region Tool Schema Descriptors

/// Builds a single MCP tool descriptor with `additionalProperties: false`.
fn tool_descriptor(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
    })
}

/// Builds the 4 `org-roam` and GTD query tool descriptors (`org_roam_*`, `org_query_*`).
fn roam_and_query_descriptors() -> Vec<Value> {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    let limit_prop = json!({ "type": "integer", "minimum": 1, "description": "Maximum number of entries to return." });
    vec![
        tool_descriptor(
            TOOL_ROAM_NODE,
            "Resolve org-roam v2 nodes by id:<uuid>, :ID:, title, alias, or OrgLocator.",
            json!({
                "query": { "type": "string", "description": "Node query (id:<uuid>, :ID:, title, alias, or OrgLocator)." },
                "workspace": ws_prop
            }),
            &["query"],
        ),
        tool_descriptor(
            TOOL_ROAM_BACKLINKS,
            "List incoming [[id:...]] backlinks pointing to a resolved org-roam v2 target node.",
            json!({
                "target": { "type": "string", "description": "Target node (id:<uuid>, :ID:, title, alias, or OrgLocator)." },
                "workspace": ws_prop
            }),
            &["target"],
        ),
        tool_descriptor(
            TOOL_ROAM_SEARCH,
            "Search Org entries via query syntax or warm FTS5 BM25 full-text body matching.",
            json!({
                "query": { "type": "string", "description": "Query expression (e.g. body:\"term\", todo:NEXT) or free-text phrase." },
                "limit": limit_prop,
                "workspace": ws_prop
            }),
            &["query"],
        ),
        tool_descriptor(
            TOOL_QUERY_NEXT,
            "List actionable (unblocked TODO) entries with optional @context tag filtering.",
            json!({
                "context": { "type": "string", "description": "Optional @context tag filter (e.g. @code)." },
                "limit": limit_prop,
                "workspace": ws_prop
            }),
            &[],
        ),
    ]
}

/// Builds the remaining 4 query, show, and write tool descriptors.
fn inspect_and_write_descriptors() -> Vec<Value> {
    let ws_prop =
        json!({ "type": "string", "description": "Optional workspace root directory override." });
    let limit_prop = json!({ "type": "integer", "minimum": 1, "description": "Maximum number of entries to return." });
    vec![
        tool_descriptor(
            TOOL_QUERY_BLOCKED,
            "List entries currently blocked by org-edna :BLOCKER: conditions with blocker details.",
            json!({ "limit": limit_prop, "workspace": ws_prop }),
            &[],
        ),
        tool_descriptor(
            TOOL_SHOW,
            "Extract a bounded single-node Org subtree or file node in json, org, or md format.",
            json!({
                "locator": { "type": "string", "description": "OrgLocator (id:<uuid>, <file>::#<id>, <file>::*/Path, or <file>::<line>)." },
                "format": { "type": "string", "enum": ["org", "json", "md"], "description": "Slice format (defaults to json)." },
                "no_children": { "type": "boolean", "description": "Omit descendant headings." },
                "max_chars": { "type": "integer", "minimum": 1, "description": "Maximum Unicode scalar characters in content." },
                "workspace": ws_prop
            }),
            &["locator"],
        ),
        tool_descriptor(
            TOOL_UPDATE_STATE,
            "Transition an entry TODO state, execute cross-file :TRIGGER: cascades, and refresh cache.",
            json!({
                "locator": { "type": "string", "description": "OrgLocator identifying the target heading entry." },
                "state": { "type": "string", "description": "Target TODO keyword (e.g. NEXT, DONE, TODO)." },
                "dry_run": { "type": "boolean", "description": "Preview state and cascade mutations without writing files." },
                "workspace": ws_prop
            }),
            &["locator", "state"],
        ),
        tool_descriptor(
            TOOL_FMT_FILE,
            "Check or format an Org file within the workspace using F-series and lint rules.",
            json!({
                "path": { "type": "string", "description": "Workspace-relative or in-workspace .org file path." },
                "check_only": { "type": "boolean", "description": "Report diagnostics and formatting status without modifying disk." },
                "workspace": ws_prop
            }),
            &["path"],
        ),
    ]
}

/// Constructs the complete `tools/list` result object for all 8 MCP tools.
pub(super) fn build_tools_list_result() -> Value {
    let mut tools = roam_and_query_descriptors();
    tools.extend(inspect_and_write_descriptors());
    json!({ "tools": tools })
}

// endregion

// region Argument Validators

/// Rejects unexpected keys in `args` that are not listed in `allowed`.
pub(super) fn reject_unknown_keys(
    args: &Map<String, Value>,
    allowed: &[&str],
) -> Result<(), String> {
    for key in args.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unexpected argument '{key}'"));
        }
    }
    Ok(())
}

/// Extracts a required non-empty string argument `key` from `args`.
pub(super) fn require_non_empty_str(
    args: &Map<String, Value>,
    key: &str,
) -> Result<String, String> {
    let Some(val) = args.get(key) else {
        return Err(format!("missing required string argument '{key}'"));
    };
    let Some(s) = val.as_str() else {
        return Err(format!("argument '{key}' must be a string"));
    };
    if s.trim().is_empty() {
        return Err(format!("argument '{key}' must not be empty"));
    }
    Ok(s.to_string())
}

/// Extracts an optional string argument `key` from `args` (treating `null` as `None`).
pub(super) fn optional_str(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    let Some(val) = args.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_str()
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| format!("argument '{key}' must be a string"))
}

/// Extracts an optional boolean argument `key` from `args`, defaulting to `default`.
pub(super) fn optional_bool(
    args: &Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    let Some(val) = args.get(key) else {
        return Ok(default);
    };
    if val.is_null() {
        return Ok(default);
    }
    val.as_bool()
        .ok_or_else(|| format!("argument '{key}' must be a boolean"))
}

/// Extracts an optional positive (`>= 1`) integer argument `key` from `args`.
pub(super) fn optional_positive_usize(
    args: &Map<String, Value>,
    key: &str,
) -> Result<Option<usize>, String> {
    let Some(val) = args.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    let Some(n) = val.as_u64() else {
        return Err(format!("argument '{key}' must be a positive integer"));
    };
    if n == 0 {
        return Err(format!("argument '{key}' must be >= 1"));
    }
    Ok(Some(n as usize))
}

// endregion

// region Response Envelopes

/// Builds a JSON-RPC 2.0 success response object.
pub(super) fn jsonrpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Builds a JSON-RPC 2.0 error response object.
pub(super) fn jsonrpc_err(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

/// Wraps a structured tool payload into a standard MCP `CallToolResult` (`isError: false`).
pub(super) fn call_tool_ok(structured: Value) -> Value {
    let text = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

/// Wraps a domain/argument error and `telemetry` into an MCP `CallToolResult` (`isError: true`).
pub(super) fn call_tool_err(message: impl Into<String>, telemetry: MutationTelemetry) -> Value {
    let structured = json!({
        "error": message.into(),
        "telemetry": telemetry
    });
    let text = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": true
    })
}

/// Converts a `Result<Value, (String, MutationTelemetry)>` into an MCP `CallToolResult`.
pub(super) fn finish_tool_call(res: Result<Value, (String, MutationTelemetry)>) -> Value {
    match res {
        Ok(structured) => call_tool_ok(structured),
        Err((msg, tel)) => call_tool_err(msg, tel),
    }
}

// endregion
