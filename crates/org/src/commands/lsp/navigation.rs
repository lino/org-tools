// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! LSP navigation dispatch stubs for symbols, links, definitions, and references.

use serde_json::Value;

use super::LspSession;

/// Handles incoming LSP navigation requests.
pub(crate) fn handle_navigation_request(
    _session: &mut LspSession,
    method: &str,
    _params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/documentSymbol"
        | "workspace/symbol"
        | "textDocument/documentLink"
        | "textDocument/definition"
        | "textDocument/references"
        | "textDocument/foldingRange" => Ok(Value::Array(Vec::new())),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}
