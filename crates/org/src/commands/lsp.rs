// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Built-in Language Server Protocol (`org lsp`) JSON-RPC 2.0 stdio server.

pub(crate) mod diagnostics;
pub(crate) mod intelligence;
pub(crate) mod navigation;
pub(crate) mod refactor_tokens;

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueHint};
use serde_json::{json, Value};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::config::Config;
use org_tools_core::diagnostic::Span;
use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::files::collect_org_files;
use org_tools_core::source::SourceFile;

use self::diagnostics::{handle_doc_notification, record_request, route_request};
use super::mcp::confinement::resolve_startup_workspace;
use super::mcp::framing::{read_frame, write_frame, FramingMode, IncomingFrame};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI & Document Types

/// CLI arguments for `org lsp`.
#[derive(Debug, Clone, Args)]
pub(crate) struct LspArgs {
    /// Workspace root directory to index and serve over LSP stdio.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Explicit org-roam directory if different from workspace root.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) roam_dir: Option<PathBuf>,
}

/// An in-memory open buffer overlay managed by the LSP session.
#[derive(Debug)]
pub(crate) struct OpenDocument {
    /// Path to the Org file on disk.
    pub(crate) path: PathBuf,
    /// Client-provided file URI.
    pub(crate) uri: String,
    /// Document version counter from the client.
    pub(crate) version: i64,
    /// Active in-memory source file.
    pub(crate) source: SourceFile,
    /// Parsed Org document tree.
    pub(crate) doc: OrgDocument,
}

impl Clone for OpenDocument {
    /// Clones the open document overlay and its parsed AST.
    fn clone(&self) -> Self {
        let source = SourceFile::new(&self.path, self.source.content.clone());
        let doc = OrgDocument::from_source(&source);
        Self {
            path: self.path.clone(),
            uri: self.uri.clone(),
            version: self.version,
            source,
            doc,
        }
    }
}

/// Prometheus metrics and JSONL event telemetry store for LSP sessions.
#[derive(Debug, Default)]
pub(crate) struct LspTelemetry {
    /// Total requests processed by `(method, status)`.
    pub(crate) requests_total: HashMap<(String, String), u64>,
    /// Last observed request duration in seconds by `method`.
    pub(crate) request_durations: HashMap<String, f64>,
    /// Total diagnostics published by `severity`.
    pub(crate) diagnostics_published_total: HashMap<String, u64>,
}

// endregion

// region Session State

/// Persistent stdio LSP session state.
pub(crate) struct LspSession {
    /// Caller-facing workspace root directory path.
    pub(crate) display_workspace: PathBuf,
    /// Normalized default workspace root directory path.
    pub(crate) default_workspace: PathBuf,
    /// Optional explicit org-roam directory.
    pub(crate) roam_dir: Option<PathBuf>,
    /// Whether `--no-cache` was passed on the CLI.
    #[allow(dead_code)]
    pub(crate) no_cache: bool,
    /// Persistent open SQLite + FTS5 cache handle when enabled.
    #[allow(dead_code)]
    pub(crate) warm_cache: Option<WorkspaceCache>,
    /// In-memory open document overlay indexed by normalized path.
    pub(crate) open_docs: HashMap<PathBuf, OpenDocument>,
    /// Whether the client has sent `initialize`.
    pub(crate) is_initialized: bool,
    /// Whether the client has sent `shutdown`.
    pub(crate) is_shutdown: bool,
    /// Desired process exit code after `exit` notification.
    pub(crate) exit_code: Option<i32>,
    /// Telemetry metrics and event recorder.
    pub(crate) telemetry: LspTelemetry,
    /// Active framing mode detected on incoming messages.
    pub(crate) framing_mode: FramingMode,
    /// Whether `--workspace` was explicitly provided on the CLI.
    pub(crate) explicit_workspace: bool,
}

impl LspSession {
    /// Constructs a new [`LspSession`] initialized with discovered workspace boundaries.
    fn new(args: LspArgs, no_cache: bool) -> Result<Self, String> {
        let explicit_workspace = args.workspace.is_some();
        let display_ws = resolve_startup_workspace(args.workspace.as_deref())?;
        let default_ws = normalize_workspace_path(&display_ws);
        Ok(Self {
            display_workspace: display_ws,
            default_workspace: default_ws,
            roam_dir: args.roam_dir,
            no_cache,
            warm_cache: None,
            open_docs: HashMap::new(),
            is_initialized: false,
            is_shutdown: false,
            exit_code: None,
            telemetry: LspTelemetry::default(),
            framing_mode: FramingMode::ContentLength,
            explicit_workspace,
        })
    }

    /// Returns the effective configuration applying to `path`.
    pub(crate) fn effective_config_for(&self, path: &Path) -> Config {
        Config::load(path.parent().unwrap_or(&self.default_workspace)).unwrap_or_default()
    }

    /// Loads document from the open buffer overlay or parses from disk.
    pub(crate) fn get_or_load_document(
        &self,
        path: &Path,
    ) -> Result<(SourceFile, OrgDocument), String> {
        let norm = normalize_workspace_path(path);
        if let Some(open) = self.open_docs.get(&norm) {
            let source = SourceFile::new(&open.path, open.source.content.clone());
            let doc = OrgDocument::from_source(&source);
            return Ok((source, doc));
        }
        let source = SourceFile::from_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc = OrgDocument::from_source(&source);
        Ok((source, doc))
    }

    /// Collects all Org documents across the workspace and overlay buffers.
    pub(crate) fn collect_workspace_documents(&self) -> Vec<(PathBuf, SourceFile, OrgDocument)> {
        let mut search_paths = vec![self.default_workspace.clone()];
        if let Some(ref roam) = self.roam_dir {
            search_paths.push(roam.clone());
        }
        let disk_files = collect_org_files(&search_paths);
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for file in disk_files {
            let norm = normalize_workspace_path(&file);
            seen.insert(norm.clone());
            if let Some(open) = self.open_docs.get(&norm) {
                let src = SourceFile::new(&open.path, open.source.content.clone());
                results.push((norm, src, OrgDocument::from_source(&open.source)));
            } else if let Ok(src) = SourceFile::from_path(&file) {
                let doc = OrgDocument::from_source(&src);
                results.push((norm, src, doc));
            }
        }
        for (norm, open) in &self.open_docs {
            if seen.insert(norm.clone()) {
                let src = SourceFile::new(&open.path, open.source.content.clone());
                results.push((norm.clone(), src, OrgDocument::from_source(&open.source)));
            }
        }
        results
    }

    /// Extracts `textDocument.uri` from JSON params and resolves it to a normalized path.
    pub(crate) fn resolve_uri_and_path(
        &self,
        params: &Value,
    ) -> Result<(String, PathBuf), (i64, String)> {
        let uri = params
            .get("textDocument")
            .and_then(|td| td.get("uri"))
            .or_else(|| params.get("uri"))
            .and_then(|u| u.as_str())
            .ok_or_else(|| (-32602, "Missing textDocument.uri".to_string()))?;
        let path = file_uri_to_path(uri).map_err(|e| (-32602, e))?;
        Ok((uri.to_string(), path))
    }
}

// endregion

// region Coordinate & URI Helpers

/// Converts a filesystem path to a standard `file://` URI string.
#[allow(dead_code)]
pub(crate) fn path_to_file_uri(path: &Path) -> String {
    let s = normalize_workspace_path(path).to_string_lossy().to_string();
    let mut out = String::from("file://");
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Converts a `file://` URI string to a local filesystem [`PathBuf`].
pub(crate) fn file_uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("URI does not start with file://: {uri}"))?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or(""), 16)
            {
                bytes.push(b);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let s = String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 in URI: {e}"))?;
    #[cfg(windows)]
    let s = if s.starts_with('/') && s.chars().nth(2) == Some(':') {
        &s[1..]
    } else {
        &s
    };
    Ok(normalize_workspace_path(Path::new(&s)))
}

/// Translates zero-based line and UTF-16 character position into a UTF-8 byte offset.
pub(crate) fn utf16_pos_to_byte_offset(
    source: &SourceFile,
    line_0: u32,
    char_utf16_0: u32,
) -> Option<usize> {
    let idx = line_0 as usize;
    if idx >= source.line_count() {
        return None;
    }
    let s = source.line_start(idx);
    let e = if idx + 1 < source.line_count() {
        source.line_start(idx + 1)
    } else {
        source.content.len()
    };
    let mut u16_cnt = 0u32;
    let mut b_cnt = 0usize;
    for ch in source.content[s..e].chars() {
        if u16_cnt >= char_utf16_0 {
            break;
        }
        u16_cnt += ch.len_utf16() as u32;
        b_cnt += ch.len_utf8();
    }
    Some(s + b_cnt)
}

/// Translates a UTF-8 byte offset into a zero-based `(line, character)` UTF-16 position.
pub(crate) fn byte_offset_to_utf16_pos(source: &SourceFile, offset: usize) -> (u32, u32) {
    let clamped = offset.min(source.content.len());
    let (l1, _) = source.line_col(clamped);
    let l0 = l1.saturating_sub(1) as u32;
    let s = source.line_start(l0 as usize);
    let u16_chars: usize = source.content[s..clamped]
        .chars()
        .map(|c| c.len_utf16())
        .sum();
    (l0, u16_chars as u32)
}

/// Translates 1-based `(line, col)` into a zero-based `(line, character)` UTF-16 position.
pub(crate) fn line_col_1based_to_utf16_pos(
    source: &SourceFile,
    line_1: usize,
    col_1: usize,
) -> (u32, u32) {
    let l0 = line_1.saturating_sub(1) as u32;
    if (l0 as usize) >= source.line_count() {
        return (l0, 0);
    }
    let s = source.line_start(l0 as usize);
    byte_offset_to_utf16_pos(source, s + col_1.saturating_sub(1))
}

/// Builds an LSP `Range` JSON value from 0-based coordinates.
pub(crate) fn lsp_range(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Value {
    json!({ "start": { "line": start_line, "character": start_char }, "end": { "line": end_line, "character": end_char } })
}

/// Translates a byte [`Span`] into an LSP `Range` JSON object.
pub(crate) fn span_to_lsp_range(source: &SourceFile, span: Span) -> Value {
    let (sl, sc) = byte_offset_to_utf16_pos(source, span.start);
    let (el, ec) = byte_offset_to_utf16_pos(source, span.end);
    lsp_range(sl, sc, el, ec)
}

// endregion

// region Dispatch & Event Loop

/// Evaluates a parsed JSON-RPC payload, writing the response frame and returning whether to continue.
fn evaluate_payload<W: Write>(session: &mut LspSession, writer: &mut W, payload: &Value) -> bool {
    let method = payload.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if payload.get("id").is_none() {
        if method == "exit" {
            session.exit_code = Some(if session.is_shutdown { 0 } else { 1 });
            return false;
        }
        handle_doc_notification(
            session,
            writer,
            method,
            payload.get("params").unwrap_or(&Value::Null),
        );
        return true;
    }
    let id = &payload["id"];
    let started = Instant::now();

    if session.is_shutdown || (!session.is_initialized && method != "initialize") {
        let msg = if session.is_shutdown {
            "Server is shut down"
        } else {
            "Server not initialized"
        };
        record_request(session, method, "error", started.elapsed().as_secs_f64());
        let _ = write_frame(
            writer,
            session.framing_mode,
            &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32600, "message": msg } }),
        );
        return true;
    }

    let params = payload.get("params").unwrap_or(&Value::Null);
    let resp = match route_request(session, method, params) {
        Ok(res) => {
            record_request(session, method, "ok", started.elapsed().as_secs_f64());
            json!({ "jsonrpc": "2.0", "id": id, "result": res })
        }
        Err((code, message)) => {
            record_request(session, method, "error", started.elapsed().as_secs_f64());
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    };
    let _ = write_frame(writer, session.framing_mode, &resp);
    true
}

/// Runs the `org lsp` stdio JSON-RPC 2.0 language server loop.
pub(crate) fn run_lsp(args: LspArgs, no_cache: bool) -> i32 {
    let mut session = match LspSession::new(args, no_cache) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org lsp: {e}");
            return EXIT_ERROR;
        }
    };
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout().lock();

    loop {
        match read_frame(&mut reader) {
            Ok(Some(IncomingFrame::Message { mode, raw_json })) => {
                session.framing_mode = mode;
                if let Ok(payload) = serde_json::from_str::<Value>(&raw_json) {
                    if !evaluate_payload(&mut session, &mut stdout, &payload) {
                        break;
                    }
                }
            }
            Ok(Some(IncomingFrame::ProtocolError {
                mode,
                code,
                message,
            })) => {
                let err = json!({ "jsonrpc": "2.0", "id": Value::Null, "error": { "code": code, "message": message } });
                let _ = write_frame(&mut stdout, mode, &err);
            }
            Ok(None) | Err(_) => break,
        }
    }
    session.exit_code.unwrap_or(EXIT_OK)
}

// endregion
