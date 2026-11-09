// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! LSP completion and hover intelligence dispatch stubs.

use serde_json::{json, Value};

use super::LspSession;

/// Handles incoming LSP intelligence requests (`completion`, `hover`).
pub(crate) fn handle_intelligence_request(
    _session: &mut LspSession,
    method: &str,
    _params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/completion" => Ok(json!({
            "isIncomplete": false,
            "items": []
        })),
        "textDocument/hover" => Ok(Value::Null),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}
