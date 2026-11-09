// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=760 reason="Cohesive LSP diagnostics publisher, formatter, telemetry, and actions"

//! LSP diagnostics publication, document formatting, and code action quick-fixes.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use org_tools_core::diagnostic::{Diagnostic, Severity};
use org_tools_core::document::OrgDocument;
use org_tools_core::entry::{parse_checklist_item, CheckboxState};
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::property::{
    add_deterministic_ids, DeterministicIdConfig, DeterministicIdOptions,
};
use org_tools_core::runner::Runner;
use org_tools_core::source::SourceFile;
use org_tools_core::state::set_state;

use super::{
    byte_offset_to_utf16_pos, file_uri_to_path, line_col_1based_to_utf16_pos, lsp_range,
    span_to_lsp_range, utf16_pos_to_byte_offset, LspSession, OpenDocument,
};
use crate::commands::mcp::framing::write_frame;

// region Telemetry Recording

/// Formats the current UTC timestamp as an ISO-8601 string.
fn iso8601_now() -> String {
    let now = std::time::SystemTime::now();
    let epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (epoch / 86400) as i64;
    let rem = (epoch % 86400) as u32;
    let (y, m, d) = crate::date::days_to_date(days + crate::date::date_to_days(1970, 1, 1));
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Appends a structured JSONL event line to `<workspace>/.cache/telemetry/events.jsonl`.
fn append_event(workspace: &Path, method: &str, status: &str, dur: f64) {
    let dir = std::env::var_os("ORG_LSP_TELEMETRY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join(".cache/telemetry"));
    let _ = fs::create_dir_all(&dir);
    let trace_id = std::env::var("FRITZ_TRACE_ID").unwrap_or_else(|_| "untraced".to_string());
    let payload = json!({
        "timestamp": iso8601_now(),
        "event": "lsp.request",
        "method": method,
        "status": status,
        "duration_seconds": dur,
        "trace_id": trace_id
    });
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("events.jsonl"))
    {
        let _ = writeln!(f, "{payload}");
    }
}

/// Flushes Prometheus textfile metrics to `<workspace>/.cache/telemetry/orgfmt_lsp.prom`.
pub(crate) fn flush_prom(session: &LspSession) {
    let dir = std::env::var_os("ORG_LSP_TELEMETRY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| session.default_workspace.join(".cache/telemetry"));
    let _ = fs::create_dir_all(&dir);
    let mut out = String::from(
        "# HELP orgfmt_lsp_requests_total Total JSON-RPC LSP requests processed.\n# TYPE orgfmt_lsp_requests_total counter\n",
    );
    for ((m, s), count) in &session.telemetry.requests_total {
        out.push_str(&format!(
            "orgfmt_lsp_requests_total{{method=\"{m}\",status=\"{s}\"}} {count}\n"
        ));
    }
    out.push_str(
        "# HELP orgfmt_lsp_request_duration_seconds LSP request duration in seconds.\n# TYPE orgfmt_lsp_request_duration_seconds gauge\n",
    );
    for (m, dur) in &session.telemetry.request_durations {
        out.push_str(&format!(
            "orgfmt_lsp_request_duration_seconds{{method=\"{m}\"}} {dur:.6}\n"
        ));
    }
    out.push_str(
        "# HELP orgfmt_lsp_diagnostics_published_total Total diagnostics published by severity.\n# TYPE orgfmt_lsp_diagnostics_published_total counter\n",
    );
    for (sev, count) in &session.telemetry.diagnostics_published_total {
        out.push_str(&format!(
            "orgfmt_lsp_diagnostics_published_total{{severity=\"{sev}\"}} {count}\n"
        ));
    }
    let _ = fs::write(dir.join("orgfmt_lsp.prom"), out);
}

/// Records request execution metrics and appends a JSONL trace event.
pub(crate) fn record_request(session: &mut LspSession, method: &str, status: &str, dur: f64) {
    *session
        .telemetry
        .requests_total
        .entry((method.to_string(), status.to_string()))
        .or_insert(0) += 1;
    session
        .telemetry
        .request_durations
        .insert(method.to_string(), dur);
    append_event(&session.default_workspace, method, status, dur);
    flush_prom(session);
}

/// Records published diagnostics metrics.
pub(crate) fn record_diagnostics(session: &mut LspSession, severity: &str, count: u64) {
    *session
        .telemetry
        .diagnostics_published_total
        .entry(severity.to_string())
        .or_insert(0) += count;
    flush_prom(session);
}

// endregion

// region Diagnostics & Store Notifications

/// Recalculates all `#+TBLFM:` spreadsheet formulas within `content`.
pub(crate) fn evaluate_tables(content: &str, constants: &HashMap<String, String>) -> String {
    org_tools_core::tblfm::calc_file(content, constants).content
}

/// Converts an internal [`Diagnostic`] into an LSP `Diagnostic` JSON object.
fn build_lsp_diagnostic(source: &SourceFile, diag: &Diagnostic) -> Value {
    let severity_num = match diag.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
    };
    let range = if let Some(ref fix) = diag.fix {
        span_to_lsp_range(source, fix.span.clone())
    } else {
        let (s_line, s_char) = line_col_1based_to_utf16_pos(source, diag.line, diag.column);
        let line_idx = diag.line.saturating_sub(1);
        let end_char = if line_idx < source.line_count() {
            let s = source.line_start(line_idx);
            let e = if line_idx + 1 < source.line_count() {
                source.line_start(line_idx + 1)
            } else {
                source.content.len()
            };
            let text = &source.content[s..e];
            let trimmed = text.trim_end_matches(['\r', '\n']);
            let len: usize = trimmed.chars().map(|c| c.len_utf16()).sum();
            (len as u32).max(s_char + 1)
        } else {
            s_char + 1
        };
        lsp_range(s_line, s_char, s_line, end_char)
    };
    json!({
        "range": range,
        "severity": severity_num,
        "code": diag.rule_id,
        "source": "org",
        "message": diag.message
    })
}

/// Computes all LSP diagnostics for `source` using workspace context from `session`.
pub(crate) fn compute_diagnostics_for_doc(session: &LspSession, source: &SourceFile) -> Vec<Value> {
    let config = session.effective_config_for(&source.path);
    let runner = Runner::new(config);
    let all_ws = session.collect_workspace_documents();
    let ws_docs: Vec<OrgDocument> = all_ws.into_iter().map(|(_, _, d)| d).collect();
    let diags = runner.check_with_workspace(source, &ws_docs);
    diags
        .iter()
        .map(|d| build_lsp_diagnostic(source, d))
        .collect()
}

/// Emits `textDocument/publishDiagnostics` and records telemetry counts.
pub(crate) fn publish_document_diagnostics<W: Write>(
    session: &mut LspSession,
    writer: &mut W,
    uri: &str,
    source: &SourceFile,
) {
    let diags = compute_diagnostics_for_doc(session, source);
    let mut sev_counts: HashMap<&str, u64> = HashMap::new();
    for d in &diags {
        let sev = match d["severity"].as_u64().unwrap_or(3) {
            1 => "error",
            2 => "warning",
            _ => "info",
        };
        *sev_counts.entry(sev).or_insert(0) += 1;
    }
    for (sev, count) in sev_counts {
        record_diagnostics(session, sev, count);
    }
    let notif = json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diags }
    });
    let _ = write_frame(writer, session.framing_mode, &notif);
}

/// Applies sequential full or incremental content change items to an existing document.
pub(crate) fn apply_content_changes(source: &SourceFile, changes: &[Value]) -> String {
    let mut content = source.content.clone();
    for change in changes {
        let text = change.get("text").and_then(|t| t.as_str()).unwrap_or("");
        if let Some(range) = change.get("range") {
            let s_l = range["start"]["line"].as_u64().unwrap_or(0) as u32;
            let s_c = range["start"]["character"].as_u64().unwrap_or(0) as u32;
            let e_l = range["end"]["line"].as_u64().unwrap_or(0) as u32;
            let e_c = range["end"]["character"].as_u64().unwrap_or(0) as u32;
            let cur_src = SourceFile::new(&source.path, content.clone());
            let s_b = utf16_pos_to_byte_offset(&cur_src, s_l, s_c).unwrap_or(0);
            let e_b = utf16_pos_to_byte_offset(&cur_src, e_l, e_c).unwrap_or(content.len());
            let safe_s = s_b.min(content.len());
            let safe_e = e_b.min(content.len()).max(safe_s);
            content.replace_range(safe_s..safe_e, text);
        } else {
            content = text.to_string();
        }
    }
    content
}

/// Handles incoming `textDocument/didOpen` notification.
fn handle_did_open<W: Write>(session: &mut LspSession, writer: &mut W, params: &Value) {
    if let Ok((uri, path)) = session.resolve_uri_and_path(params) {
        let text = params["textDocument"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let version = params["textDocument"]["version"].as_i64().unwrap_or(1);
        let source = SourceFile::new(&path, text);
        let doc = OrgDocument::from_source(&source);
        publish_document_diagnostics(session, writer, &uri, &source);
        session.open_docs.insert(
            path.clone(),
            OpenDocument {
                path,
                uri,
                version,
                source,
                doc,
            },
        );
    }
}

/// Handles incoming `textDocument/didChange` notification with incremental or full changes.
fn handle_did_change<W: Write>(session: &mut LspSession, writer: &mut W, params: &Value) {
    if let Ok((uri, path)) = session.resolve_uri_and_path(params) {
        if let Some(open) = session.open_docs.get_mut(&path) {
            if let Some(changes) = params.get("contentChanges").and_then(|c| c.as_array()) {
                let new_text = apply_content_changes(&open.source, changes);
                open.version = params["textDocument"]["version"]
                    .as_i64()
                    .unwrap_or(open.version + 1);
                open.source = SourceFile::new(&path, new_text);
                open.doc = OrgDocument::from_source(&open.source);
                let source_clone = SourceFile::new(&open.path, open.source.content.clone());
                publish_document_diagnostics(session, writer, &uri, &source_clone);
            }
        }
    }
}

/// Handles incoming `textDocument/didClose` notification, clearing overlay and diagnostics.
fn handle_did_close<W: Write>(session: &mut LspSession, writer: &mut W, params: &Value) {
    if let Ok((uri, path)) = session.resolve_uri_and_path(params) {
        session.open_docs.remove(&path);
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": [] }
        });
        let _ = write_frame(writer, session.framing_mode, &notif);
    }
}

/// Handles incoming document synchronization notifications (`didOpen`, `didChange`, `didClose`).
pub(crate) fn handle_doc_notification<W: Write>(
    session: &mut LspSession,
    writer: &mut W,
    method: &str,
    params: &Value,
) {
    match method {
        "textDocument/didOpen" => handle_did_open(session, writer, params),
        "textDocument/didChange" => handle_did_change(session, writer, params),
        "textDocument/didClose" => handle_did_close(session, writer, params),
        _ => {}
    }
}

// endregion

// region Formatting & Code Actions

/// Handles `textDocument/formatting` by running `#+TBLFM:` calc and runner formatting.
fn handle_formatting(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let content_calc = evaluate_tables(&source.content, &doc.table_constants);
    let source_calc = SourceFile::new(&path, content_calc);

    let config = session.effective_config_for(&path);
    let runner = Runner::new(config);
    let all_ws = session.collect_workspace_documents();
    let ws_docs: Vec<OrgDocument> = all_ws.into_iter().map(|(_, _, d)| d).collect();

    let (formatted, _) = runner.format_with_workspace(&source_calc, &ws_docs);
    if formatted == source.content {
        return Ok(Value::Array(Vec::new()));
    }
    let (end_l, end_c) = byte_offset_to_utf16_pos(&source, source.content.len());
    let edit = json!({
        "range": lsp_range(0, 0, end_l, end_c),
        "newText": formatted
    });
    Ok(json!([edit]))
}

/// Handles `textDocument/rangeFormatting` by applying format fixes inside the range.
fn handle_range_formatting(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, _) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let range_obj = params
        .get("range")
        .ok_or_else(|| (-32602, "Missing range".to_string()))?;
    let s_line = range_obj["start"]["line"].as_u64().unwrap_or(0) as u32;
    let s_char = range_obj["start"]["character"].as_u64().unwrap_or(0) as u32;
    let e_line = range_obj["end"]["line"].as_u64().unwrap_or(0) as u32;
    let e_char = range_obj["end"]["character"].as_u64().unwrap_or(0) as u32;

    let start_byte = utf16_pos_to_byte_offset(&source, s_line, s_char).unwrap_or(0);
    let end_byte =
        utf16_pos_to_byte_offset(&source, e_line, e_char).unwrap_or(source.content.len());

    let config = session.effective_config_for(&path);
    let runner = Runner::new(config);
    let diags = runner.check(&source);

    let edits: Vec<Value> = diags
        .into_iter()
        .filter_map(|d| d.fix)
        .filter(|fix| fix.span.start >= start_byte && fix.span.end <= end_byte)
        .map(|fix| {
            json!({
                "range": span_to_lsp_range(&source, fix.span),
                "newText": fix.replacement
            })
        })
        .collect();

    Ok(Value::Array(edits))
}

/// Finds the index of the heading entry enclosing or matching `offset`.
fn find_entry_at_offset(doc: &OrgDocument, source: &SourceFile, offset: usize) -> Option<usize> {
    if doc.entries.is_empty() {
        return None;
    }
    let (target_line, _) = source.line_col(offset);
    doc.entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.heading_line <= target_line)
        .max_by_key(|(_, e)| e.heading_line)
        .map(|(i, _)| i)
}

/// Builds a `source.fixAll` code action if any auto-fixable formatting rules can clean `source`.
fn build_fix_all_action(runner: &Runner, source: &SourceFile, uri: &str) -> Option<Value> {
    let (formatted, _) = runner.format(source);
    if formatted != source.content {
        let (end_l, end_c) = byte_offset_to_utf16_pos(source, source.content.len());
        Some(json!({
            "title": "Fix all auto-fixable formatting issues",
            "kind": "source.fixAll",
            "edit": {
                "changes": {
                    uri: [
                        {
                            "range": lsp_range(0, 0, end_l, end_c),
                            "newText": formatted
                        }
                    ]
                }
            }
        }))
    } else {
        None
    }
}

/// Builds quick-fix code actions for any fixable diagnostics overlapping `[s_byte, e_byte]`.
fn collect_quick_fix_actions(
    source: &SourceFile,
    uri: &str,
    s_byte: usize,
    e_byte: usize,
) -> Vec<Value> {
    let runner = Runner::with_defaults();
    let diags = runner.check(source);
    let mut actions = Vec::new();
    let mut any_fixes = false;

    for diag in diags {
        if let Some(ref fix) = diag.fix {
            any_fixes = true;
            if fix.span.start <= e_byte && fix.span.end >= s_byte {
                actions.push(json!({
                    "title": format!("Fix: {} ({})", diag.rule, diag.rule_id),
                    "kind": "quickfix",
                    "isPreferred": true,
                    "edit": {
                        "changes": {
                            uri: [
                                {
                                    "range": span_to_lsp_range(source, fix.span.clone()),
                                    "newText": fix.replacement
                                }
                            ]
                        }
                    }
                }));
            }
        }
    }

    if any_fixes {
        if let Some(fix_all) = build_fix_all_action(&runner, source, uri) {
            actions.push(fix_all);
        }
    }
    actions
}

/// Builds a refactoring action to toggle a checklist item if present on `line_0`.
fn build_checklist_toggle_action(source: &SourceFile, uri: &str, line_0: u32) -> Option<Value> {
    let line_idx = line_0 as usize;
    if line_idx >= source.line_count() {
        return None;
    }
    let s = source.line_start(line_idx);
    let e = if line_idx + 1 < source.line_count() {
        source.line_start(line_idx + 1)
    } else {
        source.content.len()
    };
    let raw = &source.content[s..e];
    let (state, marker) = if raw.contains("[ ]") {
        (CheckboxState::Unchecked, "[ ]")
    } else if raw.contains("[X]") {
        (CheckboxState::Checked, "[X]")
    } else if raw.contains("[x]") {
        (CheckboxState::Checked, "[x]")
    } else if raw.contains("[-]") {
        (CheckboxState::Partial, "[-]")
    } else {
        return None;
    };
    let _ = parse_checklist_item(raw.trim_end_matches(['\r', '\n']), line_idx + 1)?;
    let new_marker = match state {
        CheckboxState::Checked => "[ ]",
        CheckboxState::Unchecked | CheckboxState::Partial => "[X]",
    };
    let marker_pos = raw.find(marker)?;
    let start_b = s + marker_pos;
    let end_b = start_b + marker.len();

    Some(json!({
        "title": "Toggle checklist item",
        "kind": "refactor.rewrite",
        "edit": {
            "changes": {
                uri: [
                    {
                        "range": span_to_lsp_range(source, org_tools_core::diagnostic::Span::new(start_b, end_b)),
                        "newText": new_marker
                    }
                ]
            }
        }
    }))
}

/// Builds a refactoring action to cycle the TODO keyword on an entry.
fn build_cycle_todo_action(
    source: &SourceFile,
    doc: &OrgDocument,
    uri: &str,
    entry_idx: usize,
) -> Option<Value> {
    let entry = doc.entries.get(entry_idx)?;
    let all_kw = doc.todo_keywords.all();
    let current_kw = entry.keyword.as_deref();

    let next_kw = match current_kw {
        None => all_kw.first().copied().unwrap_or("TODO"),
        Some(kw) => {
            if let Some(pos) = all_kw.iter().position(|&k| k == kw) {
                if pos + 1 < all_kw.len() {
                    all_kw[pos + 1]
                } else {
                    ""
                }
            } else {
                "TODO"
            }
        }
    };

    let res = set_state(source, doc, Some(&[entry_idx]), next_kw, true)?;
    let title = if next_kw.is_empty() {
        "Cycle TODO state (clear)".to_string()
    } else {
        format!("Cycle TODO state (-> {next_kw})")
    };
    let (end_l, end_c) = byte_offset_to_utf16_pos(source, source.content.len());

    Some(json!({
        "title": title,
        "kind": "refactor.rewrite",
        "edit": {
            "changes": {
                uri: [
                    {
                        "range": lsp_range(0, 0, end_l, end_c),
                        "newText": res.content
                    }
                ]
            }
        }
    }))
}

/// Builds a refactoring action to mint a deterministic `:ID:` for an entry lacking one.
fn build_mint_id_action(
    source: &SourceFile,
    doc: &OrgDocument,
    uri: &str,
    entry_idx: usize,
) -> Option<Value> {
    let entry = doc.entries.get(entry_idx)?;
    if entry.properties.contains_key("ID") {
        return None;
    }
    let config = DeterministicIdConfig::resolve(None, None).ok()?;
    let opts = DeterministicIdOptions {
        config,
        explicit_seed: None,
        scope: None,
        kind: None,
        author: None,
    };
    let res = add_deterministic_ids(source, doc, Some(&[entry_idx]), &opts).ok()??;
    let (end_l, end_c) = byte_offset_to_utf16_pos(source, source.content.len());

    Some(json!({
        "title": "Mint deterministic :ID: property",
        "kind": "refactor.rewrite",
        "edit": {
            "changes": {
                uri: [
                    {
                        "range": lsp_range(0, 0, end_l, end_c),
                        "newText": res.content
                    }
                ]
            }
        }
    }))
}

/// Handles `textDocument/codeAction` requests.
fn handle_code_action(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let range_obj = params
        .get("range")
        .ok_or_else(|| (-32602, "Missing range".to_string()))?;
    let s_line = range_obj["start"]["line"].as_u64().unwrap_or(0) as u32;
    let s_char = range_obj["start"]["character"].as_u64().unwrap_or(0) as u32;
    let e_line = range_obj["end"]["line"].as_u64().unwrap_or(0) as u32;
    let e_char = range_obj["end"]["character"].as_u64().unwrap_or(0) as u32;

    let start_byte = utf16_pos_to_byte_offset(&source, s_line, s_char).unwrap_or(0);
    let end_byte =
        utf16_pos_to_byte_offset(&source, e_line, e_char).unwrap_or(source.content.len());

    let mut actions = collect_quick_fix_actions(&source, &uri, start_byte, end_byte);

    if let Some(entry_idx) = find_entry_at_offset(&doc, &source, start_byte) {
        if let Some(act) = build_cycle_todo_action(&source, &doc, &uri, entry_idx) {
            actions.push(act);
        }
        if let Some(act) = build_mint_id_action(&source, &doc, &uri, entry_idx) {
            actions.push(act);
        }
    }

    if let Some(act) = build_checklist_toggle_action(&source, &uri, s_line) {
        actions.push(act);
    }

    Ok(Value::Array(actions))
}

/// Handles incoming LSP diagnostics, formatting, and code action requests.
pub(crate) fn handle_diagnostics_request(
    session: &mut LspSession,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/formatting" => handle_formatting(session, params),
        "textDocument/rangeFormatting" => handle_range_formatting(session, params),
        "textDocument/codeAction" => handle_code_action(session, params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

// endregion

// region Initialization Capability Builder

/// Extracts workspace directory override from `initialize` parameters.
fn extract_root_workspace(params: &Value) -> Option<PathBuf> {
    if let Some(folders) = params.get("workspaceFolders").and_then(|f| f.as_array()) {
        if let Some(uri) = folders
            .first()
            .and_then(|w| w.get("uri"))
            .and_then(|u| u.as_str())
        {
            if let Ok(p) = file_uri_to_path(uri) {
                return Some(p);
            }
        }
    }
    if let Some(root_uri) = params.get("rootUri").and_then(|u| u.as_str()) {
        if let Ok(p) = file_uri_to_path(root_uri) {
            return Some(p);
        }
    }
    params
        .get("rootPath")
        .and_then(|u| u.as_str())
        .map(PathBuf::from)
}

/// Negotiates initialization and constructs capabilities result payload.
pub(crate) fn build_initialize_result(session: &mut LspSession, params: &Value) -> Value {
    session.is_initialized = true;
    if !session.explicit_workspace {
        if let Some(ws) = extract_root_workspace(params) {
            session.display_workspace = ws.clone();
            session.default_workspace = normalize_workspace_path(&ws);
        }
    }
    json!({
        "capabilities": {
            "textDocumentSync": { "openClose": true, "change": 2, "save": { "includeText": false } },
            "documentFormattingProvider": true, "documentRangeFormattingProvider": true,
            "codeActionProvider": true, "documentSymbolProvider": true,
            "workspaceSymbolProvider": true, "definitionProvider": true,
            "referencesProvider": true, "documentLinkProvider": { "resolveProvider": false },
            "foldingRangeProvider": true, "hoverProvider": true,
            "completionProvider": { "triggerCharacters": ["[", ":", "#", "*"] },
            "renameProvider": { "prepareProvider": true },
            "semanticTokensProvider": {
                "legend": {
                    "tokenTypes": ["keyword", "namespace", "type", "property", "string", "number", "operator", "comment", "macro", "decorator"],
                    "tokenModifiers": ["declaration", "definition", "readonly", "modification", "documentation"]
                },
                "full": true
            }
        },
        "serverInfo": { "name": "org-tools", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Dispatches an incoming LSP request method to its module handler.
pub(crate) fn route_request(
    session: &mut LspSession,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(build_initialize_result(session, params)),
        "shutdown" => {
            session.is_shutdown = true;
            Ok(Value::Null)
        }
        "textDocument/formatting" | "textDocument/rangeFormatting" | "textDocument/codeAction" => {
            handle_diagnostics_request(session, method, params)
        }
        "textDocument/documentSymbol"
        | "workspace/symbol"
        | "textDocument/documentLink"
        | "textDocument/definition"
        | "textDocument/references"
        | "textDocument/foldingRange" => {
            super::navigation::handle_navigation_request(session, method, params)
        }
        "textDocument/completion" | "textDocument/hover" => {
            super::intelligence::handle_intelligence_request(session, method, params)
        }
        "textDocument/prepareRename"
        | "textDocument/rename"
        | "textDocument/semanticTokens/full" => {
            super::refactor_tokens::handle_refactor_tokens_request(session, method, params)
        }
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

// endregion
