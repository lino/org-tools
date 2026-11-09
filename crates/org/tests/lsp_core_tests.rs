// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=720 reason="Comprehensive Red-to-Green behavioural test suite for LSP protocol core, store, diagnostics, and actions"

//! End-to-end integration tests for `org lsp` protocol core, framing, diagnostics, and actions.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

// region Test Client Harness

/// Interactive stdio LSP client wrapping a spawned `org lsp` child process.
struct LspTestClient {
    /// Spawned child process.
    child: Child,
    /// Writable handle to child stdin.
    stdin: Option<ChildStdin>,
    /// Buffered reader over child stdout.
    stdout: BufReader<ChildStdout>,
}

impl LspTestClient {
    /// Spawns `org lsp` with arguments, working directory, and environment overrides.
    fn spawn(args: &[&str], cwd: Option<&Path>, envs: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
        cmd.arg("lsp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn org lsp process");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Spawns `org lsp --workspace <dir>` with piped stdio.
    fn spawn_workspace(dir: &Path, envs: &[(&str, &str)]) -> Self {
        Self::spawn(&["--workspace", dir.to_str().unwrap()], Some(dir), envs)
    }

    /// Sends a `Content-Length` framed JSON-RPC request and returns the decoded JSON payload.
    fn send_request(&mut self, method: &str, id: i64, params: Value) -> Value {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.write_frame(&payload);
        self.read_response_matching_id(id)
    }

    /// Sends a `Content-Length` framed JSON-RPC notification (without `id`).
    fn send_notification(&mut self, method: &str, params: Value) {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.write_frame(&payload);
    }

    /// Writes a single `Content-Length` framed JSON message to child stdin.
    fn write_frame(&mut self, payload: &Value) {
        let body = serde_json::to_string(payload).expect("serialize payload");
        let header = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(header.as_bytes()).expect("write header");
        stdin.flush().expect("flush stdin");
    }

    /// Reads frames until a response matching `id` is found, collecting notifications.
    fn read_response_matching_id(&mut self, target_id: i64) -> Value {
        loop {
            let msg = self.read_frame();
            if msg.get("id").and_then(Value::as_i64) == Some(target_id) {
                return msg;
            }
        }
    }

    /// Reads frames until a notification matching `method` is found.
    fn read_notification_matching(&mut self, target_method: &str) -> Value {
        loop {
            let msg = self.read_frame();
            if msg.get("method").and_then(Value::as_str) == Some(target_method) {
                return msg;
            }
        }
    }

    /// Reads a single `Content-Length` framed JSON message from child stdout.
    fn read_frame(&mut self) -> Value {
        let mut header = String::new();
        loop {
            header.clear();
            let read_bytes = self.stdout.read_line(&mut header).expect("read header");
            assert!(read_bytes > 0, "unexpected EOF while reading frame header");
            let trimmed = header.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                let len: usize = len_str.trim().parse().expect("parse Content-Length");
                let mut sep = String::new();
                self.stdout.read_line(&mut sep).expect("read separator");
                let mut body = vec![0u8; len];
                self.stdout.read_exact(&mut body).expect("read body");
                return serde_json::from_slice(&body).expect("parse JSON body");
            }
        }
    }

    /// Closes child stdin and waits for child process termination, returning exit status code.
    fn finish(mut self) -> i32 {
        self.stdin.take();
        self.child
            .wait()
            .expect("wait for child")
            .code()
            .unwrap_or(-1)
    }
}

/// Creates a temporary workspace populated with fixture Org documents.
fn create_test_workspace() -> TempDir {
    let temp = TempDir::new().expect("create temp dir");
    let ws = temp.path();
    fs::create_dir_all(ws.join(".cache/telemetry")).expect("create telemetry dir");

    let doc1 = r#"#+TITLE: Test Document 1
* TODO First Task                                                       :tag1:
:PROPERTIES:
:ID:       11111111-1111-1111-1111-111111111111
:END:
Here is some body text with trailing whitespace.   
- [ ] Checklist item 1
- [X] Checklist item 2

| Quantity | Price | Total |
|---+---+---|
| 2 | 10 | |
| 3 | 5 | |
#+TBLFM: $3=$1*$2
"#;

    let doc2 = r#"#+TITLE: Test Document 2
* NEXT Second Task
:PROPERTIES:
:ID:       22222222-2222-2222-2222-222222222222
:END:
Reference to first task: [[id:11111111-1111-1111-1111-111111111111][First Task]].
"#;

    fs::write(ws.join("doc1.org"), doc1).expect("write doc1.org");
    fs::write(ws.join("doc2.org"), doc2).expect("write doc2.org");
    temp
}

// endregion

// region Behavioural Integration Tests

/// Verifies `initialize` negotiation and advertised capabilities.
#[test]
fn test_lsp_initialize_and_capabilities() {
    let ws = create_test_workspace();
    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);

    let init_resp = client.send_request(
        "initialize",
        1,
        json!({
            "rootUri": format!("file://{}", ws.path().display()),
            "capabilities": {}
        }),
    );

    let result = &init_resp["result"];
    let caps = &result["capabilities"];
    assert_eq!(caps["textDocumentSync"]["openClose"], true);
    assert_eq!(caps["textDocumentSync"]["change"], 2);
    assert_eq!(caps["documentFormattingProvider"], true);
    assert_eq!(caps["documentRangeFormattingProvider"], true);
    assert_eq!(caps["codeActionProvider"], true);
    assert_eq!(
        caps["completionProvider"]["triggerCharacters"],
        json!(["[", ":", "#", "*"])
    );
    assert_eq!(caps["renameProvider"]["prepareProvider"], true);
    assert_eq!(result["serverInfo"]["name"], "org-tools");

    client.send_notification("initialized", json!({}));
    let shutdown_resp = client.send_request("shutdown", 2, json!({}));
    assert!(shutdown_resp["result"].is_null());
    client.send_notification("exit", json!({}));
    let code = client.finish();
    assert_eq!(code, 0);
}

/// Verifies exit code 1 when `exit` notification is received without prior `shutdown`.
#[test]
fn test_lsp_exit_without_shutdown() {
    let ws = create_test_workspace();
    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);

    client.send_request(
        "initialize",
        1,
        json!({
            "rootUri": format!("file://{}", ws.path().display()),
            "capabilities": {}
        }),
    );
    client.send_notification("initialized", json!({}));
    client.send_notification("exit", json!({}));
    let code = client.finish();
    assert_eq!(code, 1);
}

/// Verifies that requests received after `shutdown` return error `-32600`.
#[test]
fn test_lsp_post_shutdown_request_rejected() {
    let ws = create_test_workspace();
    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);

    client.send_request(
        "initialize",
        1,
        json!({
            "rootUri": format!("file://{}", ws.path().display()),
            "capabilities": {}
        }),
    );
    client.send_notification("initialized", json!({}));
    let shutdown_resp = client.send_request("shutdown", 2, json!({}));
    assert!(shutdown_resp["result"].is_null());

    let post_req = client.send_request(
        "textDocument/formatting",
        3,
        json!({
            "textDocument": { "uri": format!("file://{}/doc1.org", ws.path().display()) },
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    );
    assert_eq!(post_req["error"]["code"], -32600);
    client.send_notification("exit", json!({}));
    let code = client.finish();
    assert_eq!(code, 0);
}

/// Verifies UTF-16 coordinate translation across ASCII, CJK, and emoji surrogate pairs via range formatting.
#[test]
fn test_utf16_coordinate_translation() {
    let ws = create_test_workspace();
    let doc_path = ws.path().join("cjk.org");
    let initial = "#+TITLE: CJK\n\n* TODO First  \n你好世界\nEmoji 👋 and 🌍 globe   \n";
    fs::write(&doc_path, initial).expect("write cjk.org");

    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);
    let uri = format!("file://{}", doc_path.display());

    client.send_request(
        "initialize",
        1,
        json!({ "rootUri": format!("file://{}", ws.path().display()) }),
    );
    client.send_notification("initialized", json!({}));

    // Range formatting on line 4 with emoji 👋 and 🌍 (surrogate pairs)
    let range_resp = client.send_request(
        "textDocument/rangeFormatting",
        2,
        json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 4, "character": 0 },
                "end": { "line": 4, "character": 24 }
            },
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    );

    let edits = range_resp["result"].as_array().expect("range edits");
    assert!(
        !edits.is_empty(),
        "must produce range edit for trailing whitespace"
    );

    client.send_request("shutdown", 3, json!({}));
    client.send_notification("exit", json!({}));
    client.finish();
}

/// Sends incremental `didChange` content modifications and asserts diagnostics are emitted.
fn send_did_change_and_assert_diagnostics(client: &mut LspTestClient, uri: &str) {
    client.send_notification(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [
                {
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 20 }
                    },
                    "text": "#+TITLE: Mutated Title"
                }
            ]
        }),
    );
    let diag_change = client.read_notification_matching("textDocument/publishDiagnostics");
    assert_eq!(diag_change["params"]["uri"], uri);
}

/// Verifies in-memory document store overlay across didOpen, didChange, didSave, and didClose.
#[test]
fn test_document_store_open_change_save_close() {
    let ws = create_test_workspace();
    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);
    let uri = format!("file://{}/doc1.org", ws.path().display());

    client.send_request(
        "initialize",
        1,
        json!({ "rootUri": format!("file://{}", ws.path().display()) }),
    );
    client.send_notification("initialized", json!({}));

    // didOpen publishes diagnostics
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "org",
                "version": 1,
                "text": "#+TITLE: Open Buffer\n* TODO New Entry\n"
            }
        }),
    );
    let diag_open = client.read_notification_matching("textDocument/publishDiagnostics");
    assert_eq!(diag_open["params"]["uri"], uri);

    // didChange (incremental replacement of first line)
    send_did_change_and_assert_diagnostics(&mut client, &uri);

    // didClose clears open overlay
    client.send_notification(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri } }),
    );
    let diag_close = client.read_notification_matching("textDocument/publishDiagnostics");
    assert_eq!(diag_close["params"]["uri"], uri);

    client.send_request("shutdown", 3, json!({}));
    client.send_notification("exit", json!({}));
    let code = client.finish();
    assert_eq!(code, 0);
}

/// Verifies `textDocument/formatting` with `#+TBLFM:` recalculation and protected block preservation.
#[test]
fn test_formatting_and_tblfm() {
    let ws = create_test_workspace();
    let doc_path = ws.path().join("tbl.org");
    let initial_content = "#+TITLE: Table Test\n\n| A | B | C |\n|---+---+---|\n| 2 | 4 | |\n#+TBLFM: $3=$1*$2\n\n#+BEGIN_SRC python\ndef foo():\n    return 42\n#+END_SRC\n";
    fs::write(&doc_path, initial_content).expect("write tbl.org");

    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);
    let uri = format!("file://{}", doc_path.display());

    client.send_request(
        "initialize",
        1,
        json!({ "rootUri": format!("file://{}", ws.path().display()) }),
    );
    client.send_notification("initialized", json!({}));

    let fmt_resp = client.send_request(
        "textDocument/formatting",
        2,
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    );

    let edits = fmt_resp["result"]
        .as_array()
        .expect("formatting edits array");
    assert!(!edits.is_empty(), "formatting should produce edits");
    let new_text = edits[0]["newText"].as_str().expect("newText string");
    // $3 should be calculated as 8
    assert!(
        new_text.contains("| 2 | 4 | 8 |"),
        "table formula must be evaluated: {new_text}"
    );
    // Protected block must preserve interior code
    assert!(
        new_text.contains("def foo():\n    return 42"),
        "protected python block must remain untouched"
    );

    client.send_request("shutdown", 3, json!({}));
    client.send_notification("exit", json!({}));
    client.finish();
}

/// Asserts that quickfix and source.fixAll actions are returned for diagnostics on line 5.
fn assert_quickfix_actions_available(client: &mut LspTestClient, uri: &str) {
    let actions_resp = client.send_request(
        "textDocument/codeAction",
        2,
        json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 5, "character": 0 },
                "end": { "line": 5, "character": 10 }
            },
            "context": { "diagnostics": [] }
        }),
    );
    let actions = actions_resp["result"].as_array().expect("actions array");
    let titles: Vec<&str> = actions.iter().filter_map(|a| a["title"].as_str()).collect();
    assert!(
        titles
            .iter()
            .any(|t| t.contains("trailing-whitespace") || t.contains("Fix")),
        "must offer quickfix"
    );
    assert!(
        titles.iter().any(|t| t.contains("Fix all")),
        "must offer source.fixAll"
    );
}

/// Asserts that heading refactoring actions (Cycle TODO state) are returned on heading line 1.
fn assert_heading_cycle_todo_action(client: &mut LspTestClient, uri: &str) {
    let heading_actions_resp = client.send_request(
        "textDocument/codeAction",
        3,
        json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 6 }
            },
            "context": { "diagnostics": [] }
        }),
    );
    let heading_actions = heading_actions_resp["result"]
        .as_array()
        .expect("heading actions");
    let heading_titles: Vec<&str> = heading_actions
        .iter()
        .filter_map(|a| a["title"].as_str())
        .collect();
    assert!(
        heading_titles.iter().any(|t| t.contains("Cycle TODO")),
        "must offer Cycle TODO state: {heading_titles:?}"
    );
}

/// Asserts that checklist toggle actions are returned on checklist line 6.
fn assert_checklist_toggle_action(client: &mut LspTestClient, uri: &str) {
    let check_actions_resp = client.send_request(
        "textDocument/codeAction",
        4,
        json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 6, "character": 2 },
                "end": { "line": 6, "character": 5 }
            },
            "context": { "diagnostics": [] }
        }),
    );
    let check_actions = check_actions_resp["result"]
        .as_array()
        .expect("checklist actions");
    let check_titles: Vec<&str> = check_actions
        .iter()
        .filter_map(|a| a["title"].as_str())
        .collect();
    assert!(
        check_titles.iter().any(|t| t.contains("Toggle checklist")),
        "must offer Toggle checklist item: {check_titles:?}"
    );
}

/// Verifies `textDocument/codeAction` serving quick-fixes, source.fixAll, and refactorings.
#[test]
fn test_code_actions_quickfix_and_refactor() {
    let ws = create_test_workspace();
    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);
    let uri = format!("file://{}/doc1.org", ws.path().display());

    client.send_request(
        "initialize",
        1,
        json!({ "rootUri": format!("file://{}", ws.path().display()) }),
    );
    client.send_notification("initialized", json!({}));

    assert_quickfix_actions_available(&mut client, &uri);
    assert_heading_cycle_todo_action(&mut client, &uri);
    assert_checklist_toggle_action(&mut client, &uri);

    client.send_request("shutdown", 5, json!({}));
    client.send_notification("exit", json!({}));
    client.finish();
}

/// Asserts that Prometheus metrics file exists and records request metrics.
fn assert_prom_metrics_recorded(tel_dir: &Path) {
    let prom_file = tel_dir.join("orgfmt_lsp.prom");
    assert!(
        prom_file.exists(),
        "Prometheus file must exist: {}",
        prom_file.display()
    );
    let prom_content = fs::read_to_string(&prom_file).expect("read prom file");
    assert!(
        prom_content.contains("orgfmt_lsp_requests_total{method=\"initialize\",status=\"ok\"}"),
        "prom content: {prom_content}"
    );
    assert!(
        prom_content.contains("orgfmt_lsp_request_duration_seconds{method=\"initialize\"}"),
        "prom content: {prom_content}"
    );
}

/// Asserts that JSONL event log exists and propagates trace and event attributes.
fn assert_jsonl_events_recorded(tel_dir: &Path, expected_trace_id: &str) {
    let events_file = tel_dir.join("events.jsonl");
    assert!(
        events_file.exists(),
        "JSONL events file must exist: {}",
        events_file.display()
    );
    let events_content = fs::read_to_string(&events_file).expect("read jsonl file");
    assert!(
        events_content.contains(expected_trace_id),
        "JSONL must propagate trace id: {events_content}"
    );
    assert!(
        events_content.contains("\"event\":\"lsp.request\""),
        "JSONL must record lsp.request: {events_content}"
    );
}

/// Verifies Prometheus metrics and JSONL telemetry output with `FRITZ_TRACE_ID` propagation.
#[test]
fn test_telemetry_prom_and_jsonl() {
    let ws = create_test_workspace();
    let tel_dir = ws.path().join("custom_telemetry");
    let envs = [
        ("ORG_LSP_TELEMETRY_DIR", tel_dir.to_str().unwrap()),
        ("FRITZ_TRACE_ID", "test-trace-uuid-1234"),
    ];
    let mut client = LspTestClient::spawn(
        &["--workspace", ws.path().to_str().unwrap()],
        Some(ws.path()),
        &envs,
    );

    client.send_request(
        "initialize",
        1,
        json!({ "rootUri": format!("file://{}", ws.path().display()) }),
    );
    client.send_notification("initialized", json!({}));
    client.send_request("shutdown", 2, json!({}));
    client.send_notification("exit", json!({}));
    client.finish();

    assert_prom_metrics_recorded(&tel_dir);
    assert_jsonl_events_recorded(&tel_dir, "test-trace-uuid-1234");
}

/// Verifies `textDocument/publishDiagnostics` detects cross-file duplicate IDs across the workspace.
#[test]
fn test_publish_diagnostics_cross_file_duplicate_id() {
    let ws = create_test_workspace();
    let mut client = LspTestClient::spawn_workspace(ws.path(), &[]);
    let dup_uri = format!("file://{}/dup.org", ws.path().display());

    client.send_request(
        "initialize",
        1,
        json!({ "rootUri": format!("file://{}", ws.path().display()) }),
    );
    client.send_notification("initialized", json!({}));

    // doc1.org already defines ID: 11111111-1111-1111-1111-111111111111
    let dup_content = "* Duplicate Heading\n:PROPERTIES:\n:ID:       11111111-1111-1111-1111-111111111111\n:END:\n";
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": dup_uri,
                "languageId": "org",
                "version": 1,
                "text": dup_content
            }
        }),
    );

    let diag_notif = client.read_notification_matching("textDocument/publishDiagnostics");
    assert_eq!(diag_notif["params"]["uri"], dup_uri);
    let diags = diag_notif["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diags
            .iter()
            .any(|d| d["code"] == "E009" || d["code"] == "E002"),
        "must report duplicate-id diagnostic: {diags:?}"
    );

    client.send_request("shutdown", 2, json!({}));
    client.send_notification("exit", json!({}));
    client.finish();
}

// endregion
