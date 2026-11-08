// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Red-to-Green CLI and MCP integration tests for `org refile` and `org_refile` (`ORGFMT-WP-18`).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

// region Constants & Helpers

/// Expected exit code when an operational or cycle error occurs (`EXIT_ERROR`).
const EXIT_ERROR_CODE: i32 = 2;
/// Default target display column for `F007` heading tag alignment.
const TAG_COLUMN: usize = 77;
/// UUID for the source subtree in `inbox.org`.
const SRC_TASK_ID: &str = "80000000-0000-4000-8000-000000000801";
/// UUID for the child heading inside the source subtree in `inbox.org`.
const SRC_CHILD_ID: &str = "80000000-0000-4000-8000-000000000802";
/// UUID for the target project heading in `projects.org`.
const DEST_PROJECT_ID: &str = "80000000-0000-4000-8000-000000000803";
/// UUID for a level-0 `org-roam` file node.
const FILE_NODE_ID: &str = "80000000-0000-4000-8000-000000000804";

/// Writes UTF-8 text to `path`, creating parent directories as needed.
fn write_fixture(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directories");
    }
    fs::write(path, content).expect("write fixture file");
}

/// Runs `org` with `args` inside `cwd` and returns the process [`Output`].
fn run_org(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_org"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("execute org binary")
}

/// Asserts that `org fmt check` exits with status `0` on `targets` inside `cwd`.
fn assert_fmt_clean(cwd: &Path, targets: &[&str]) {
    let mut cmd_args = vec!["fmt", "check"];
    cmd_args.extend_from_slice(targets);
    let out = run_org(cwd, &cmd_args);
    assert!(
        out.status.success(),
        "expected org fmt check clean, got:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Interactive `stdio` client wrapping a spawned `org mcp` child process.
struct McpRefileClient {
    /// Spawned `org mcp` child process.
    child: Child,
    /// Writable handle to child `stdin`.
    stdin: Option<ChildStdin>,
    /// Buffered reader over child `stdout`.
    stdout: BufReader<ChildStdout>,
    /// Auto-incrementing JSON-RPC request ID counter.
    next_id: i64,
}

impl McpRefileClient {
    /// Spawns `org mcp --workspace <ws>` over piped `stdio`.
    fn spawn(ws: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_org"))
            .args(["mcp", "--workspace", ws.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn org mcp");
        let stdin = Some(child.stdin.take().expect("piped stdin"));
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Sends a newline-delimited JSON-RPC request and parses the JSON-RPC response.
    fn send_rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).expect("write request");
        stdin.flush().expect("flush stdin");
        let mut line = String::new();
        assert!(self.stdout.read_line(&mut line).expect("read reply") > 0);
        serde_json::from_str(&line).expect("parse JSON-RPC reply")
    }

    /// Invokes `tools/call` for `name` with `arguments` and returns `(is_error, structuredContent)`.
    fn call_tool(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let resp = self.send_rpc(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let res = &resp["result"];
        let is_err = res["isError"].as_bool().expect("CallToolResult.isError");
        let structured = res["structuredContent"].clone();
        (is_err, structured)
    }

    /// Closes `stdin` and asserts clean process exit (`0`).
    fn close_cleanly(mut self) {
        drop(self.stdin.take());
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).expect("drain stdout");
        assert!(rest.is_empty(), "unexpected extra stdout: {rest:?}");
        assert!(self.child.wait().expect("wait child").success());
    }
}

// endregion

// region Slice 1: Cross-File Subtree Refile & Level/Tag Adjustment

/// Verifies cross-file subtree refiling adjusts heading stars, aligns `F007` tags, and skips literal blocks.
#[test]
fn slice1_cross_file_refile_adjusts_subtree_levels_tags_and_preserves_literal_blocks() {
    let dir = TempDir::new().unwrap();
    let (inbox, projects) = (
        dir.path().join("inbox.org"),
        dir.path().join("projects.org"),
    );
    let inbox_raw = format!(
        "* TODO Ship parser                                                     :@code:\n:PROPERTIES:\n:ID: {SRC_TASK_ID}\n:END:\nParser implementation notes.\n#+BEGIN_SRC org\n* literal heading inside src block :unaligned:\n#+END_SRC\n\n** NEXT Write unit tests :test:\n:PROPERTIES:\n:ID: {SRC_CHILD_ID}\n:END:\nChild body line.\n\n* TODO Triage backlog\nRemaining inbox item.\n"
    );
    let projects_raw = format!(
        "* Active Projects\n:PROPERTIES:\n:ID: {DEST_PROJECT_ID}\n:END:\nProject overview.\n\n** DONE Initial setup\n\n* Someday\nFuture ideas.\n"
    );
    write_fixture(&inbox, &inbox_raw);
    write_fixture(&projects, &projects_raw);

    let out = run_org(
        dir.path(),
        &[
            "refile",
            &format!("id:{SRC_TASK_ID}"),
            "--to",
            &format!("id:{DEST_PROJECT_ID}"),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&inbox).unwrap(),
        "* TODO Triage backlog\nRemaining inbox item.\n"
    );

    let proj_after = fs::read_to_string(&projects).unwrap();
    let moved_root = proj_after
        .lines()
        .find(|l| l.contains("Ship parser"))
        .unwrap();
    assert!(moved_root.starts_with("** TODO Ship parser ") && moved_root.ends_with(" :@code:"));
    assert_eq!(moved_root.chars().count(), TAG_COLUMN);

    let moved_child = proj_after
        .lines()
        .find(|l| l.contains("Write unit tests"))
        .unwrap();
    assert!(
        moved_child.starts_with("*** NEXT Write unit tests ") && moved_child.ends_with(" :test:")
    );
    assert_eq!(moved_child.chars().count(), TAG_COLUMN);
    assert!(proj_after.contains("\n* literal heading inside src block :unaligned:\n"));
    assert!(
        proj_after.find("** DONE Initial setup").unwrap()
            < proj_after.find("** TODO Ship parser").unwrap()
    );
    assert!(
        proj_after.find("** TODO Ship parser").unwrap() < proj_after.find("* Someday").unwrap()
    );

    write_fixture(
        &projects,
        &proj_after.replace(
            "* literal heading inside src block :unaligned:",
            ",* literal heading",
        ),
    );
    assert_fmt_clean(dir.path(), &["inbox.org", "projects.org"]);
}

// endregion

// region Slice 2: Same-File Refile (Earlier-After-Later & Later-Under-Earlier)

/// Verifies same-file refiling in both directions preserves line offsets and `org fmt check` cleanliness.
#[test]
fn slice2_same_file_refile_earlier_after_later_and_later_under_earlier() {
    let dir = TempDir::new().unwrap();
    let tasks = dir.path().join("tasks.org");
    let initial = concat!(
        "* Alpha\nAlpha body.\n\n** Alpha Child\nAlpha child body.\n\n",
        "* Beta\nBeta body.\n\n* Omega\nOmega body.\n"
    );
    write_fixture(&tasks, initial);

    let out1 = run_org(
        dir.path(),
        &["refile", "tasks.org::*Alpha", "--to", "tasks.org::*Omega"],
    );
    assert!(
        out1.status.success(),
        "{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let after1 = fs::read_to_string(&tasks).unwrap();
    assert!(after1.starts_with("* Beta\nBeta body.\n\n* Omega\nOmega body.\n\n** Alpha\n"));
    assert!(after1.contains("*** Alpha Child\nAlpha child body.\n"));
    assert_fmt_clean(dir.path(), &["tasks.org"]);

    let out2 = run_org(
        dir.path(),
        &[
            "refile",
            "tasks.org::*/Omega/Alpha",
            "--to",
            "tasks.org::*Beta",
        ],
    );
    assert!(
        out2.status.success(),
        "{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let after2 = fs::read_to_string(&tasks).unwrap();
    let pos_beta = after2.find("* Beta").unwrap();
    let pos_alpha = after2.find("** Alpha").unwrap();
    let pos_omega = after2.find("* Omega").unwrap();
    assert!(pos_beta < pos_alpha && pos_alpha < pos_omega);
    assert!(after2.contains("*** Alpha Child\nAlpha child body.\n"));
    assert_fmt_clean(dir.path(), &["tasks.org"]);
}

// endregion

// region Slice 3: --as-root & File-Target Refiling

/// Verifies `--as-root` and plain file targets promote deeply nested subtrees (`***` -> `*`, `****` -> `**`).
#[test]
fn slice3_as_root_and_file_target_promote_nested_subtree_to_top_level() {
    let dir = TempDir::new().unwrap();
    let work = dir.path().join("work.org");
    let archive = dir.path().join("archive.org");

    write_fixture(
        &work,
        concat!(
            "* Area\n** Project\n*** TODO Deep task :deep:\n**** DONE Deep child :sub:\n",
            "Deep child body.\n\n*** TODO Second deep task\n**** TODO Second deep child\n"
        ),
    );
    write_fixture(&archive, "* Existing Root\nExisting body.\n");

    let out1 = run_org(dir.path(), &["refile", "*Deep task", "--to", "archive.org"]);
    assert!(out1.status.success());
    let arch1 = fs::read_to_string(&archive).unwrap();
    let root_line = arch1.lines().find(|l| l.contains("Deep task")).unwrap();
    assert!(root_line.starts_with("* TODO Deep task ") && root_line.chars().count() == TAG_COLUMN);
    let child_line = arch1.lines().find(|l| l.contains("Deep child")).unwrap();
    assert!(
        child_line.starts_with("** DONE Deep child ") && child_line.chars().count() == TAG_COLUMN
    );

    let out2 = run_org(
        dir.path(),
        &[
            "refile",
            "*Second deep task",
            "--to",
            "archive.org::*Existing Root",
            "--as-root",
        ],
    );
    assert!(out2.status.success());
    let arch2 = fs::read_to_string(&archive).unwrap();
    assert!(arch2.contains("\n* TODO Second deep task\n** TODO Second deep child\n"));
    assert_fmt_clean(dir.path(), &["work.org", "archive.org"]);
}

// endregion

// region Slice 4: Self & Descendant Cycle Rejection

/// Verifies refiling a subtree into itself or any of its descendants exits with `EXIT_ERROR` (`2`).
#[test]
fn slice4_self_and_descendant_cycle_rejected_with_exit_error() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("cycle.org");
    let raw = format!(
        concat!(
            ":PROPERTIES:\n:ID: {file_id}\n:END:\n#+TITLE: Cycle Test\n\n",
            "* Parent\n:PROPERTIES:\n:ID: {parent_id}\n:END:\n",
            "** Child\n*** Grandchild\n:PROPERTIES:\n:ID: {grand_id}\n:END:\n"
        ),
        file_id = FILE_NODE_ID,
        parent_id = SRC_TASK_ID,
        grand_id = SRC_CHILD_ID,
    );
    write_fixture(&file, &raw);

    let parent_loc = format!("id:{SRC_TASK_ID}");
    let grand_loc = format!("id:{SRC_CHILD_ID}");
    let file_loc = format!("id:{FILE_NODE_ID}");

    for (src, dst) in [
        (parent_loc.as_str(), parent_loc.as_str()),
        (parent_loc.as_str(), grand_loc.as_str()),
        (file_loc.as_str(), parent_loc.as_str()),
    ] {
        let out = run_org(dir.path(), &["refile", src, "--to", dst]);
        assert_eq!(out.status.code(), Some(EXIT_ERROR_CODE));
        assert!(String::from_utf8_lossy(&out.stderr).contains("org:"));
        assert_eq!(fs::read_to_string(&file).unwrap(), raw);
    }
}

// endregion

// region Slice 5: --dry-run, --format json, Cache Invalidation & org_refile MCP

/// Verifies `--dry-run`, `--format json` `MutationResult`, warm cache invalidation, and `org_refile` MCP.
#[test]
fn slice5_dry_run_json_envelope_cache_invalidation_and_mcp_org_refile() {
    let dir = TempDir::new().unwrap();
    let (src_file, dst_file) = (dir.path().join("inbox.org"), dir.path().join("roadmap.org"));
    let src_raw = format!("* TODO Migrate subtree\n:PROPERTIES:\n:ID: {SRC_TASK_ID}\n:END:\n** TODO Sub-item\n\n* TODO Keep me\n");
    let dst_raw = format!("* Target Epic\n:PROPERTIES:\n:ID: {DEST_PROJECT_ID}\n:END:\n");
    write_fixture(&src_file, &src_raw);
    write_fixture(&dst_file, &dst_raw);
    assert!(run_org(dir.path(), &["cache", "rebuild"]).status.success());

    let (src_loc, dst_loc) = (format!("id:{SRC_TASK_ID}"), format!("id:{DEST_PROJECT_ID}"));
    let dry = run_org(
        dir.path(),
        &[
            "refile",
            &src_loc,
            "--to",
            &dst_loc,
            "--dry-run",
            "--format",
            "json",
        ],
    );
    assert!(dry.status.success());
    let dry_json: Value = serde_json::from_slice(&dry.stdout).expect("valid JSON");
    assert_eq!(
        (dry_json["command"].as_str(), dry_json["dry_run"].as_bool()),
        (Some("refile"), Some(true))
    );
    assert_eq!(
        (
            dry_json["files_modified"].as_u64(),
            dry_json["entries_affected"].as_u64()
        ),
        (Some(2), Some(1))
    );
    assert!(dry_json["details"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("level 1 -> 2"));
    assert_eq!(fs::read_to_string(&src_file).unwrap(), src_raw);

    let mut client = McpRefileClient::spawn(dir.path());
    let list = client.send_rpc("tools/list", json!({}));
    assert!(list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "org_refile"));
    assert!(
        client
            .call_tool(
                "org_refile",
                json!({ "source_locator": &src_loc, "target": &src_loc })
            )
            .0
    );
    assert!(
        client
            .call_tool(
                "org_refile",
                json!({ "source_locator": &src_loc, "target": "../outside.org" })
            )
            .0
    );

    let (is_err, res) = client.call_tool(
        "org_refile",
        json!({ "source_locator": &src_loc, "target": &dst_loc }),
    );
    assert!(!is_err && res["command"] == "refile" && res["files_modified"] == 2);
    let (show_err, shown) = client.call_tool("org_show", json!({ "locator": &src_loc }));
    assert!(
        !show_err
            && shown["level"] == 2
            && shown["file"].as_str().unwrap().ends_with("roadmap.org")
    );
    client.close_cleanly();
}

// endregion
