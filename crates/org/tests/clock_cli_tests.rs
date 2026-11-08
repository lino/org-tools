// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Red-to-Green CLI & MCP integration tests for `org clock in|out|cancel`,
//! `:EFFORT:` variance in `org clock report`, and `org_clock_in` / `org_clock_out` (`ORGFMT-WP-17`).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

// region Test Helpers

/// Builds a `Command` targeting the compiled `org` binary.
fn org_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_org"))
}

/// Writes UTF-8 fixture text to `path`, creating parent directories when needed.
fn write_fixture(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directories");
    }
    fs::write(path, content).expect("write fixture");
}

/// Sends newline-delimited JSON-RPC 2.0 requests to `org mcp` and returns parsed responses.
fn run_mcp_session(workspace: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = org_cmd()
        .args(["mcp", "--workspace", workspace.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn org mcp");
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));

    let mut responses = Vec::with_capacity(requests.len());
    for req in requests {
        let line = format!("{}\n", serde_json::to_string(req).unwrap());
        stdin.write_all(line.as_bytes()).expect("write request");
        stdin.flush().expect("flush request");

        let mut resp_line = String::new();
        stdout.read_line(&mut resp_line).expect("read response");
        responses.push(serde_json::from_str(resp_line.trim()).expect("parse JSON response"));
    }

    drop(stdin);
    let status = child.wait().expect("wait org mcp");
    assert!(status.success(), "org mcp exited with {status}");
    responses
}

// endregion

// region Slice 1: `org clock in`, Duplicate Guard & `--auto-out`

/// Verifies `org clock in` creates `:LOGBOOK:` and rejects a duplicate clock-in with exit code 2.
#[test]
fn clock_in_creates_logbook_and_rejects_duplicate_running_clock() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tasks.org");
    write_fixture(
        &file,
        "* TODO Implement clock mutations\n:PROPERTIES:\n:CUSTOM_ID: task-clock\n:END:\n* TODO Second task\n:PROPERTIES:\n:CUSTOM_ID: task-two\n:END:\n",
    );

    let loc1 = format!("{}::#task-clock", file.display());
    let out1 = org_cmd()
        .args([
            "clock",
            "in",
            &loc1,
            "--at",
            "2026-11-08T09:00",
            "--format",
            "json",
        ])
        .output()
        .expect("run org clock in");
    assert_eq!(
        out1.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let res1: Value = serde_json::from_slice(&out1.stdout).expect("valid JSON");
    assert_eq!(res1["subcommand"], "clock in");
    assert_eq!(res1["entries_affected"], 1);

    let updated = fs::read_to_string(&file).unwrap();
    assert!(
        updated.contains(":LOGBOOK:\nCLOCK: [2026-11-08 Sun 09:00]\n:END:"),
        "expected :LOGBOOK: with open clock, got:\n{updated}"
    );

    // Attempting to clock in again on the same or another entry without --auto-out fails with exit 2.
    let loc2 = format!("{}::#task-two", file.display());
    let dup_out = org_cmd()
        .args(["clock", "in", &loc2, "--at", "2026-11-08T09:30"])
        .output()
        .expect("run duplicate clock in");
    assert_eq!(dup_out.status.code(), Some(2));
    assert_eq!(fs::read_to_string(&file).unwrap(), updated);
}

/// Verifies `org clock in --auto-out` closes the previous running clock and opens the new clock.
#[test]
fn clock_in_with_auto_out_closes_previous_clock_and_opens_new_one() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tasks.org");
    write_fixture(
        &file,
        "* TODO First task\n:PROPERTIES:\n:CUSTOM_ID: first\n:END:\n:LOGBOOK:\nCLOCK: [2026-11-08 Sun 09:00]\n:END:\n* TODO Second task\n:PROPERTIES:\n:CUSTOM_ID: second\n:END:\n",
    );

    let loc2 = format!("{}::#second", file.display());
    let out = org_cmd()
        .args([
            "clock",
            "in",
            &loc2,
            "--at",
            "2026-11-08 10:15",
            "--auto-out",
            "--format",
            "json",
        ])
        .output()
        .expect("run clock in --auto-out");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let updated = fs::read_to_string(&file).unwrap();
    assert!(
        updated.contains("CLOCK: [2026-11-08 Sun 09:00]--[2026-11-08 Sun 10:15] =>  1:15"),
        "expected closed clock on first task, got:\n{updated}"
    );
    assert!(
        updated.contains("* TODO Second task\n:PROPERTIES:\n:CUSTOM_ID: second\n:END:\n:LOGBOOK:\nCLOCK: [2026-11-08 Sun 10:15]\n:END:"),
        "expected open clock on second task, got:\n{updated}"
    );
}

// endregion

// region Slice 2: `org clock out` & `org clock cancel`

/// Verifies `org clock out --at ... --format json` computes `=>  H:MM` and returns `MutationResult`.
#[test]
fn clock_out_computes_duration_and_emits_json_envelope() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("work.org");
    write_fixture(
        &file,
        "* NEXT Deep work session\n:PROPERTIES:\n:CUSTOM_ID: deep-work\n:END:\n:LOGBOOK:\nCLOCK: [2026-11-08 Sun 09:00]\n:END:\n",
    );

    let out = org_cmd()
        .args([
            "clock",
            "out",
            dir.path().to_str().unwrap(),
            "--at",
            "[2026-11-08 Sun 10:45]",
            "--format",
            "json",
        ])
        .output()
        .expect("run org clock out");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let res: Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(res["subcommand"], "clock out");
    assert_eq!(res["details"][0]["duration_minutes"], 105);
    assert_eq!(res["details"][0]["duration"], "1:45");

    let updated = fs::read_to_string(&file).unwrap();
    assert!(
        updated.contains("CLOCK: [2026-11-08 Sun 09:00]--[2026-11-08 Sun 10:45] =>  1:45"),
        "expected closed clock line, got:\n{updated}"
    );
}

/// Verifies `org clock cancel` removes the active open `CLOCK:` line cleanly.
#[test]
fn clock_cancel_removes_open_clock_line() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("cancel.org");
    write_fixture(
        &file,
        "* TODO Abortable task\n:PROPERTIES:\n:CUSTOM_ID: abort-me\n:END:\n:LOGBOOK:\nCLOCK: [2026-11-08 Sun 11:00]\nCLOCK: [2026-11-07 Sat 09:00]--[2026-11-07 Sat 10:00] =>  1:00\n:END:\n",
    );

    let out = org_cmd()
        .args([
            "clock",
            "cancel",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run org clock cancel");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let res: Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(res["subcommand"], "clock cancel");
    assert_eq!(res["entries_affected"], 1);

    let updated = fs::read_to_string(&file).unwrap();
    assert!(!updated.contains("2026-11-08 Sun 11:00"));
    assert!(updated.contains("CLOCK: [2026-11-07 Sat 09:00]--[2026-11-07 Sat 10:00] =>  1:00"));
}

// endregion

// region Slice 3: `:EFFORT:` Comparison in `org clock report`

/// Verifies `org clock report` includes `effort_minutes` and `variance_minutes` in JSON and human output.
#[test]
fn clock_report_includes_effort_and_variance_in_json_and_human() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("effort.org");
    write_fixture(
        &file,
        "* DONE Over-budget task\n:PROPERTIES:\n:EFFORT: 1:00\n:END:\n:LOGBOOK:\nCLOCK: [2026-11-08 Sun 09:00]--[2026-11-08 Sun 10:30] =>  1:30\n:END:\n* DONE Under-budget task\n:PROPERTIES:\n:EFFORT: 2:00\n:END:\n:LOGBOOK:\nCLOCK: [2026-11-08 Sun 11:00]--[2026-11-08 Sun 12:15] =>  1:15\n:END:\n",
    );

    let json_out = org_cmd()
        .args([
            "clock",
            "report",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run org clock report --format json");
    assert_eq!(json_out.status.code(), Some(0));
    let rows: Value = serde_json::from_slice(&json_out.stdout).expect("valid JSON");
    let arr = rows.as_array().expect("JSON array");
    assert_eq!(arr.len(), 2);

    assert_eq!(arr[0]["label"], "Over-budget task");
    assert_eq!(arr[0]["minutes"], 90);
    assert_eq!(arr[0]["effort_minutes"], 60);
    assert_eq!(arr[0]["effort"], "1:00");
    assert_eq!(arr[0]["variance_minutes"], 30);

    assert_eq!(arr[1]["label"], "Under-budget task");
    assert_eq!(arr[1]["minutes"], 75);
    assert_eq!(arr[1]["effort_minutes"], 120);
    assert_eq!(arr[1]["effort"], "2:00");
    assert_eq!(arr[1]["variance_minutes"], -45);

    let human_out = org_cmd()
        .args(["clock", "report", file.to_str().unwrap()])
        .output()
        .expect("run org clock report human");
    assert_eq!(human_out.status.code(), Some(0));
    let human_text = String::from_utf8_lossy(&human_out.stdout);
    assert!(human_text.contains("Effort"));
    assert!(human_text.contains("Variance"));
    assert!(human_text.contains("+0:30"));
    assert!(human_text.contains("-0:45"));
}

// endregion

// region Slice 4: MCP `org_clock_in` & `org_clock_out`

/// Verifies `org_clock_in` and `org_clock_out` over `org mcp` `stdio`.
#[test]
fn mcp_clock_in_and_clock_out_round_trip() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("mcp_tasks.org");
    write_fixture(
        &file,
        "* NEXT MCP Clock Target\n:PROPERTIES:\n:CUSTOM_ID: mcp-clock-1\n:END:\n",
    );

    let responses = run_mcp_session(
        dir.path(),
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "org_clock_in",
                    "arguments": {
                        "locator": "mcp_tasks.org::#mcp-clock-1",
                        "at": "2026-11-08T13:00"
                    }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "org_clock_out",
                    "arguments": {
                        "at": "2026-11-08T14:20"
                    }
                }
            }),
        ],
    );

    let res1 = &responses[0]["result"];
    assert_eq!(res1["isError"], false, "res1: {res1}");
    assert_eq!(res1["structuredContent"]["subcommand"], "clock in");

    let res2 = &responses[1]["result"];
    assert_eq!(res2["isError"], false, "res2: {res2}");
    assert_eq!(res2["structuredContent"]["subcommand"], "clock out");
    assert_eq!(
        res2["structuredContent"]["details"][0]["duration_minutes"],
        80
    );
    assert_eq!(res2["structuredContent"]["details"][0]["duration"], "1:20");

    let final_text = fs::read_to_string(&file).unwrap();
    assert!(
        final_text.contains("CLOCK: [2026-11-08 Sun 13:00]--[2026-11-08 Sun 14:20] =>  1:20"),
        "expected closed clock in file, got:\n{final_text}"
    );
}

// endregion
