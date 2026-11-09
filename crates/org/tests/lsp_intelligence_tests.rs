// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=600 reason="Comprehensive Red-to-Green behavioural test suite for LSP completion and hover intelligence"

//! Integration tests for LSP context-aware completion and rich hover previews.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

// region Test Client Harness & Workspace Fixture

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
    /// Spawns `org lsp --workspace <dir>` with piped stdio.
    fn spawn_workspace(dir: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
        cmd.arg("lsp")
            .arg("--workspace")
            .arg(dir.to_str().unwrap_or("."))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn org lsp process");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
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

    /// Sends a `Content-Length` framed JSON-RPC notification.
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

    /// Reads frames until a response matching `id` is found.
    fn read_response_matching_id(&mut self, target_id: i64) -> Value {
        loop {
            let msg = self.read_frame();
            if msg.get("id").and_then(Value::as_i64) == Some(target_id) {
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

/// Creates a temporary workspace populated with fixture Org documents for intelligence tests.
fn create_intelligence_workspace() -> TempDir {
    let temp = TempDir::new().expect("create temp dir");
    let ws = temp.path();

    let doc1 = r#"#+TITLE: Main Project Note
#+FILETAGS: :project:core:
* TODO Alpha Task                                                       :dev:
:PROPERTIES:
:ID:       11111111-1111-1111-1111-111111111111
:ROAM_ALIASES: "Alpha Alias"
:BLOCKER:  ids(22222222-2222-2222-2222-222222222222)
:TRIGGER:  ids(33333333-3333-3333-3333-333333333333) todo!("NEXT")
:END:
Alpha body text mentioning [[id:22222222-2222-2222-2222-222222222222]].
* DONE Finished Milestone
:PROPERTIES:
:ID:       33333333-3333-3333-3333-333333333333
:END:
| A | B | Total |
|---+---+-------|
| 2 | 3 |       |
| 4 | 5 |       |
|---+---+-------|
|   |   |       |
#+TBLFM: $3=$1+$2::@>$3=vsum(@I$3..@II$3)
"#;

    let doc2 = r#"#+TITLE: Dependency Specification
#+TAGS: urgent backend
* TODO Beta Blocker Task                                             :urgent:
:PROPERTIES:
:ID:       22222222-2222-2222-2222-222222222222
:CUSTOM_ID: custom-beta
:END:
Beta body content explaining the blocker requirement.
"#;

    fs::write(ws.join("doc1.org"), doc1).expect("write doc1.org");
    fs::write(ws.join("doc2.org"), doc2).expect("write doc2.org");
    temp
}

/// Initializes an LSP session and returns the test client and root URI.
fn init_session(temp: &TempDir) -> (LspTestClient, String) {
    let mut client = LspTestClient::spawn_workspace(temp.path());
    let root_uri = format!("file://{}", temp.path().display());
    let init_res = client.send_request(
        "initialize",
        1,
        json!({
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "completion": { "completionItem": { "snippetSupport": true } },
                    "hover": { "contentFormat": ["markdown"] }
                }
            }
        }),
    );
    assert!(init_res.get("result").is_some(), "initialize succeeded");
    client.send_notification("initialized", json!({}));
    (client, root_uri)
}

// endregion

// region Completion Tests

/// Verifies `textDocument/completion` for `[[id:` node link references and alias filtering.
#[test]
fn test_completion_node_links_and_aliases() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/scratch_link.org");

    // Line with unclosed `[[id:`
    let test_content = "* Heading\nSee [[id:";
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": doc_uri,
                "languageId": "org",
                "version": 1,
                "text": test_content
            }
        }),
    );

    // Request completion right after `[[id:` (line 1, col 9)
    let comp = client.send_request(
        "textDocument/completion",
        2,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 1, "character": 9 }
        }),
    );
    let items = comp["result"]["items"].as_array().expect("items array");
    assert!(!items.is_empty(), "expected node link completions");

    // Should include both nodes and aliases
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(
        labels.contains(&"Beta Blocker Task"),
        "expected Beta Blocker Task in {labels:?}"
    );
    assert!(
        labels.contains(&"Alpha Alias"),
        "expected Alpha Alias in {labels:?}"
    );

    // Check item structure for Beta Blocker Task
    let beta_item = items
        .iter()
        .find(|i| i["label"] == "Beta Blocker Task")
        .expect("find Beta Blocker Task item");
    assert_eq!(beta_item["kind"], 18, "reference completion kind");
    let edit = &beta_item["textEdit"];
    assert_eq!(
        edit["newText"].as_str().unwrap(),
        "[[id:22222222-2222-2222-2222-222222222222][Beta Blocker Task]]"
    );
    assert_eq!(edit["range"]["start"]["character"], 4); // where `[[` started

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

/// Verifies `textDocument/completion` for node links when editor auto-closed `]]`.
#[test]
fn test_completion_node_links_with_autoclosed_brackets() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/scratch.org");

    // Line where editor auto-inserted `]]` after `[[id:`
    let content = "Link to [[id:Beta]]";
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": doc_uri,
                "languageId": "org",
                "version": 1,
                "text": content
            }
        }),
    );

    // Cursor is between `Beta` and `]]` (line 0, col 17)
    let comp = client.send_request(
        "textDocument/completion",
        3,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 0, "character": 17 }
        }),
    );
    let items = comp["result"]["items"].as_array().expect("items array");
    let beta = items
        .iter()
        .find(|i| i["label"] == "Beta Blocker Task")
        .expect("find Beta");
    let edit = &beta["textEdit"];
    assert_eq!(edit["range"]["start"]["character"], 8); // `[[`
    assert_eq!(edit["range"]["end"]["character"], 19); // after `]]`

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

/// Verifies `textDocument/completion` for heading TODO keywords and priority cookies.
#[test]
fn test_completion_todo_keywords_and_priorities() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/test_todo.org");

    let content = "* TO\n* [#\n";
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": doc_uri,
                "languageId": "org",
                "version": 1,
                "text": content
            }
        }),
    );

    // Request completion on line 0, char 4 ("* TO|")
    let comp1 = client.send_request(
        "textDocument/completion",
        4,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 0, "character": 4 }
        }),
    );
    let items1 = comp1["result"]["items"].as_array().expect("items1 array");
    let kw_labels: Vec<&str> = items1.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(
        kw_labels.contains(&"TODO"),
        "expected TODO in {kw_labels:?}"
    );
    let todo_item = items1.iter().find(|i| i["label"] == "TODO").unwrap();
    assert_eq!(todo_item["kind"], 14, "keyword kind");

    // Request completion on line 1, char 4 ("* [#|")
    let comp2 = client.send_request(
        "textDocument/completion",
        5,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 1, "character": 4 }
        }),
    );
    let items2 = comp2["result"]["items"].as_array().expect("items2 array");
    let pri_labels: Vec<&str> = items2.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(
        pri_labels.contains(&"[#A]"),
        "expected [#A] in {pri_labels:?}"
    );
    let pri_item = items2.iter().find(|i| i["label"] == "[#A]").unwrap();
    assert_eq!(pri_item["kind"], 12, "value kind");

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

/// Verifies `textDocument/completion` for tags, `#+` directives, and `:PROPERTIES:` keys.
#[test]
fn test_completion_tags_directives_and_properties() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/test_props.org");

    let content = "* Heading :d\n#+TI\n* Another\n:PROPERTIES:\n:I\n:END:\n";
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": doc_uri,
                "languageId": "org",
                "version": 1,
                "text": content
            }
        }),
    );

    // 1. Tag completion on line 0, char 12 ("* Heading :d|")
    let comp_tag = client.send_request(
        "textDocument/completion",
        6,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 0, "character": 12 }
        }),
    );
    let tag_items = comp_tag["result"]["items"].as_array().expect("tag items");
    let tag_labels: Vec<&str> = tag_items
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(
        tag_labels.iter().any(|l| l.contains("dev")),
        "expected dev tag in {tag_labels:?}"
    );

    // 2. Directive completion on line 1, char 4 ("#+TI|")
    let comp_dir = client.send_request(
        "textDocument/completion",
        7,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 1, "character": 4 }
        }),
    );
    let dir_items = comp_dir["result"]["items"].as_array().expect("dir items");
    let dir_labels: Vec<&str> = dir_items
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(
        dir_labels.contains(&"#+TITLE:"),
        "expected #+TITLE: in {dir_labels:?}"
    );

    // 3. Property completion inside drawer on line 4, char 2 (":I|")
    let comp_prop = client.send_request(
        "textDocument/completion",
        8,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 4, "character": 2 }
        }),
    );
    let prop_items = comp_prop["result"]["items"].as_array().expect("prop items");
    let prop_labels: Vec<&str> = prop_items
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(
        prop_labels.contains(&":ID:"),
        "expected :ID: in {prop_labels:?}"
    );

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

// endregion

// region Hover Tests

/// Verifies `textDocument/hover` on `[[id:...]]` node links with rich Markdown metadata.
#[test]
fn test_hover_on_id_links() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/doc1.org");

    // doc1.org line 10 (0-indexed: line 9) has: "Alpha body text mentioning [[id:22222222-2222-2222-2222-222222222222]]."
    // `[[` begins around character 27
    let hover = client.send_request(
        "textDocument/hover",
        9,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 9, "character": 35 }
        }),
    );
    let val = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover markdown");
    assert!(
        val.contains("**Beta Blocker Task**"),
        "hover should contain node title: {val}"
    );
    assert!(
        val.contains("22222222-2222-2222-2222-222222222222"),
        "hover should contain ID"
    );
    assert!(
        val.contains("**Backlinks**: 1"),
        "hover should show backlinks: {val}"
    );

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

/// Verifies `textDocument/hover` on headings with `:BLOCKER:` evaluating org-edna status.
#[test]
fn test_hover_on_edna_blocker_heading() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/doc1.org");

    // Line 2 (0-indexed: line 2) is `* TODO Alpha Task` which has :BLOCKER: ids(Beta)
    let hover = client.send_request(
        "textDocument/hover",
        10,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 2, "character": 5 }
        }),
    );
    let val = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover markdown");
    println!("HOVER VAL:\n{val}");
    assert!(
        val.contains("**Edna Status**: BLOCKED"),
        "Alpha should be blocked by Beta: {val}"
    );
    assert!(
        val.contains("Beta Blocker Task"),
        "blocked by detail should list Beta: {val}"
    );

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

/// Verifies `textDocument/hover` on table row or `#+TBLFM:` line showing evaluated spreadsheet values.
#[test]
fn test_hover_on_table_and_tblfm() {
    let temp = create_intelligence_workspace();
    let (mut client, root_uri) = init_session(&temp);
    let doc_uri = format!("{root_uri}/doc1.org");

    // doc1.org has table on lines 15..20 (0-indexed) with #+TBLFM: $3=$1+$2::@>$3=vsum(@I$3..@II$3)
    let hover = client.send_request(
        "textDocument/hover",
        11,
        json!({
            "textDocument": { "uri": doc_uri },
            "position": { "line": 16, "character": 2 }
        }),
    );
    let val = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover table markdown");
    assert!(
        val.contains("| 2 | 3 | 5 |") || val.contains("5"),
        "table should show evaluated cell sum: {val}"
    );
    assert!(
        val.contains("TBLFM") || val.contains("Table"),
        "table hover header: {val}"
    );

    client.send_notification("exit", json!({}));
    let _ = client.finish();
}

// endregion
