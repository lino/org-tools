// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=600 reason="End-to-end integration tests for LSP navigation handlers"

//! Integration tests for LSP document symbols, workspace symbols, links, definitions, references, and folding.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use org_tools_core::export_md::normalize_workspace_path;
use serde_json::{json, Value};
use tempfile::TempDir;

// region Test Client Harness

/// Interactive stdio LSP test client wrapping a spawned `org lsp` child process.
struct LspNavTestClient {
    /// Spawned child process.
    child: Child,
    /// Writable handle to child stdin.
    stdin: Option<ChildStdin>,
    /// Buffered reader over child stdout.
    stdout: BufReader<ChildStdout>,
}

impl LspNavTestClient {
    /// Spawns `org lsp --workspace <dir>` with piped stdio.
    fn spawn(dir: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
        cmd.arg("lsp")
            .arg("--workspace")
            .arg(dir.to_str().expect("valid utf8 dir"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .current_dir(dir);
        let mut child = cmd.spawn().expect("spawn org lsp process");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Sends a `Content-Length` framed JSON-RPC request and returns the decoded JSON response.
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

    /// Shuts down the LSP session and ensures clean exit.
    fn shutdown_and_exit(mut self) {
        let resp = self.send_request("shutdown", 9999, Value::Null);
        assert!(resp.get("result").is_some());
        self.send_notification("exit", Value::Null);
        self.stdin.take();
        let status = self.child.wait().expect("wait for child");
        assert!(status.success());
    }
}

/// Converts a path to a canonical `file://` URI string.
fn to_file_uri(path: &Path) -> String {
    let s = normalize_workspace_path(path)
        .to_string_lossy()
        .replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// Creates a test workspace with sample `.org` files and subdirectories.
fn setup_test_workspace() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = TempDir::new().expect("create temp dir");
    let ws = temp.path();

    let doc1_path = ws.join("doc1.org");
    let doc2_path = ws.join("doc2.org");
    let sub_dir = ws.join("sub");
    fs::create_dir_all(&sub_dir).expect("create sub dir");
    let included_path = sub_dir.join("included.org");

    let doc1_content = r#":PROPERTIES:
:ID:       11111111-1111-1111-1111-111111111111
:ROAM_ALIASES: "Doc One"
:END:
#+TITLE: Document One

#+INCLUDE: "sub/included.org"

* TODO Parent Task                                                      :tag1:
:PROPERTIES:
:CUSTOM_ID: parent-custom-id
:END:
See [[id:22222222-2222-2222-2222-222222222222][Second Document]] and [[file:sub/included.org::#target-custom-id][Included Target]].
** DONE [#A] Child Task                                                 :tag2:
Body of child task.
#+BEGIN_SRC rust
fn example() {}
#+END_SRC
"#;

    let doc2_content = r#":PROPERTIES:
:ID:       22222222-2222-2222-2222-222222222222
:END:
#+TITLE: Document Two

* Standalone Heading
:PROPERTIES:
:ID:       33333333-3333-3333-3333-333333333333
:CUSTOM_ID: doc2-custom-id
:END:
Link back to [[id:11111111-1111-1111-1111-111111111111]].
"#;

    let included_content = r#"#+TITLE: Included File

* Target Heading
:PROPERTIES:
:CUSTOM_ID: target-custom-id
:END:
Target content here.
"#;

    fs::write(&doc1_path, doc1_content).expect("write doc1");
    fs::write(&doc2_path, doc2_content).expect("write doc2");
    fs::write(&included_path, included_content).expect("write included");

    (temp, doc1_path, doc2_path, included_path)
}

// endregion

// region Test Cases

/// Verifies `textDocument/documentSymbol` and `workspace/symbol` responses.
#[test]
fn test_document_symbols_and_workspace_symbols() {
    let (_temp, doc1_path, _doc2_path, _inc) = setup_test_workspace();
    let ws = doc1_path.parent().expect("workspace root");
    let mut client = LspNavTestClient::spawn(ws);

    let init_res = client.send_request("initialize", 1, json!({ "rootUri": to_file_uri(ws) }));
    assert!(init_res.get("result").is_some());

    let sym_res = client.send_request(
        "textDocument/documentSymbol",
        2,
        json!({ "textDocument": { "uri": to_file_uri(&doc1_path) } }),
    );
    let syms = sym_res["result"].as_array().expect("array of symbols");
    assert!(!syms.is_empty(), "expected document symbols");

    // File level ID symbol should be present.
    let file_sym = syms.iter().find(|s| s["name"] == "Document One");
    assert!(file_sym.is_some(), "expected Document One symbol");
    assert_eq!(file_sym.unwrap()["kind"], 1);

    // Parent heading symbol with nested children.
    let parent_sym = syms.iter().find(|s| s["name"] == "Parent Task");
    assert!(parent_sym.is_some(), "expected Parent Task symbol");
    let children = parent_sym.unwrap()["children"]
        .as_array()
        .expect("children");
    assert_eq!(children.len(), 1, "expected 1 child under Parent Task");
    assert_eq!(children[0]["name"], "Child Task");

    // Workspace symbol search.
    let ws_res = client.send_request("workspace/symbol", 3, json!({ "query": "child" }));
    let ws_syms = ws_res["result"].as_array().expect("array of ws symbols");
    assert!(!ws_syms.is_empty(), "expected matching workspace symbols");
    assert!(ws_syms.iter().any(|s| s["name"] == "Child Task"));

    client.shutdown_and_exit();
}

/// Verifies `textDocument/documentLink` and `textDocument/definition` navigation.
#[test]
fn test_document_links_and_goto_definition() {
    let (_temp, doc1_path, doc2_path, inc_path) = setup_test_workspace();
    let ws = doc1_path.parent().expect("workspace root");
    let mut client = LspNavTestClient::spawn(ws);

    let init_res = client.send_request("initialize", 1, json!({ "rootUri": to_file_uri(ws) }));
    assert!(init_res.get("result").is_some());

    // Document links.
    let links_res = client.send_request(
        "textDocument/documentLink",
        2,
        json!({ "textDocument": { "uri": to_file_uri(&doc1_path) } }),
    );
    let links = links_res["result"].as_array().expect("array of links");
    assert!(links.len() >= 3, "expected at least 3 document links");

    // Definition for [[id:22222222...]] link at line 12.
    let def_res = client.send_request(
        "textDocument/definition",
        3,
        json!({
            "textDocument": { "uri": to_file_uri(&doc1_path) },
            "position": { "line": 12, "character": 12 }
        }),
    );
    let def = &def_res["result"];
    assert!(!def.is_null(), "expected definition result");
    assert_eq!(def["uri"], to_file_uri(&doc2_path));

    // Definition for #+INCLUDE: "sub/included.org" at line 6.
    let inc_def_res = client.send_request(
        "textDocument/definition",
        4,
        json!({
            "textDocument": { "uri": to_file_uri(&doc1_path) },
            "position": { "line": 6, "character": 15 }
        }),
    );
    let inc_def = &inc_def_res["result"];
    assert!(!inc_def.is_null(), "expected include definition result");
    assert_eq!(inc_def["uri"], to_file_uri(&inc_path));

    client.shutdown_and_exit();
}

/// Verifies `textDocument/references` and `textDocument/foldingRange`.
#[test]
fn test_references_and_folding_ranges() {
    let (_temp, doc1_path, doc2_path, _inc) = setup_test_workspace();
    let ws = doc1_path.parent().expect("workspace root");
    let mut client = LspNavTestClient::spawn(ws);

    let init_res = client.send_request("initialize", 1, json!({ "rootUri": to_file_uri(ws) }));
    assert!(init_res.get("result").is_some());

    // References for level-0 ID in doc2 at line 1.
    let refs_res = client.send_request(
        "textDocument/references",
        2,
        json!({
            "textDocument": { "uri": to_file_uri(&doc2_path) },
            "position": { "line": 1, "character": 12 },
            "context": { "includeDeclaration": true }
        }),
    );
    let refs = refs_res["result"].as_array().expect("array of references");
    assert!(
        refs.len() >= 2,
        "expected declaration and incoming reference"
    );
    assert!(refs.iter().any(|r| r["uri"] == to_file_uri(&doc1_path)));

    // Folding ranges for doc1.
    let fold_res = client.send_request(
        "textDocument/foldingRange",
        3,
        json!({ "textDocument": { "uri": to_file_uri(&doc1_path) } }),
    );
    let folds = fold_res["result"].as_array().expect("array of folds");
    assert!(!folds.is_empty(), "expected folding ranges");
    // Ensure properties drawer and headings folded.
    assert!(folds.iter().any(|f| f["startLine"] == 0));

    client.shutdown_and_exit();
}

/// Verifies that unsaved open buffer overlays are immediately reflected in workspace symbol queries.
#[test]
fn test_workspace_symbol_open_buffer_overlay() {
    let (_temp, doc1_path, _doc2_path, _inc) = setup_test_workspace();
    let ws = doc1_path.parent().expect("workspace root");
    let mut client = LspNavTestClient::spawn(ws);

    let init_res = client.send_request("initialize", 1, json!({ "rootUri": to_file_uri(ws) }));
    assert!(init_res.get("result").is_some());

    let unsaved_uri = format!("{}/unsaved.org", to_file_uri(ws));
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": unsaved_uri,
                "languageId": "org",
                "version": 1,
                "text": "* Special Unsaved Buffer Heading\nContent here.\n"
            }
        }),
    );

    let sym_res = client.send_request("workspace/symbol", 2, json!({ "query": "Unsaved Buffer" }));
    let syms = sym_res["result"].as_array().expect("array of symbols");
    assert_eq!(
        syms.len(),
        1,
        "expected overlay heading in workspace symbols"
    );
    assert_eq!(syms[0]["name"], "Special Unsaved Buffer Heading");

    client.shutdown_and_exit();
}

/// Verifies protected region exclusion for links, custom ID references, and comment block folding.
#[test]
fn test_protected_regions_and_custom_id_references() {
    let (_temp, doc1_path, _doc2_path, inc_path) = setup_test_workspace();
    let ws = doc1_path.parent().expect("workspace root");
    let mut client = LspNavTestClient::spawn(ws);

    let init_res = client.send_request("initialize", 1, json!({ "rootUri": to_file_uri(ws) }));
    assert!(init_res.get("result").is_some());

    // In included.org, references for custom-id target-custom-id at line 4.
    let refs_res = client.send_request(
        "textDocument/references",
        2,
        json!({
            "textDocument": { "uri": to_file_uri(&inc_path) },
            "position": { "line": 4, "character": 15 },
            "context": { "includeDeclaration": true }
        }),
    );
    let refs = refs_res["result"].as_array().expect("references array");
    assert!(!refs.is_empty(), "expected references to target-custom-id");
    assert!(refs.iter().any(|r| r["uri"] == to_file_uri(&doc1_path)));

    // Verify folding for comments and logbooks in an open buffer.
    let sample_uri = format!("{}/fold_sample.org", to_file_uri(ws));
    let sample_text = "* Main Heading\n:LOGBOOK:\nCLOCK: [2026-11-09 Mon 10:00]--[2026-11-09 Mon 11:00] => 1:00\n:END:\n#+BEGIN_COMMENT\nSecret note.\n#+END_COMMENT\n";
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": sample_uri,
                "languageId": "org",
                "version": 1,
                "text": sample_text
            }
        }),
    );
    let fold_res = client.send_request(
        "textDocument/foldingRange",
        3,
        json!({ "textDocument": { "uri": sample_uri } }),
    );
    let folds = fold_res["result"].as_array().expect("folds array");
    assert!(folds.iter().any(|f| f["kind"] == "comment"));

    client.shutdown_and_exit();
}

// endregion
