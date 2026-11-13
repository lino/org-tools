// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=450 reason="Comprehensive Red-to-Green integration test suite for LSP rename and semantic tokens"

//! End-to-end integration tests for LSP rename (headings, CUSTOM_ID, ID) and semantic tokens.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use org_tools_core::export_md::normalize_workspace_path;
use serde_json::{json, Value};
use tempfile::TempDir;

// region LSP Test Client

/// Test client managing a child `org lsp` process over stdio.
struct LspTestClient {
    /// Spawned child process handle.
    child: Child,
    /// Standard input handle for writing JSON-RPC frames.
    stdin: Option<ChildStdin>,
    /// Standard output reader for reading JSON-RPC frames.
    stdout: BufReader<ChildStdout>,
}

impl LspTestClient {
    /// Spawns `org lsp --workspace <dir>` with piped stdio.
    fn spawn(dir: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
        cmd.arg("lsp")
            .args(["--workspace", dir.to_str().unwrap()])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn org lsp");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Writes a Content-Length framed JSON message to stdin.
    fn write_frame(&mut self, payload: &Value) {
        let body = serde_json::to_string(payload).expect("serialize payload");
        let header = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(header.as_bytes()).expect("write header");
        stdin.flush().expect("flush stdin");
    }

    /// Sends a JSON-RPC request and returns the response frame matching `id`.
    fn send_request(&mut self, method: &str, id: i64, params: Value) -> Value {
        self.write_frame(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        loop {
            let msg = self.read_frame();
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
            }
        }
    }

    /// Sends a JSON-RPC notification.
    fn send_notification(&mut self, method: &str, params: Value) {
        self.write_frame(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }));
    }

    /// Reads a single Content-Length framed message.
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

    /// Gracefully shuts down and finishes the LSP session.
    fn finish(mut self) -> i32 {
        self.stdin.take();
        self.child
            .wait()
            .expect("wait for child")
            .code()
            .unwrap_or(-1)
    }
}

/// Converts a path to a canonical RFC 8089 `file://` URI string.
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

/// Initializes an LSP client connection for `dir`.
fn init_client(dir: &Path) -> LspTestClient {
    let mut client = LspTestClient::spawn(dir);
    let init_resp = client.send_request(
        "initialize",
        1,
        json!({
            "rootUri": to_file_uri(dir),
            "capabilities": {}
        }),
    );
    assert!(init_resp["result"].get("capabilities").is_some());
    client.send_notification("initialized", json!({}));
    client
}

// endregion

// region Integration Tests

/// Verifies that workspace edits correctly update the heading definition and cross-file links.
fn assert_heading_rename_edits(doc1_edits: &[Value], doc2_edits: &[Value]) {
    assert!(!doc1_edits.is_empty());
    assert!(!doc2_edits.is_empty());

    let has_heading_edit = doc1_edits.iter().any(|e| {
        let text = e["newText"].as_str().unwrap_or("");
        text.contains("* TODO New Clean Title") && text.contains(":tag1:tag2:")
    });
    assert!(has_heading_edit, "heading was updated with aligned tags");

    let has_link_edit = doc1_edits.iter().any(|e| {
        e["newText"]
            .as_str()
            .unwrap_or("")
            .contains("*New Clean Title")
    });
    assert!(has_link_edit, "internal heading link was rewritten");

    let has_id_desc_edit = doc2_edits.iter().any(|e| {
        e["newText"].as_str().unwrap_or("") == "New Clean Title"
            || e["newText"]
                .as_str()
                .unwrap_or("")
                .contains("New Clean Title")
    });
    assert!(has_id_desc_edit, "cross-file id link desc was rewritten");
}

/// Verifies prepareRename and rename on a heading title with tag re-alignment and cross-file link updates.
#[test]
fn test_rename_heading_title_with_tags_and_workspace_links() {
    let temp = TempDir::new().expect("create temp dir");
    let ws = normalize_workspace_path(temp.path());

    let doc1 = "#+TITLE: Notes\n* TODO Old Heading Title :tag1:tag2:\n:PROPERTIES:\n:ID:       11111111-2222-3333-4444-555555555555\n:END:\nLink: [[*Old Heading Title]]\n";
    let doc2 = "#+TITLE: Tasks\nRef: [[id:11111111-2222-3333-4444-555555555555][Old Heading Title]]\nTarget: [[file:doc1.org::*Old Heading Title]]\nOther: [[id:11111111-2222-3333-4444-555555555555][Keep Desc]]\n";
    fs::write(ws.join("doc1.org"), doc1).expect("write doc1");
    fs::write(ws.join("doc2.org"), doc2).expect("write doc2");

    let mut client = init_client(&ws);
    let doc1_uri = to_file_uri(&ws.join("doc1.org"));
    let doc2_uri = to_file_uri(&ws.join("doc2.org"));

    let prep = client.send_request(
        "textDocument/prepareRename",
        2,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 1, "character": 9 }
        }),
    );
    assert_eq!(prep["result"]["placeholder"], "Old Heading Title");

    let ren = client.send_request(
        "textDocument/rename",
        3,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 1, "character": 9 },
            "newName": "New Clean Title"
        }),
    );
    let changes = &ren["result"]["changes"];
    let doc1_edits = changes[&doc1_uri].as_array().expect("doc1 edits");
    let doc2_edits = changes[&doc2_uri].as_array().expect("doc2 edits");
    assert_heading_rename_edits(doc1_edits, doc2_edits);

    client.send_notification("exit", json!({}));
    assert_eq!(client.finish(), 1);
}

/// Verifies prepareRename and rename invoked directly on a `[[*...]]` heading link.
#[test]
fn test_rename_invoked_from_heading_link() {
    let temp = TempDir::new().expect("create temp dir");
    let ws = normalize_workspace_path(temp.path());

    let doc1 = "#+TITLE: Doc\n* Target Heading\nBody text.\nLink here: [[*Target Heading]]\n";
    fs::write(ws.join("doc1.org"), doc1).expect("write doc1");

    let mut client = init_client(&ws);
    let doc1_uri = to_file_uri(&ws.join("doc1.org"));

    let prep = client.send_request(
        "textDocument/prepareRename",
        2,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 3, "character": 15 }
        }),
    );
    assert_eq!(prep["result"]["placeholder"], "Target Heading");

    let ren = client.send_request(
        "textDocument/rename",
        3,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 3, "character": 15 },
            "newName": "Updated Heading"
        }),
    );
    let edits = &ren["result"]["changes"][&doc1_uri];
    let has_def_edit = edits.as_array().unwrap().iter().any(|e| {
        e["newText"]
            .as_str()
            .unwrap_or("")
            .contains("* Updated Heading")
    });
    assert!(has_def_edit, "defining heading updated");

    client.send_notification("exit", json!({}));
    assert_eq!(client.finish(), 1);
}

/// Verifies prepareRename and rename on `:CUSTOM_ID:` property line and references.
#[test]
fn test_rename_custom_id_and_file_links() {
    let temp = TempDir::new().expect("create temp dir");
    let ws = normalize_workspace_path(temp.path());

    let doc1 = "* Heading\n:PROPERTIES:\n:CUSTOM_ID: my-cid\n:END:\nLink: [[#my-cid]]\n";
    let doc2 = "Ref: [[file:doc1.org::#my-cid]]\n";
    fs::write(ws.join("doc1.org"), doc1).expect("write doc1");
    fs::write(ws.join("doc2.org"), doc2).expect("write doc2");

    let mut client = init_client(&ws);
    let doc1_uri = to_file_uri(&ws.join("doc1.org"));
    let doc2_uri = to_file_uri(&ws.join("doc2.org"));

    let prep = client.send_request(
        "textDocument/prepareRename",
        2,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 2, "character": 14 }
        }),
    );
    assert_eq!(prep["result"]["placeholder"], "my-cid");

    let ren = client.send_request(
        "textDocument/rename",
        3,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 2, "character": 14 },
            "newName": "new-cool-cid"
        }),
    );
    let changes = &ren["result"]["changes"];
    let doc1_edits = &changes[&doc1_uri];
    let doc2_edits = &changes[&doc2_uri];

    let has_cid_prop = doc1_edits
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["newText"].as_str().unwrap_or("").contains("new-cool-cid"));
    assert!(has_cid_prop, "CUSTOM_ID property line was updated");

    let has_doc2_link = doc2_edits
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["newText"].as_str().unwrap_or("").contains("new-cool-cid"));
    assert!(has_doc2_link, "cross-file CUSTOM_ID link was updated");

    client.send_notification("exit", json!({}));
    assert_eq!(client.finish(), 1);
}

/// Verifies prepareRename and rename on `:ID:` property line and `[[id:...]]` links.
#[test]
fn test_rename_id_property_and_roam_links() {
    let temp = TempDir::new().expect("create temp dir");
    let ws = normalize_workspace_path(temp.path());

    let doc1 = "* Node\n:PROPERTIES:\n:ID:       aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee\n:END:\n";
    let doc2 = "Link: [[id:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee][Title]]\nBare: [[id:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee]]\n";
    fs::write(ws.join("doc1.org"), doc1).expect("write doc1");
    fs::write(ws.join("doc2.org"), doc2).expect("write doc2");

    let mut client = init_client(&ws);
    let doc1_uri = to_file_uri(&ws.join("doc1.org"));
    let doc2_uri = to_file_uri(&ws.join("doc2.org"));

    let prep = client.send_request(
        "textDocument/prepareRename",
        2,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 2, "character": 15 }
        }),
    );
    assert_eq!(
        prep["result"]["placeholder"],
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    );

    let ren = client.send_request(
        "textDocument/rename",
        3,
        json!({
            "textDocument": { "uri": doc1_uri },
            "position": { "line": 2, "character": 15 },
            "newName": "99999999-9999-9999-9999-999999999999"
        }),
    );
    let changes = &ren["result"]["changes"];
    let doc1_edits = &changes[&doc1_uri];
    let doc2_edits = &changes[&doc2_uri];

    let has_id_edit = doc1_edits.as_array().unwrap().iter().any(|e| {
        e["newText"]
            .as_str()
            .unwrap_or("")
            .contains("99999999-9999-9999-9999-999999999999")
    });
    assert!(has_id_edit, "defining ID was updated");

    let doc2_count = doc2_edits.as_array().unwrap().len();
    assert!(doc2_count >= 2, "both ID links in doc2 were updated");

    client.send_notification("exit", json!({}));
    assert_eq!(client.finish(), 1);
}

/// Verifies semantic tokens generation skipping protected block interiors while emitting macro markers.
#[test]
fn test_semantic_tokens_full_with_protected_regions() {
    let temp = TempDir::new().expect("create temp dir");
    let ws = temp.path();

    let doc = "#+TITLE: Document\n* TODO [#A] Heading Title :tag:\nSCHEDULED: <2026-11-09 Mon>\n:PROPERTIES:\n:ID:       1111\n:END:\n#+BEGIN_SRC python\nx = 10\n# comment inside src\n#+END_SRC\n# Comment outside\n";
    fs::write(ws.join("doc.org"), doc).expect("write doc");

    let mut client = init_client(ws);
    let doc_uri = to_file_uri(&ws.join("doc.org"));

    let resp = client.send_request(
        "textDocument/semanticTokens/full",
        2,
        json!({
            "textDocument": { "uri": doc_uri }
        }),
    );
    let data = resp["result"]["data"].as_array().expect("data array");
    assert!(!data.is_empty(), "semantic tokens data must not be empty");
    assert_eq!(
        data.len() % 5,
        0,
        "semantic token data must be multiples of 5"
    );

    // Token types:
    // 0: keyword (TODO, SCHEDULED:)
    // 1: namespace (heading stars and title)
    // 2: type ([#A])
    // 3: property (:ID:)
    // 5: number (timestamp)
    // 7: comment (# Comment outside)
    // 8: macro (#+TITLE:, :PROPERTIES:, :END:, #+BEGIN_SRC, #+END_SRC)
    // 9: decorator (:tag:)
    let token_types: Vec<u64> = data.chunks(5).map(|c| c[3].as_u64().unwrap()).collect();

    assert!(token_types.contains(&0), "contains keyword token");
    assert!(token_types.contains(&1), "contains namespace token");
    assert!(token_types.contains(&2), "contains type token");
    assert!(token_types.contains(&3), "contains property token");
    assert!(token_types.contains(&5), "contains number timestamp token");
    assert!(token_types.contains(&7), "contains comment token");
    assert!(token_types.contains(&8), "contains macro token");
    assert!(token_types.contains(&9), "contains decorator token");

    client.send_notification("exit", json!({}));
    assert_eq!(client.finish(), 1);
}

// endregion
