// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! End-to-end `stdio` MCP server integration tests.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::files::join_slash_rel;
use serde_json::{json, Value};
use tempfile::TempDir;

// region Constants & Path Helpers

/// Forbidden machine-specific path prefix in repository configuration files.
const FORBIDDEN_USER_PATH_PREFIX: &str = "/Users/";
/// Expected 16 MCP tool names exposed by `org mcp`.
#[rustfmt::skip]
const EXPECTED_TOOLS: &[&str] = &[
    "org_roam_node", "org_roam_backlinks", "org_roam_search", "org_query_next",
    "org_query_blocked", "org_query_agenda", "org_show", "org_update_state",
    "org_update_planning", "org_fmt_file", "org_refile", "org_clock_in",
    "org_clock_out", "org_roam_capture", "org_fleeting_list", "org_fleeting_transform",
];
/// UUID for the hub specification file node in the synthetic workspace.
const HUB_ID: &str = "10000000-0000-4000-8000-000000000101";
/// UUID for the upstream blocker task in `tasks.org`.
const BLOCKER_ID: &str = "10000000-0000-4000-8000-000000000102";
/// UUID for the downstream blocked task in `roadmap.org`.
const BLOCKED_ID: &str = "10000000-0000-4000-8000-000000000103";

/// Returns the repository workspace root derived from `CARGO_MANIFEST_DIR`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

/// Writes UTF-8 fixture text to `path`, creating parent directories as needed.
fn write_file_str(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directories");
    }
    fs::write(path, content).expect("write fixture");
}

/// Seeds a three-file workspace with roam links, edna triggers, checklists, and dirty formatting.
fn seed_mcp_workspace(root: &Path) {
    let hub = format!(":PROPERTIES:\n:ID:           {HUB_ID}\n:ROAM_ALIASES: \"Roam Spec\" \"WP07-Hub\"\n:END:\n#+TITLE: Hub Architecture\n#+FILETAGS: :spec:\n\nCore architecture note.\n\n* NEXT [#A] Implement SearchEngine probe                               :@code:\n:PROPERTIES:\n:CUSTOM_ID: probe-task\n:END:\n- [X] Wire schema\n- [ ] Verify telemetry (v2 — café)\nBody mentions SearchEngine (v2 engine) for BM25 search.\n");
    let tasks = format!("#+TITLE: Sprint Tasks\n\n* TODO Finish upstream foundation                                      :@code:\n:PROPERTIES:\n:ID:      {BLOCKER_ID}\n:TRIGGER: ids(\"{BLOCKED_ID}\") todo!(\"NEXT\")\n:END:\nImplements [[id:{HUB_ID}][Hub Architecture]] prerequisites.\n");
    let roadmap = format!("#+TITLE: Roadmap\n\n* TODO Ship downstream deliverable                                     :@ops:\n:PROPERTIES:\n:ID: {BLOCKED_ID}\n:BLOCKER: ids(\"{BLOCKER_ID}\")\n:END:\nTrailing spaces here   \n| Name | Count |\n|---|---|\n| alpha | 1 |\nReferences [[id:{HUB_ID}][Hub Spec]].\n");
    write_file_str(&join_slash_rel(root, "specs/hub.org"), &hub);
    write_file_str(&join_slash_rel(root, "plan/tasks.org"), &tasks);
    write_file_str(&join_slash_rel(root, "plan/roadmap.org"), &roadmap);
}

/// Asserts that `telemetry` carries `duration_us` and exact `(scanned, hits, misses)` counts.
fn assert_telemetry(tel: &Value, expected: (u64, u64, u64)) {
    assert!(tel["duration_us"].is_u64(), "missing duration_us: {tel}");
    assert_eq!(tel["files_scanned"].as_u64(), Some(expected.0));
    assert_eq!(tel["cache_hits"].as_u64(), Some(expected.1));
    assert_eq!(tel["cache_misses"].as_u64(), Some(expected.2));
}

// endregion

// region Stdio MCP Client Harness

/// Interactive `stdio` client wrapping a spawned `org mcp` child process.
struct McpTestClient {
    /// Spawned `org mcp` child process.
    child: Child,
    /// Writable handle to child `stdin`.
    stdin: Option<ChildStdin>,
    /// Buffered reader over child `stdout`.
    stdout: BufReader<ChildStdout>,
}

impl McpTestClient {
    /// Spawns `org` with `args`, optional `cwd`, and environment pairs over piped `stdio`.
    fn spawn_in(args: &[&str], cwd: Option<&Path>, envs: &[(&str, &Path)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn org mcp process");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Spawns `org mcp --workspace <dir>` with piped `stdio`.
    fn spawn_ws(dir: &TempDir) -> Self {
        Self::spawn_in(
            &["mcp", "--workspace", dir.path().to_str().unwrap()],
            None,
            &[],
        )
    }

    /// Writes raw text/bytes directly to child `stdin` and flushes.
    fn write_raw(&mut self, raw: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(raw.as_bytes()).expect("write raw stdin");
        stdin.flush().expect("flush stdin");
    }

    /// Sends a newline-delimited JSON-RPC message and reads one newline-delimited JSON response.
    fn send_line(&mut self, msg: &Value) -> Value {
        self.write_raw(&format!("{}\n", serde_json::to_string(msg).unwrap()));
        self.read_line_json()
    }

    /// Reads one newline-delimited JSON-RPC frame from `stdout`.
    fn read_line_json(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.stdout.read_line(&mut line).expect("read line") > 0);
        serde_json::from_str(&line).expect("stdout frame must be valid JSON")
    }

    /// Sends a `Content-Length` framed JSON-RPC message (with optional leading headers) and reads reply.
    fn send_framed(&mut self, extra_headers: &str, msg: &Value) -> Value {
        let body = serde_json::to_string(msg).unwrap();
        let len = body.len();
        self.write_raw(&format!(
            "{extra_headers}Content-Length: {len}\r\n\r\n{body}"
        ));
        let mut header = String::new();
        self.stdout.read_line(&mut header).expect("read header");
        let len_str = header.trim().strip_prefix("Content-Length:").expect("hdr");
        let len: usize = len_str.trim().parse().expect("Content-Length integer");
        let mut sep = String::new();
        self.stdout.read_line(&mut sep).expect("read separator");
        assert!(sep.trim().is_empty());
        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf).expect("read body");
        serde_json::from_slice(&buf).expect("valid JSON body")
    }

    /// Invokes `tools/call` for `name` with `arguments` and returns `(is_error, structuredContent)`.
    fn call_tool(&mut self, id: i64, name: &str, arguments: Value) -> (bool, Value) {
        let resp = self.send_line(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }));
        let res = &resp["result"];
        let is_err = res["isError"].as_bool().expect("CallToolResult.isError");
        let text = res["content"][0]["text"].as_str().expect("content text");
        let structured = res["structuredContent"].clone();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), structured);
        (is_err, structured)
    }

    /// Invokes `tools/call`, asserts `isError == false`, and returns `structuredContent`.
    fn call_ok(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        let (is_err, payload) = self.call_tool(id, name, arguments);
        assert!(!is_err, "unexpected tool error on {name}: {payload}");
        payload
    }

    /// Invokes `tools/call` and asserts `isError == true` with a string `error` message.
    fn assert_err(&mut self, id: i64, name: &str, arguments: Value) {
        let (is_err, p) = self.call_tool(id, name, arguments);
        assert!(is_err && p["error"].is_string(), "expected err {name}: {p}");
    }

    /// Closes `stdin`, waits for process exit, and asserts exit code `0` with zero trailing stdout.
    fn close_cleanly(mut self) {
        drop(self.stdin.take());
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).expect("drain stdout");
        assert!(rest.is_empty(), "extra stdout bytes: {rest:?}");
        assert!(self.child.wait().expect("wait exit").success());
    }
}

// endregion

// region Slice 1: Handshake, Dual Framing, Discovery & JSON-RPC Errors

/// Verifies lazy startup, `initialize`, notifications, unordered HTTP headers, and `tools/list`.
#[test]
fn slice1_initialize_notifications_dual_framing_and_tools_list_schemas() {
    let dir = TempDir::new().unwrap();
    seed_mcp_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);

    let init = client.send_line(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2024-11-05" }
    }));
    let res = &init["result"];
    assert_eq!(res["protocolVersion"], "2024-11-05");
    assert_eq!(res["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(res["serverInfo"]["name"], "org-tools");

    client.write_raw("\n   \r\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
    client.write_raw("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\"}\n");
    let ctype = "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n";
    let ping = client.send_framed(ctype, &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
    assert_eq!((&ping["id"], &ping["result"]), (&json!(2), &json!({})));

    let list_req = json!({"jsonrpc": "2.0", "id": "L1", "method": "tools/list"});
    let list = client.send_framed("", &list_req);
    let tools = list["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, EXPECTED_TOOLS);
    for tool in tools {
        assert!(!tool["description"].as_str().unwrap_or("").is_empty());
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    client.close_cleanly();
    assert!(!WorkspaceCache::resolve_db_path(dir.path()).exists());
}

/// Verifies JSON-RPC 2.0 error codes (`-32700`, `-32600`, `-32601`, `-32602`) keep the session alive.
#[test]
fn slice1_jsonrpc_error_codes_keep_stdio_session_alive() {
    let dir = TempDir::new().unwrap();
    let mut client = McpTestClient::spawn_ws(&dir);

    client.write_raw("{malformed json\n");
    assert_eq!(client.read_line_json()["error"]["code"], -32700);
    let r1 = client.send_line(&json!({"jsonrpc": "1.0", "id": 10, "method": "ping"}));
    assert_eq!(r1["error"]["code"], -32600);
    let r2 = client.send_line(&json!({"jsonrpc": "2.0", "id": 11, "method": "no/such"}));
    assert_eq!(r2["error"]["code"], -32601);
    let r3 = client.send_line(&json!({
        "jsonrpc": "2.0", "id": 12, "method": "tools/call",
        "params": { "name": "unknown_tool", "arguments": {} }
    }));
    assert_eq!(r3["error"]["code"], -32602);
    let ok = client.send_line(&json!({"jsonrpc": "2.0", "id": 13, "method": "ping"}));
    assert_eq!(ok["result"], json!({}));
    client.close_cleanly();
}

// endregion

// region Slice 2: Warm WorkspaceCache Read/Write Tools & Telemetry

/// Verifies cold Call 1 (`cache_misses == 3`) and warm subsequent calls (`cache_hits == 3`) on roam/search.
#[test]
fn slice2_roam_and_search_tools_use_warm_cache_and_handle_domain_errors() {
    let dir = TempDir::new().unwrap();
    seed_mcp_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);

    let node = client.call_ok(1, "org_roam_node", json!({ "query": "Roam Spec" }));
    assert_eq!(node["nodes"][0]["id"], HUB_ID);
    assert_telemetry(&node["telemetry"], (3, 0, 3));
    assert!(WorkspaceCache::resolve_db_path(dir.path()).exists());

    let empty = client.call_ok(2, "org_roam_node", json!({ "query": "NoSuchNode" }));
    assert!(empty["nodes"].as_array().unwrap().is_empty());
    assert_telemetry(&empty["telemetry"], (3, 3, 0));

    let bl = client.call_ok(3, "org_roam_backlinks", json!({ "target": HUB_ID }));
    assert_eq!(bl["backlinks"].as_array().unwrap().len(), 2);
    assert_telemetry(&bl["telemetry"], (3, 3, 0));
    client.assert_err(4, "org_roam_backlinks", json!({ "target": "id:missing" }));

    let q_fts = json!({ "query": "body:\"SearchEngine\"" });
    let s1 = client.call_ok(5, "org_roam_search", q_fts);
    assert_eq!(s1["count"], 1);
    assert_telemetry(&s1["telemetry"], (3, 3, 0));
    let s2 = client.call_ok(6, "org_roam_search", json!({ "query": "SearchEngine (v2" }));
    assert_eq!(s2["count"], 1);
    client.close_cleanly();
}

/// Verifies `org_query_next`, `org_query_blocked`, and `org_show` cold-to-warm telemetry and slicing.
#[test]
fn slice2_query_and_show_tools_extract_slices_with_warm_telemetry() {
    let dir = TempDir::new().unwrap();
    seed_mcp_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);

    let q_next = json!({ "context": "@code", "limit": 5 });
    let next = client.call_ok(1, "org_query_next", q_next);
    assert_eq!(next["count"], 2);
    assert_telemetry(&next["telemetry"], (3, 0, 3));

    let blk = client.call_ok(2, "org_query_blocked", json!({}));
    assert_eq!(blk["count"], 1);
    assert_eq!(blk["entries"][0]["locator"], format!("id:{BLOCKED_ID}"));
    assert_telemetry(&blk["telemetry"], (3, 3, 0));

    let sh_args = json!({"locator": "specs/hub.org::#probe-task", "format": "md", "max_chars": 40});
    let sh = client.call_ok(3, "org_show", sh_args);
    assert_eq!(sh["format"], "md");
    assert_eq!(sh["truncated"], true);
    assert_eq!(sh["char_count"], 40);
    assert_eq!(sh["checklist_summary"]["total"], 2);
    assert_telemetry(&sh["telemetry"], (3, 3, 0));
    client.assert_err(4, "org_show", json!({ "locator": "id:missing" }));
    client.close_cleanly();
}

/// Verifies `org_update_state` (`:TRIGGER:` cascade) and `org_fmt_file` mutate disk and refresh `org-cache.db`.
#[test]
fn slice2_write_tools_mutate_cascade_and_refresh_warm_workspace_cache() {
    let dir = TempDir::new().unwrap();
    seed_mcp_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);
    let loc = format!("id:{BLOCKER_ID}");
    let dry_args = json!({ "locator": loc, "state": "DONE", "dry_run": true });
    let dry = client.call_ok(1, "org_update_state", dry_args);
    assert_eq!(dry["files_modified"], 2);
    assert_telemetry(&dry["telemetry"], (3, 0, 3));
    let up_args = json!({ "locator": loc, "state": "DONE" });
    let up = client.call_ok(2, "org_update_state", up_args);
    assert_eq!(up["files_modified"], 2);
    assert_eq!(up["entries_affected"], 2);
    assert_eq!(up["cascaded"].as_array().unwrap().len(), 1);
    assert_telemetry(&up["telemetry"], (3, 1, 2));
    let blk = client.call_ok(3, "org_query_blocked", json!({}));
    assert_eq!(blk["count"], 0);
    assert_telemetry(&blk["telemetry"], (3, 3, 0));
    let fmt_res = client.call_ok(4, "org_fmt_file", json!({ "path": "plan/roadmap.org" }));
    assert_eq!(fmt_res["changed"], true);
    assert_telemetry(&fmt_res["telemetry"], (3, 2, 1));
    let roadmap_text = fs::read_to_string(join_slash_rel(dir.path(), "plan/roadmap.org")).unwrap();
    assert!(roadmap_text.contains("| alpha |     1 |"));
    let after_read = client.call_ok(5, "org_roam_node", json!({ "query": BLOCKED_ID }));
    assert_eq!(after_read["nodes"][0]["todo_keyword"], "NEXT");
    assert_telemetry(&after_read["telemetry"], (3, 3, 0));
    client.close_cleanly();
}

// endregion

// region Slice 3: Security Confinement, Coarse-Tick Invalidation & .mcp.json Contract

/// Verifies `--no-cache`, `ORG_TOOLS_WORKSPACE`, `.mcp.json`, and coarse-tick `invalidate_paths`.
#[test]
fn slice3_no_cache_env_mcp_json_and_coarse_tick_invalidation() {
    let dir = TempDir::new().unwrap();
    seed_mcp_workspace(dir.path());
    let envs = [("ORG_TOOLS_WORKSPACE", dir.path())];
    let mut client = McpTestClient::spawn_in(&["--no-cache", "mcp"], None, &envs);
    let node = client.call_ok(1, "org_roam_node", json!({ "query": HUB_ID }));
    assert_eq!(node["nodes"][0]["id"], HUB_ID);
    assert_telemetry(&node["telemetry"], (3, 0, 3));
    assert!(!WorkspaceCache::resolve_db_path(dir.path()).exists());
    client.close_cleanly();

    let raw_mcp = fs::read_to_string(repo_root().join(".mcp.json")).expect("read .mcp.json");
    assert!(!raw_mcp.contains(FORBIDDEN_USER_PATH_PREFIX));
    let parsed: Value = serde_json::from_str(&raw_mcp).expect("valid .mcp.json");
    let expected_org = json!({ "command": "org", "args": ["mcp"], "type": "stdio" });
    assert_eq!(parsed["mcpServers"]["org"], expected_org);

    let hub = join_slash_rel(dir.path(), "specs/hub.org");
    let (mut cache, _) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    let frozen_mtime = fs::metadata(&hub).unwrap().modified().unwrap();
    let raw_hub = fs::read_to_string(&hub).unwrap();
    fs::write(&hub, raw_hub.replacen("* NEXT [#A]", "* TODO [#A]", 1)).unwrap();
    let fh = OpenOptions::new().write(true).open(&hub).unwrap();
    fh.set_modified(frozen_mtime).unwrap();
    drop(fh);
    cache.invalidate_paths(&[hub]).unwrap();
    let rt = cache.refresh().unwrap();
    let counts = (rt.files_scanned, rt.cache_hits, rt.cache_misses);
    assert_eq!(counts, (3, 2, 1));
}

/// Verifies workspace confinement, symlink rejection, `.org` guard, and `cwd != workspace` locators.
#[test]
fn slice3_workspace_confinement_symlink_and_extension_guards() {
    let (ws, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    seed_mcp_workspace(ws.path());
    let outside_org = outside.path().join("outside.org");
    let md_path = ws.path().join("README.md");
    let outside_orig = "* TODO Secret outside task\nConfidential body.\n";
    let md_orig = "# Notes\n\n| Col | Val |\n| --- | --- |\n| a | 1 |\n";
    write_file_str(&outside_org, outside_orig);
    write_file_str(&md_path, md_orig);
    #[cfg(unix)]
    let (trig_path, trig_orig) = {
        symlink(&outside_org, ws.path().join("escape.org")).expect("create escape symlink");
        let p = ws.path().join("escape_trig.org");
        let s = "* TODO Trigger escape\n:PROPERTIES:\n:CUSTOM_ID: trig-esc\n:TRIGGER:  file(\"escape.org\") todo!(\"DONE\")\n:END:\n";
        write_file_str(&p, s);
        (p, s)
    };

    let (ws_str, out_dir) = (ws.path().to_str().unwrap(), Some(outside.path()));
    let mut client = McpTestClient::spawn_in(&["mcp", "--workspace", ws_str], out_dir, &[]);
    let rel_args = json!({ "locator": "specs/hub.org::#probe-task", "state": "TODO" });
    let rel_up = client.call_ok(1, "org_update_state", rel_args);
    assert_eq!(rel_up["entries_affected"], 1);
    let sh_args = json!({ "locator": "specs/hub.org::#probe-task" });
    assert_eq!(client.call_ok(2, "org_show", sh_args)["keyword"], "TODO");

    let out_loc = format!("{}::1", outside_org.display());
    let bad_all = [out_loc.as_str(), "../outside.org::1"];
    for (idx, loc) in bad_all.into_iter().enumerate() {
        let args = json!({ "locator": loc, "state": "DONE" });
        client.assert_err(3 + idx as i64, "org_update_state", args);
    }
    client.assert_err(7, "org_show", json!({ "locator": &out_loc }));
    client.assert_err(10, "org_fmt_file", json!({ "path": "README.md" }));
    #[cfg(unix)]
    {
        let bad_sym = ["escape.org::1", "escape_trig.org::#trig-esc"];
        for (idx, loc) in bad_sym.into_iter().enumerate() {
            let args = json!({ "locator": loc, "state": "DONE" });
            client.assert_err(5 + idx as i64, "org_update_state", args);
        }
        client.assert_err(8, "org_show", json!({ "locator": "escape.org::1" }));
        client.assert_err(9, "org_fmt_file", json!({ "path": "escape.org" }));
    }

    client.close_cleanly();
    assert_eq!(fs::read_to_string(&outside_org).unwrap(), outside_orig);
    assert_eq!(fs::read_to_string(&md_path).unwrap(), md_orig);
    #[cfg(unix)]
    assert_eq!(fs::read_to_string(&trig_path).unwrap(), trig_orig);
}

// endregion

#[path = "mcp_tests/roam_workflow.rs"]
mod roam_workflow;
