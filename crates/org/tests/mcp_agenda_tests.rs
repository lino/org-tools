// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! End-to-end Red-to-Green `stdio` MCP and documentation tests for `ORGFMT-WP-15`.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Map, Value};
use tempfile::TempDir;

// region Constants & Test Harness

/// UUID for the level-0 `org-roam` file node in `specs/hub.org`.
const FILE_NODE_ID: &str = "20000000-0000-4000-8000-000000000201";
/// UUID for the primary actionable task in `plan/tasks.org`.
const TASK_ALPHA_ID: &str = "20000000-0000-4000-8000-000000000202";
/// UUID for the downstream blocked task in `plan/roadmap.org`.
const TASK_BLOCKED_ID: &str = "20000000-0000-4000-8000-000000000203";

/// Returns the repository workspace root derived from `CARGO_MANIFEST_DIR`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

/// Writes UTF-8 fixture text to `path`, creating parent directories as needed.
fn write_fixture(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent dirs");
    }
    fs::write(path, content).expect("write fixture file");
}

/// Seeds a multi-file workspace for agenda projection and planning mutation tests.
fn seed_agenda_workspace(root: &Path) {
    let put = |rel: &str, s: &str| write_fixture(&root.join(rel), s);
    put(".org-tools.toml", "[agenda]\ndeadline_warning_days = 5\n");
    let h = format!(":PROPERTIES:\n:ID: {FILE_NODE_ID}\n:END:\n#+TITLE: Hub Spec\n\n* NEXT Review design <2026-04-08 Wed 14:00>                            :@arch:\n:PROPERTIES:\n:CUSTOM_ID: design-review\n:END:\n");
    let t = format!("#+TITLE: Tasks\n\n* TODO Ship sprint milestone                                           :@code:\nSCHEDULED: <2026-04-06 Mon 09:00 +1w>\n:PROPERTIES:\n:ID: {TASK_ALPHA_ID}\n:END:\n\n* TODO Prepare release notes                                           :@docs:\nDEADLINE: <2026-04-10 Fri>\n:PROPERTIES:\n:CUSTOM_ID: rel-notes\n:END:\n\n* TODO Triage raw capture\n");
    let r = format!("#+TITLE: Roadmap\n\n* TODO Deploy production cutover                                       :@ops:\nSCHEDULED: <2026-04-04 Sat>\n:PROPERTIES:\n:ID:      {TASK_BLOCKED_ID}\n:BLOCKER: ids(\"{TASK_ALPHA_ID}\")\n:END:\n");
    put("specs/hub.org", &h);
    put("plan/tasks.org", &t);
    put("plan/roadmap.org", &r);
}

/// Asserts that `tel` carries `duration_us` and exact `(scanned, hits, misses)` counters.
fn assert_telemetry(tel: &Value, expected: (u64, u64, u64)) {
    assert!(tel["duration_us"].is_u64(), "missing duration_us: {tel}");
    assert_eq!(tel["files_scanned"].as_u64(), Some(expected.0));
    assert_eq!(tel["cache_hits"].as_u64(), Some(expected.1));
    assert_eq!(tel["cache_misses"].as_u64(), Some(expected.2));
}

/// Asserts that `text` contains every `|`-separated token in `pipe_tokens`.
fn assert_contains_pipe_tokens(label: &str, text: &str, pipe_tokens: &str) {
    for tok in pipe_tokens.split('|') {
        assert!(text.contains(tok), "{label} missing '{tok}'");
    }
}

/// Asserts that `org fmt check` exits cleanly (`0`) for `targets` inside `cwd`.
fn assert_org_fmt_clean(cwd: &Path, targets: &[&str]) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
    let out = cmd
        .current_dir(cwd)
        .arg("fmt")
        .arg("check")
        .args(targets)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "fmt check: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Interactive `stdio` client wrapping a spawned `org mcp` child process.
struct McpAgendaClient {
    /// Spawned `org mcp` child process.
    child: Child,
    /// Writable handle to child `stdin`.
    stdin: Option<ChildStdin>,
    /// Buffered reader over child `stdout`.
    stdout: BufReader<ChildStdout>,
    /// Auto-incrementing JSON-RPC request ID counter.
    next_id: i64,
}

impl McpAgendaClient {
    /// Spawns `org mcp --workspace <ws>` with optional `cwd` over piped `stdio`.
    fn spawn(ws: &Path, cwd: Option<&Path>) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_org"));
        cmd.args(["mcp", "--workspace", ws.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().expect("spawn org mcp");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Invokes `tools/call` for `name` with `arguments` and returns `(is_error, structuredContent)`.
    fn call_tool(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let id = self.next_id;
        self.next_id += 1;
        let params = json!({ "name": name, "arguments": arguments });
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params });
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).expect("write frame");
        stdin.flush().expect("flush stdin");
        let mut line = String::new();
        assert!(self.stdout.read_line(&mut line).expect("read reply") > 0);
        let resp: Value = serde_json::from_str(&line).expect("valid JSON reply");
        let res = &resp["result"];
        let is_err = res["isError"].as_bool().expect("CallToolResult.isError");
        let text = res["content"][0]["text"].as_str().expect("content text");
        let structured = res["structuredContent"].clone();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), structured);
        (is_err, structured)
    }

    /// Invokes `tools/call`, asserts `isError == false`, and returns `structuredContent`.
    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let (is_err, payload) = self.call_tool(name, arguments);
        assert!(!is_err, "unexpected tool error on {name}: {payload}");
        payload
    }

    /// Invokes `tools/call` and asserts `isError == true` with a non-empty `error` string.
    fn err(&mut self, name: &str, arguments: Value) {
        let (is_err, p) = self.call_tool(name, arguments);
        let ok = is_err && p["error"].as_str().is_some_and(|s| !s.is_empty());
        assert!(ok, "expected domain error on {name}: {p}");
        assert!(p["telemetry"]["duration_us"].is_u64(), "missing tel: {p}");
    }

    /// Closes `stdin` and asserts clean process exit (`0`) with zero trailing `stdout`.
    fn close_cleanly(mut self) {
        drop(self.stdin.take());
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).expect("drain stdout");
        assert!(rest.is_empty(), "extra stdout: {rest:?}");
        assert!(self.child.wait().expect("wait child").success());
    }
}

// endregion

// region JSON Schema Contract Validator

/// Checks whether `instance` matches a JSON Schema primitive type keyword.
fn matches_json_type(instance: &Value, ty: &str) -> bool {
    match ty {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => false,
    }
}

/// Validates scalar `type`, `enum`, `const`, and `minimum` constraints on `val`.
fn check_scalar_schema(val: &Value, schema: &Value) {
    if let Some(ty) = schema.get("type") {
        let ok = match ty {
            Value::String(s) => matches_json_type(val, s),
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .any(|s| matches_json_type(val, s)),
            _ => false,
        };
        assert!(ok, "type mismatch: expected {ty}, got {val}");
    }
    if let Some(c) = schema.get("const") {
        assert_eq!(val, c, "const mismatch");
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        assert!(allowed.contains(val), "{val} not in enum {allowed:?}");
    }
    if let (Some(min), Some(n)) = (schema.get("minimum").and_then(Value::as_i64), val.as_i64()) {
        assert!(n >= min, "{n} < minimum {min}");
    }
}

/// Validates object `required`, `properties`, and `additionalProperties` recursively.
fn check_object_schema(obj: &Map<String, Value>, schema: &Value, root: &Value) {
    if let Some(reqs) = schema.get("required").and_then(Value::as_array) {
        for req in reqs.iter().filter_map(Value::as_str) {
            assert!(obj.contains_key(req), "missing required key '{req}'");
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    for (k, v) in obj {
        if let Some(p_schema) = props.and_then(|m| m.get(k)) {
            assert_conforms_to_schema(v, p_schema, root);
        } else if let Some(add) = schema.get("additionalProperties") {
            assert_ne!(add, &Value::Bool(false), "unexpected property '{k}'");
            if add.is_object() {
                assert_conforms_to_schema(v, add, root);
            }
        } else {
            panic!("unhandled key '{k}'");
        }
    }
}

/// Recursively validates `val` against `schema` (resolving `#/$defs/...` against `root`).
fn assert_conforms_to_schema(val: &Value, schema: &Value, root: &Value) {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let def_key = r.strip_prefix("#/$defs/").expect("local $defs ref");
        let target = &root["$defs"][def_key];
        assert!(target.is_object(), "missing $defs/{def_key}");
        return assert_conforms_to_schema(val, target, root);
    }
    check_scalar_schema(val, schema);
    if let (Some(arr), Some(items)) = (val.as_array(), schema.get("items")) {
        for item in arr {
            assert_conforms_to_schema(item, items, root);
        }
    }
    if let Some(obj) = val.as_object() {
        check_object_schema(obj, schema, root);
    }
}

/// Loads `schemas/<filename>` from the repository and validates `instance` against it.
fn assert_repo_schema(instance: &Value, filename: &str) {
    let raw = fs::read_to_string(repo_root().join("schemas").join(filename)).expect("read schema");
    let schema: Value = serde_json::from_str(&raw).expect("parse schema");
    assert_conforms_to_schema(instance, &schema, &schema);
}

// endregion

// region Slice 2: org_query_agenda & org_update_planning Behavioural Tests

/// Verifies `org_query_agenda` JSON schema compliance, warm telemetry, `path`, and text formats.
#[test]
fn slice2_query_agenda_projects_timeline_composite_formats_and_warm_telemetry() {
    let (ws, scratch) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    seed_agenda_workspace(ws.path());
    let mut c = McpAgendaClient::spawn(ws.path(), None);

    let q1 = json!({ "from": "2026-04-06", "days": 9, "composite": true });
    let a1 = c.ok("org_query_agenda", q1);
    assert_repo_schema(&a1, "org-agenda-output.schema.json");
    assert_eq!(a1["from"], "2026-04-06");
    assert_eq!((&a1["days"], &a1["warning_days"]), (&json!(9), &json!(5)));
    assert_telemetry(&a1["telemetry"], (3, 0, 3));
    assert!(a1["days_timeline"].as_array().unwrap().len() >= 4);
    assert_eq!(a1["composite"]["blocked"].as_array().unwrap().len(), 1);
    assert_eq!(a1["composite"]["inbox"].as_array().unwrap().len(), 1);

    let q_sub = json!({ "path": "specs", "from": "2026-04-08", "days": 1, "warning_days": 2 });
    let sub = c.ok("org_query_agenda", q_sub);
    assert_repo_schema(&sub, "org-agenda-output.schema.json");
    assert_eq!(sub["warning_days"], 2);
    assert_eq!(sub["days_timeline"].as_array().unwrap().len(), 1);
    assert_telemetry(&sub["telemetry"], (3, 3, 0));

    for fmt in ["org", "md", "human", "locator"] {
        let q = json!({ "from": "2026-04-06", "days": 7, "composite": true, "format": fmt });
        let env = c.ok("org_query_agenda", q);
        assert_eq!(env["format"], fmt);
        assert_eq!(env["has_items"], true);
        assert!(!env["report"].as_str().unwrap_or("").is_empty());
        assert_telemetry(&env["telemetry"], (3, 3, 0));
        if fmt == "org" {
            let p = scratch.path().join("rendered.org");
            write_fixture(&p, env["report"].as_str().unwrap());
            assert_org_fmt_clean(scratch.path(), &["rendered.org"]);
        }
    }
    c.close_cleanly();
}

/// Verifies empty workspaces succeed (`isError: false`) while invalid args and path escapes fail.
#[test]
fn slice2_query_agenda_empty_workspace_and_confinement_validation() {
    let (empty_ws, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let cfg = empty_ws.path().join(".org-tools.toml");
    write_fixture(&cfg, "[agenda]\ndeadline_warning_days = 3\n");
    let mut ec = McpAgendaClient::spawn(empty_ws.path(), Some(outside.path()));
    let er = ec.ok("org_query_agenda", json!({ "from": "2026-04-06" }));
    assert_repo_schema(&er, "org-agenda-output.schema.json");
    assert_eq!(er["warning_days"], 3);
    assert!(er["days_timeline"].as_array().unwrap().is_empty());
    assert_telemetry(&er["telemetry"], (0, 0, 0));
    ec.close_cleanly();

    let ws = TempDir::new().unwrap();
    seed_agenda_workspace(ws.path());
    let outside_org = outside.path().join("secret.org");
    write_fixture(&outside_org, "* TODO Secret\nSCHEDULED: <2026-04-06 Mon>\n");
    write_fixture(&ws.path().join("notes.md"), "# Markdown\n");
    #[cfg(unix)]
    {
        symlink(&outside_org, ws.path().join("escape.org")).expect("file symlink");
        symlink(outside.path(), ws.path().join("escape_dir")).expect("dir symlink");
    }

    let mut c = McpAgendaClient::spawn(ws.path(), Some(outside.path()));
    let pre = c.ok(
        "org_query_agenda",
        json!({ "from": "2026-03-01", "days": 2 }),
    );
    assert!(pre["days_timeline"].as_array().unwrap().is_empty());
    let bad_agenda = concat!(
        r#"{"unknown_key":true}|{"days":0}|{"days":-3}|{"days":"7"}|{"warning_days":-1}|"#,
        r#"{"from":"2026-02-30"}|{"format":"xml"}|{"path":"../secret.org"}|"#,
        r#"{"path":"notes.md"}|{"path":"missing.org"}"#
    );
    for raw in bad_agenda.split('|') {
        c.err("org_query_agenda", serde_json::from_str(raw).unwrap());
    }
    let out_q = json!({ "path": outside_org.to_str().unwrap() });
    c.err("org_query_agenda", out_q);
    #[cfg(unix)]
    for p in ["escape.org", "escape_dir"] {
        c.err("org_query_agenda", json!({ "path": p }));
    }
    c.close_cleanly();
}

/// Verifies `org_update_planning` `dry_run`, atomic writes, batch `targets`, `clear`, and cache refresh.
#[test]
fn slice2_update_planning_mutates_clears_and_refreshes_warm_session() {
    let ws = TempDir::new().unwrap();
    seed_agenda_workspace(ws.path());
    let tasks_path = ws.path().join("plan/tasks.org");
    let frozen_mtime = fs::metadata(&tasks_path).unwrap().modified().unwrap();
    let before_disk = fs::read_to_string(&tasks_path).unwrap();
    let mut c = McpAgendaClient::spawn(ws.path(), None);

    let q_win = json!({ "from": "2026-03-20", "days": 2 });
    let init_ag = c.ok("org_query_agenda", q_win.clone());
    assert!(init_ag["days_timeline"].as_array().unwrap().is_empty());
    assert_telemetry(&init_ag["telemetry"], (3, 0, 3));

    let loc = format!("id:{TASK_ALPHA_ID}");
    let dry_q = json!({ "kind": "scheduled", "locator": &loc, "value": "2026-03-20 10:30", "dry_run": true });
    let dry = c.ok("org_update_planning", dry_q);
    assert_repo_schema(&dry, "org-mutation-output.schema.json");
    assert_eq!(dry["command"], "set-scheduled");
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read_to_string(&tasks_path).unwrap(), before_disk);

    let set_q = json!({ "kind": "scheduled", "locator": &loc, "value": "2026-03-20 10:30" });
    let set_res = c.ok("org_update_planning", set_q);
    assert_repo_schema(&set_res, "org-mutation-output.schema.json");
    assert_telemetry(&set_res["telemetry"], (3, 2, 1));
    let f = OpenOptions::new().write(true).open(&tasks_path).unwrap();
    f.set_modified(frozen_mtime).unwrap();

    let after_ag = c.ok("org_query_agenda", q_win);
    assert_eq!(after_ag["days_timeline"].as_array().unwrap().len(), 1);
    let shown = c.ok("org_show", json!({ "locator": &loc }));
    assert_eq!(shown["planning"]["scheduled"], "2026-03-20 10:30");

    let batch_q = json!({ "kind": "deadline", "targets": [&loc, "plan/tasks.org::#rel-notes"], "value": "2026-05-02 -3d" });
    let batch = c.ok("org_update_planning", batch_q);
    assert_eq!(batch["command"], "set-deadline");
    assert_eq!(batch["entries_affected"], 2);

    let clr_q = json!({ "kind": "scheduled", "locator": &loc, "clear": true });
    let clr = c.ok("org_update_planning", clr_q);
    assert_eq!(clr["details"][0]["action"], "cleared-scheduled");
    let s2 = c.ok("org_show", json!({ "locator": &loc }));
    assert!(s2["planning"]["scheduled"].is_null() && s2["planning"]["deadline"] == "2026-05-02");
    c.close_cleanly();
}

/// Verifies `org_update_planning` rejects file nodes, bad dates, conflicts, and path/symlink escapes.
#[test]
fn slice2_update_planning_rejects_file_nodes_conflicts_and_escapes() {
    let (ws, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    seed_agenda_workspace(ws.path());
    let outside_org = outside.path().join("outside.org");
    let outside_orig = "* TODO Secret outside\n";
    write_fixture(&outside_org, outside_orig);
    #[cfg(unix)]
    symlink(&outside_org, ws.path().join("escape.org")).expect("escape symlink");

    let mut c = McpAgendaClient::spawn(ws.path(), Some(outside.path()));
    let loc = format!("id:{TASK_ALPHA_ID}");
    let bad_loc_q = |l: &str| json!({ "kind": "scheduled", "locator": l, "value": "2026-05-01" });
    let locs = format!(
        "id:{FILE_NODE_ID}|../outside.org::1|{}::1",
        outside_org.display()
    );
    for bad_loc in locs.split('|') {
        c.err("org_update_planning", bad_loc_q(bad_loc));
    }
    #[cfg(unix)]
    c.err("org_update_planning", bad_loc_q("escape.org::1"));
    for bad in [
        json!({ "kind": "scheduled", "locator": &loc, "value": "2026-02-30" }),
        json!({ "kind": "scheduled", "locator": &loc, "value": "[2026-05-01 Fri]" }),
        json!({ "kind": "scheduled", "locator": &loc }),
        json!({ "kind": "scheduled", "locator": &loc, "clear": true, "value": "2026-05-01" }),
        json!({ "kind": "scheduled", "value": "2026-05-01" }),
        json!({ "kind": "scheduled", "locator": &loc, "targets": [&loc], "value": "2026-05-01" }),
        json!({ "kind": "closed", "locator": &loc, "value": "2026-05-01" }),
        json!({ "kind": "scheduled", "locator": &loc, "value": "2026-05-01", "bogus": 1 }),
    ] {
        c.err("org_update_planning", bad);
    }
    c.close_cleanly();
    assert_eq!(fs::read_to_string(&outside_org).unwrap(), outside_orig);
}

// endregion

// region Slice 3: Synchronized Documentation & Self-Dogfood Lint Contract

/// Verifies `README.org`, `usage.org`, and `architecture.org` cover agenda, planning, and 10 MCP tools.
#[test]
fn slice3_docs_cover_agenda_planning_mcp_tools_and_pass_org_fmt_check() {
    let root = repo_root();
    let rd = |p: &str| fs::read_to_string(root.join(p)).unwrap();
    let r = "set-scheduled|set-deadline|deadline_warning_days|org_query_agenda|org_update_planning";
    assert_contains_pipe_tokens("README.org", &rd("README.org"), r);
    assert_contains_pipe_tokens("usage.org", &rd("docs/user/usage.org"), "--from|--warning-days|--composite|--no-repeat|set-scheduled|set-deadline|deadline_warning_days|org_query_agenda|org_update_planning|=agenda=");
    assert_contains_pipe_tokens("architecture.org", &rd("docs/technical/architecture.org"), "planning.rs|query_agenda.rs|update_planning.rs|tools_planning.rs|org_query_agenda|org_update_planning");
    assert_org_fmt_clean(&root, &["README.org", "docs/"]);
}

// endregion
