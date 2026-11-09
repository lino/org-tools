// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Built-in zero-Emacs Model Context Protocol (`org mcp`) JSON-RPC 2.0 `stdio` server.

pub(crate) mod confinement;
pub(crate) mod framing;
mod schema;
mod tools_clock;
mod tools_lifecycle;
mod tools_planning;
mod tools_read;
mod tools_show;
mod tools_write;

use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use serde_json::{json, Map, Value};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::files::collect_org_files;

use self::confinement::resolve_startup_workspace;
use self::framing::{read_frame, write_frame, IncomingFrame};
use self::schema::{
    build_tools_list_result, jsonrpc_err, jsonrpc_ok, optional_str, reject_unknown_keys,
    ALL_TOOL_NAMES, DEFAULT_PROTOCOL_VERSION, ERR_INVALID_PARAMS, ERR_INVALID_REQUEST,
    ERR_METHOD_NOT_FOUND, ERR_PARSE_ERROR, SERVER_NAME, TOOL_CLOCK_IN, TOOL_CLOCK_OUT,
    TOOL_FMT_FILE, TOOL_QUERY_AGENDA, TOOL_QUERY_BLOCKED, TOOL_QUERY_NEXT, TOOL_REFILE,
    TOOL_ROAM_BACKLINKS, TOOL_ROAM_NODE, TOOL_ROAM_SEARCH, TOOL_SHOW, TOOL_UPDATE_PLANNING,
    TOOL_UPDATE_STATE,
};
use self::tools_clock::{exec_clock_in, exec_clock_out};
use self::tools_lifecycle::exec_refile;
use self::tools_planning::{exec_query_agenda, exec_update_planning};
use self::tools_read::{
    exec_query_blocked, exec_query_next, exec_roam_backlinks, exec_roam_node, exec_roam_search,
};
use self::tools_show::exec_show;
use self::tools_write::{exec_fmt_file, exec_update_state};
use super::MutationTelemetry;
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI arguments for `org mcp`.
#[derive(Debug, Clone, Args)]
pub(crate) struct McpArgs {
    /// Workspace root directory to index and serve over MCP `stdio`.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
}

// endregion

// region Session State

/// Persistent `stdio` MCP session holding the warm [`WorkspaceCache`] SQLite connection.
pub(super) struct McpSession {
    /// Caller-facing workspace root directory path.
    display_workspace: PathBuf,
    /// Normalized default workspace root directory path.
    default_workspace: PathBuf,
    /// Whether `--no-cache` was supplied on the CLI.
    pub(super) no_cache: bool,
    /// Persistent open SQLite + FTS5 cache handle when `!no_cache`.
    warm_cache: Option<WorkspaceCache>,
}

impl McpSession {
    /// Initializes an [`McpSession`] lazily without creating `.cache/org-cache.db` until a tool runs.
    fn new(display_workspace: PathBuf, no_cache: bool) -> Result<Self, String> {
        let default_workspace = normalize_workspace_path(&display_workspace);
        Ok(Self {
            display_workspace,
            default_workspace,
            no_cache,
            warm_cache: None,
        })
    }

    /// Validates `allowed` keys in `args` and resolves `(display_ws, norm_ws, is_default)`.
    pub(super) fn prepare_workspace(
        &self,
        args: &Map<String, Value>,
        allowed: &[&str],
    ) -> Result<(PathBuf, PathBuf, bool), String> {
        reject_unknown_keys(args, allowed)?;
        let ws = optional_str(args, "workspace")?;
        self.resolve_call_workspace(ws.as_deref())
    }

    /// Resolves an optional per-call `workspace` override into `(display_ws, norm_ws, is_default)`.
    fn resolve_call_workspace(
        &self,
        arg_ws: Option<&str>,
    ) -> Result<(PathBuf, PathBuf, bool), String> {
        let Some(raw) = arg_ws.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok((
                self.display_workspace.clone(),
                self.default_workspace.clone(),
                true,
            ));
        };
        let raw_path = PathBuf::from(raw);
        let candidate = if raw_path.is_dir() || raw_path.is_absolute() {
            raw_path
        } else {
            self.display_workspace.join(&raw_path)
        };
        if !candidate.is_dir() {
            return Err(format!("workspace directory not found: {raw}"));
        }
        let norm = normalize_workspace_path(&candidate);
        let is_default = norm == self.default_workspace;
        Ok((candidate, norm, is_default))
    }

    /// Opens or refreshes the persistent [`WorkspaceCache`] (or scans directly) and builds telemetry.
    pub(super) fn sync_telemetry(
        &mut self,
        norm_ws: &Path,
        is_default: bool,
        started: Instant,
    ) -> Result<MutationTelemetry, String> {
        if !self.no_cache && is_default {
            let tel = match self.warm_cache {
                Some(ref mut cache) => cache.refresh()?,
                None => {
                    let (cache, tel) = WorkspaceCache::open_and_sync(&self.default_workspace)?;
                    self.warm_cache = Some(cache);
                    tel
                }
            };
            return Ok(self.counts_telemetry(
                started,
                (tel.files_scanned, tel.cache_hits, tel.cache_misses),
            ));
        }
        if !self.no_cache {
            let (_, tel) = WorkspaceCache::open_and_sync(norm_ws)?;
            return Ok(self.counts_telemetry(
                started,
                (tel.files_scanned, tel.cache_hits, tel.cache_misses),
            ));
        }
        let scanned = collect_org_files(&[norm_ws.to_path_buf()]).len();
        Ok(self.counts_telemetry(started, (scanned, 0, scanned)))
    }

    /// Marks written `paths` stale in `WorkspaceCache` prior to `refresh()` for coarse-tick filesystems.
    pub(super) fn invalidate_workspace_paths(
        &mut self,
        norm_ws: &Path,
        is_default: bool,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        if self.no_cache || paths.is_empty() {
            return Ok(());
        }
        if is_default {
            if let Some(ref mut cache) = self.warm_cache {
                cache.invalidate_paths(paths)?;
                return Ok(());
            }
        }
        if WorkspaceCache::resolve_db_path(norm_ws).is_file() {
            let (mut cache, _) = WorkspaceCache::open_and_sync(norm_ws)?;
            cache.invalidate_paths(paths)?;
            if is_default {
                self.warm_cache = Some(cache);
            }
        }
        Ok(())
    }

    /// Returns a shared reference to the warm [`WorkspaceCache`] when serving the default workspace.
    pub(super) fn warm_cache_for(&self, is_default: bool) -> Option<&WorkspaceCache> {
        self.warm_cache
            .as_ref()
            .filter(|_| !self.no_cache && is_default)
    }

    /// Builds a zero-scan [`MutationTelemetry`] snapshot for fast argument-validation errors.
    pub(super) fn empty_telemetry(&self, started: Instant) -> MutationTelemetry {
        self.counts_telemetry(started, (0, 0, 0))
    }

    /// Builds [`MutationTelemetry`] from `started` and `(scanned, hits, misses)` counters.
    pub(super) fn counts_telemetry(
        &self,
        started: Instant,
        counts: (usize, usize, usize),
    ) -> MutationTelemetry {
        MutationTelemetry {
            duration_us: started.elapsed().as_micros() as u64,
            files_scanned: counts.0,
            cache_hits: counts.1,
            cache_misses: counts.2,
        }
    }
}

// endregion

// region JSON-RPC 2.0 Method Dispatch

/// Handles the MCP `initialize` handshake request.
fn handle_initialize(id: Value, params: Option<&Value>) -> Value {
    let proto = match params {
        None => DEFAULT_PROTOCOL_VERSION,
        Some(v) if v.is_null() => DEFAULT_PROTOCOL_VERSION,
        Some(Value::Object(map)) => map
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_PROTOCOL_VERSION),
        Some(_) => {
            return jsonrpc_err(
                id,
                ERR_INVALID_PARAMS,
                "Invalid params: 'params' must be an object",
            );
        }
    };
    jsonrpc_ok(
        id,
        json!({
            "protocolVersion": proto,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") }
        }),
    )
}

/// Validates optional `params` for parameterless MCP methods (`ping`, `tools/list`).
fn ensure_optional_object_params(params: Option<&Value>) -> Result<(), &'static str> {
    match params {
        None => Ok(()),
        Some(v) if v.is_null() || v.is_object() => Ok(()),
        Some(_) => Err("Invalid params: 'params' must be an object"),
    }
}

/// Dispatches read-only roam, query, agenda, and show MCP tools (`Some`), or returns `None`.
fn try_dispatch_read_tool(
    session: &mut McpSession,
    name: &str,
    args: &Map<String, Value>,
    started: Instant,
) -> Option<Value> {
    match name {
        TOOL_ROAM_NODE => Some(exec_roam_node(session, args, started)),
        TOOL_ROAM_BACKLINKS => Some(exec_roam_backlinks(session, args, started)),
        TOOL_ROAM_SEARCH => Some(exec_roam_search(session, args, started)),
        TOOL_QUERY_NEXT => Some(exec_query_next(session, args, started)),
        TOOL_QUERY_BLOCKED => Some(exec_query_blocked(session, args, started)),
        TOOL_QUERY_AGENDA => Some(exec_query_agenda(session, args, started)),
        TOOL_SHOW => Some(exec_show(session, args, started)),
        _ => None,
    }
}

/// Dispatches an individual `tools/call` invocation to its read or write handler.
fn dispatch_named_tool(session: &mut McpSession, name: &str, args: &Map<String, Value>) -> Value {
    let started = Instant::now();
    if let Some(res) = try_dispatch_read_tool(session, name, args, started) {
        return res;
    }
    match name {
        TOOL_UPDATE_STATE => exec_update_state(session, args, started),
        TOOL_UPDATE_PLANNING => exec_update_planning(session, args, started),
        TOOL_FMT_FILE => exec_fmt_file(session, args, started),
        TOOL_REFILE => exec_refile(session, args, started),
        TOOL_CLOCK_IN => exec_clock_in(session, args, started),
        TOOL_CLOCK_OUT => exec_clock_out(session, args, started),
        _ => unreachable!("tool name pre-validated against ALL_TOOL_NAMES"),
    }
}

/// Validates `tools/call` parameters and executes the requested MCP tool.
fn handle_tools_call(session: &mut McpSession, id: Value, params: Option<&Value>) -> Value {
    let Some(Value::Object(param_map)) = params else {
        return jsonrpc_err(id, ERR_INVALID_PARAMS, "Invalid params: object required");
    };
    let Some(tool_name) = param_map.get("name").and_then(Value::as_str) else {
        return jsonrpc_err(id, ERR_INVALID_PARAMS, "Invalid params: missing 'name'");
    };
    if !ALL_TOOL_NAMES.contains(&tool_name) {
        let msg = format!("Invalid params: unknown tool '{tool_name}'");
        return jsonrpc_err(id, ERR_INVALID_PARAMS, msg);
    }
    let empty_args = Map::new();
    let args_ref = match param_map.get("arguments") {
        None => &empty_args,
        Some(v) if v.is_null() => &empty_args,
        Some(Value::Object(m)) => m,
        Some(_) => {
            return jsonrpc_err(id, ERR_INVALID_PARAMS, "Invalid params: 'arguments' object");
        }
    };
    jsonrpc_ok(id, dispatch_named_tool(session, tool_name, args_ref))
}

/// Validates a parsed JSON-RPC 2.0 request object and extracts `(id_opt, method, params)`.
fn extract_jsonrpc_call(parsed: &Value) -> Result<(Option<Value>, &str, Option<&Value>), Value> {
    let Some(obj) = parsed.as_object() else {
        return Err(jsonrpc_err(
            Value::Null,
            ERR_INVALID_REQUEST,
            "Invalid Request: expected object",
        ));
    };
    let id_field = obj.get("id");
    if id_field.is_some_and(|v| !(v.is_string() || v.is_number() || v.is_null())) {
        return Err(jsonrpc_err(
            Value::Null,
            ERR_INVALID_REQUEST,
            "Invalid Request: bad 'id'",
        ));
    }
    let reply_id = id_field.cloned().unwrap_or(Value::Null);
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(jsonrpc_err(
            reply_id,
            ERR_INVALID_REQUEST,
            "Invalid Request: jsonrpc != 2.0",
        ));
    }
    let Some(method) = obj
        .get("method")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
    else {
        return Err(jsonrpc_err(
            reply_id,
            ERR_INVALID_REQUEST,
            "Invalid Request: missing 'method'",
        ));
    };
    Ok((id_field.cloned(), method, obj.get("params")))
}

/// Parses and routes one JSON-RPC 2.0 frame, returning `None` for notifications.
fn dispatch_raw_json(session: &mut McpSession, raw_json: &str) -> Option<Value> {
    let parsed: Value = match serde_json::from_str(raw_json) {
        Ok(v) => v,
        Err(err) => {
            return Some(jsonrpc_err(
                Value::Null,
                ERR_PARSE_ERROR,
                format!("Parse error: {err}"),
            ));
        }
    };
    let (id_opt, method, params) = match extract_jsonrpc_call(&parsed) {
        Ok(parts) => parts,
        Err(err_resp) => return Some(err_resp),
    };
    let id = id_opt?;

    let response = match method {
        "initialize" => handle_initialize(id, params),
        "ping" => ensure_optional_object_params(params)
            .map(|()| jsonrpc_ok(id.clone(), json!({})))
            .unwrap_or_else(|msg| jsonrpc_err(id, ERR_INVALID_PARAMS, msg)),
        "tools/list" => ensure_optional_object_params(params)
            .map(|()| jsonrpc_ok(id.clone(), build_tools_list_result()))
            .unwrap_or_else(|msg| jsonrpc_err(id, ERR_INVALID_PARAMS, msg)),
        "tools/call" => handle_tools_call(session, id, params),
        _ => jsonrpc_err(
            id,
            ERR_METHOD_NOT_FOUND,
            format!("Method not found: '{method}'"),
        ),
    };
    Some(response)
}

// endregion

// region Entrypoint

/// Processes the `stdin` frame stream until EOF or unrecoverable I/O error.
fn serve_stdio_loop(session: &mut McpSession) -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(f)) => f,
            Ok(None) => return EXIT_OK,
            Err(err) => {
                eprintln!("org: mcp stdin error: {err}");
                return EXIT_ERROR;
            }
        };
        let (mode, reply_opt) = match frame {
            IncomingFrame::Message { mode, raw_json } => {
                (mode, dispatch_raw_json(session, &raw_json))
            }
            IncomingFrame::ProtocolError {
                mode,
                code,
                message,
            } => (mode, Some(jsonrpc_err(Value::Null, code, message))),
        };
        if let Some(reply) = reply_opt {
            if let Err(err) = write_frame(&mut writer, mode, &reply) {
                return if err.kind() == io::ErrorKind::BrokenPipe {
                    EXIT_OK
                } else {
                    eprintln!("org: mcp stdout error: {err}");
                    EXIT_ERROR
                };
            }
        }
    }
}

/// Runs the `org mcp` JSON-RPC 2.0 server over `stdin`/`stdout` until `stdin` reaches EOF.
pub(crate) fn run_mcp(args: McpArgs, no_cache: bool) -> i32 {
    let workspace = match resolve_startup_workspace(args.workspace.as_deref()) {
        Ok(ws) => ws,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let mut session = match McpSession::new(workspace, no_cache) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    serve_stdio_loop(&mut session)
}

// endregion
