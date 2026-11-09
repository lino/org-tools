// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! LSP rename preparation, execution, and semantic token stubs.

use serde_json::{json, Value};

use super::LspSession;

/// Handles incoming LSP rename and semantic token requests.
pub(crate) fn handle_refactor_tokens_request(
    _session: &mut LspSession,
    method: &str,
    _params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/prepareRename" => Ok(Value::Null),
        "textDocument/rename" => Ok(json!({ "changes": {} })),
        "textDocument/semanticTokens/full" => Ok(json!({ "data": [] })),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}
