// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for Luhmann fleeting note queue and transformation workflows (`ORGFMT-WP-24`).

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

// region Test Fixture Helpers

/// Creates an `assert_cmd::Command` targeting the `org` binary.
fn org_cmd() -> Command {
    Command::cargo_bin("org").expect("org binary must compile")
}

/// Writes `content` to `path`, creating parent directories as needed.
fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, content).expect("write test fixture");
}

/// Creates an executable editor shell script in `dir` on Unix.
#[cfg(unix)]
fn write_editor_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join(name);
    write_file(&script, &format!("#!/bin/sh\n{body}\n"));
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}

/// Sets up a standard workspace with a vault directory and `.org-tools.toml`.
fn setup_workspace(temp: &TempDir) -> (PathBuf, PathBuf) {
    let ws = temp.path().join("ws");
    let vault = ws.join("notes");
    fs::create_dir_all(&vault).unwrap();
    let cfg = r#"
[roam]
default_directory = "notes"
gtd_subdir = "gtd"
zettel_subdir = "zettel"
fleeting_subdir = "fleeting"
daily_subdir = "daily"

[roam.directories]
notes = "./notes"
"#;
    write_file(&ws.join(".org-tools.toml"), cfg);
    (ws, vault)
}

// endregion

// region Slice 1: Fleeting Capture and Listing Tests

/// Verifies `org fleeting capture` creates an unrefined fleeting note with complete metadata.
#[test]
fn slice1_capture_creates_fleeting_note_with_metadata() {
    let temp = TempDir::new().unwrap();
    let (ws, vault) = setup_workspace(&temp);

    // Create an existing medium node to verify link resolution
    let medium_file = vault.join("media/book.org");
    write_file(
        &medium_file,
        ":PROPERTIES:\n:ID:       11111111-2222-3333-4444-555555555555\n:END:\n#+title: Deep Work\n",
    );

    let output = org_cmd()
        .current_dir(&ws)
        .args([
            "fleeting",
            "capture",
            "Initial Idea on Attention",
            "--body",
            "Attention residue limits cognitive focus.",
            "--medium",
            "Deep Work",
            "--location",
            "Chapter 2, p. 45",
            "--source-file",
            "crates/org/src/main.rs",
            "--no-edit",
            "--format",
            "json",
        ])
        .output()
        .expect("execute fleeting capture");

    assert!(
        output.status.success(),
        "fleeting capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let val: Value = serde_json::from_str(&stdout).expect("valid json output");
    assert_eq!(val["files_modified"], 1);
    assert_eq!(val["dry_run"], false);

    let fleeting_dir = vault.join("fleeting");
    assert!(fleeting_dir.is_dir());
    let files: Vec<_> = fs::read_dir(&fleeting_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);

    let content = fs::read_to_string(&files[0]).unwrap();
    assert!(content.contains(":TYPE:"));
    assert!(content.contains("fleeting"));
    assert!(content.contains(":STATUS:"));
    assert!(content.contains("unrefined"));
    assert!(content.contains(":CAPTURED_AT:"));
    assert!(content.contains(":ATTACHED_MEDIUM:"));
    assert!(content.contains("[[id:11111111-2222-3333-4444-555555555555][Deep Work]]"));
    assert!(content.contains(":MEDIUM_LOCATION:"));
    assert!(content.contains("Chapter 2, p. 45"));
    assert!(content.contains(":SOURCE_FILE:"));
    assert!(content.contains("crates/org/src/main.rs"));
    assert!(content.contains("#+title: Initial Idea on Attention"));
    assert!(content.contains("#+filetags: :fleeting:"));
    assert!(content.contains("* Fleeting Thoughts\n\nAttention residue limits cognitive focus."));
    assert!(content.contains("* Context & Source"));
}

/// Verifies interactive capture refinement via `$EDITOR` and aborts on empty buffer.
#[test]
#[cfg(unix)]
fn slice1_capture_editor_abort_on_empty_or_failure() {
    let temp = TempDir::new().unwrap();
    let (ws, _vault) = setup_workspace(&temp);

    // Editor script that fails (exit 1)
    let fail_editor = write_editor_script(temp.path(), "fail_editor.sh", "exit 1");
    let out_fail = org_cmd()
        .current_dir(&ws)
        .env("EDITOR", fail_editor.to_str().unwrap())
        .args(["fleeting", "capture", "Test Note", "--edit"])
        .output()
        .expect("run failing capture editor");
    assert_eq!(out_fail.status.code(), Some(1));

    // Editor script that empties file
    let empty_editor = write_editor_script(temp.path(), "empty_editor.sh", "> \"$1\"");
    let out_empty = org_cmd()
        .current_dir(&ws)
        .env("EDITOR", empty_editor.to_str().unwrap())
        .args(["fleeting", "capture", "Test Note", "--edit"])
        .output()
        .expect("run empty capture editor");
    assert_eq!(out_empty.status.code(), Some(1));
}

/// Verifies `org fleeting list` discovers file and heading fleeting notes and filters by status.
#[test]
fn slice1_list_discovers_file_and_heading_stubs_with_status_filtering() {
    let temp = TempDir::new().unwrap();
    let (ws, vault) = setup_workspace(&temp);

    // 1. File-level unrefined fleeting note
    let f1 = vault.join("fleeting/20261112100000-idea-one.org");
    write_file(
        &f1,
        ":PROPERTIES:\n:ID:       aaaaaaaa-1111-2222-3333-444444444444\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Idea One\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nFirst thought.\n",
    );

    // 2. Heading-level fleeting stub (active/unrefined)
    let scratch = vault.join("scratchpad.org");
    write_file(
        &scratch,
        "#+title: Scratchpad\n\n* Quick Note on Rust :fleeting:\n:PROPERTIES:\n:ID:       bbbbbbbb-1111-2222-3333-444444444444\n:END:\nRust traits are powerful.\n\n* Done Stub :zettel_stub:\n:PROPERTIES:\n:ID:       cccccccc-1111-2222-3333-444444444444\n:STATUS:   reformed\n:END:\nAlready refined.\n",
    );

    // List default (unrefined)
    let out = org_cmd()
        .current_dir(&ws)
        .args(["fleeting", "list", "--format", "json"])
        .output()
        .expect("run fleeting list");
    assert!(out.status.success());
    let val: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(val["count"], 2);

    // List reformed
    let out_reformed = org_cmd()
        .current_dir(&ws)
        .args([
            "fleeting", "list", "--status", "reformed", "--format", "locator",
        ])
        .output()
        .expect("run fleeting list reformed");
    assert!(out_reformed.status.success());
    let loc_text = String::from_utf8_lossy(&out_reformed.stdout);
    assert!(loc_text.contains("id:cccccccc-1111-2222-3333-444444444444"));

    // List all
    let out_all = org_cmd()
        .current_dir(&ws)
        .args(["fleeting", "list", "--status", "all", "--format", "json"])
        .output()
        .expect("run fleeting list all");
    assert!(out_all.status.success());
    let val_all: Value = serde_json::from_str(&String::from_utf8_lossy(&out_all.stdout)).unwrap();
    assert_eq!(val_all["count"], 3);

    // List nonexistent status returns EXIT_ISSUES (1)
    let out_none = org_cmd()
        .current_dir(&ws)
        .args(["fleeting", "list", "--status", "discarded"])
        .output()
        .expect("run fleeting list discarded");
    assert_eq!(out_none.status.code(), Some(1));
}

// endregion

// region Slice 2: Transformation to Permanent Zettel (to-zettel / reform)

/// Verifies `org fleeting to-zettel` reforms a fleeting note into a permanent Zettel.
#[test]
fn slice2_to_zettel_transforms_note_and_stamps_source() {
    let temp = TempDir::new().unwrap();
    let (ws, vault) = setup_workspace(&temp);

    let f1 = vault.join("fleeting/20261112100000-network-topology.org");
    write_file(
        &f1,
        ":PROPERTIES:\n:ID:               f1111111-0000-0000-0000-000000000001\n:TYPE:             fleeting\n:STATUS:           unrefined\n:ATTACHED_MEDIUM:  Book on Networking\n:MEDIUM_LOCATION:  p. 100\n:END:\n#+title: Network Topology\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\n\nMesh networks are resilient against single-node failures.\n\n* Context & Source\n",
    );

    let out = org_cmd()
        .current_dir(&ws)
        .args([
            "fleeting",
            "to-zettel",
            "id:f1111111-0000-0000-0000-000000000001",
            "--title",
            "Mesh Network Topology Resilience",
            "--no-edit",
            "--format",
            "json",
        ])
        .output()
        .expect("run to-zettel");

    assert!(
        out.status.success(),
        "to-zettel failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify source fleeting note was updated
    let updated_src = fs::read_to_string(&f1).unwrap();
    assert!(updated_src.contains(":STATUS:"));
    assert!(updated_src.contains("reformed"));
    assert!(updated_src.contains(":PERMANENT_NOTE:"));
    assert!(updated_src.contains(":REFORMED_AT:"));
    assert!(updated_src.contains("* Reformed Permanent Note"));

    // Verify created permanent Zettel
    let zettel_file = vault.join("zettel/mesh-network-topology-resilience.org");
    assert!(zettel_file.is_file());
    let zettel_content = fs::read_to_string(&zettel_file).unwrap();
    assert!(zettel_content.contains(":TYPE:"));
    assert!(zettel_content.contains("concept"));
    assert!(zettel_content.contains(":SOURCE_MEDIUM:   Book on Networking"));
    assert!(zettel_content.contains(":MEDIUM_LOCATION: p. 100"));
    assert!(zettel_content.contains("#+title: Mesh Network Topology Resilience"));
    assert!(zettel_content.contains("* Thesis"));
    assert!(zettel_content
        .contains("* Core Concept\n\nMesh networks are resilient against single-node failures."));
    assert!(zettel_content.contains("* Context & Significance"));
    assert!(zettel_content.contains("* Related Concepts & Links"));
    assert!(zettel_content.contains("* Open Questions & Folgezettel"));

    // Verify generated Zettel passes org fmt check cleanly
    let fmt_check = org_cmd()
        .current_dir(&ws)
        .args(["fmt", "check", zettel_file.to_str().unwrap()])
        .output()
        .expect("check zettel formatting");
    assert!(fmt_check.status.success());
}

// endregion

// region Slice 3: to-task, discard, and interactive process

/// Verifies `org fleeting to-task` appends a task to the inbox and updates the source fleeting note.
#[test]
fn slice3_to_task_creates_inbox_entry_and_syncs_tags() {
    let temp = TempDir::new().unwrap();
    let (ws, vault) = setup_workspace(&temp);

    let f1 = vault.join("fleeting/20261112100000-fleeting-fix-auth.org");
    write_file(
        &f1,
        ":PROPERTIES:\n:ID:       f2222222-0000-0000-0000-000000000002\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Fleeting: Refactor token refresh\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nEnsure expired tokens return 401 instead of 500.\n",
    );

    let out = org_cmd()
        .current_dir(&ws)
        .args([
            "fleeting",
            "to-task",
            "id:f2222222-0000-0000-0000-000000000002",
            "--context",
            "@code",
            "--effort",
            "0:30",
            "--format",
            "json",
        ])
        .output()
        .expect("run to-task");

    assert!(
        out.status.success(),
        "to-task failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Check inbox was created with clean title (prefix stripped)
    let inbox_file = vault.join("gtd/inbox.org");
    assert!(inbox_file.is_file());
    let inbox_content = fs::read_to_string(&inbox_file).unwrap();
    assert!(inbox_content.contains("* TODO Refactor token refresh"));
    assert!(inbox_content.contains(":@code:"));
    assert!(inbox_content.contains(":EFFORT:"));
    assert!(inbox_content.contains("0:30"));
    assert!(inbox_content.contains(":SOURCE_FLEETING: [[id:f2222222-0000-0000-0000-000000000002][Fleeting: Refactor token refresh]]"));
    assert!(inbox_content.contains("Ensure expired tokens return 401 instead of 500."));

    // Check source fleeting note was stamped
    let updated_src = fs::read_to_string(&f1).unwrap();
    assert!(updated_src.contains(":STATUS:"));
    assert!(updated_src.contains("converted-to-task"));
    assert!(updated_src.contains(":TASK_NOTE:"));
    assert!(updated_src.contains(":CONVERTED_AT:"));
}

/// Verifies `org fleeting discard` marks a note discarded or deletes it when `--delete` is passed.
#[test]
fn slice3_discard_marks_or_deletes_fleeting_note() {
    let temp = TempDir::new().unwrap();
    let (ws, vault) = setup_workspace(&temp);

    let f1 = vault.join("fleeting/20261112100000-discard-me.org");
    write_file(
        &f1,
        ":PROPERTIES:\n:ID:       f3333333-0000-0000-0000-000000000003\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Discard Me\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nIrrelevant thought.\n",
    );

    // Discard without delete
    let out = org_cmd()
        .current_dir(&ws)
        .args([
            "fleeting",
            "discard",
            "id:f3333333-0000-0000-0000-000000000003",
        ])
        .output()
        .expect("run discard");
    assert!(out.status.success());
    let updated = fs::read_to_string(&f1).unwrap();
    assert!(updated.contains(":STATUS:       discarded"));
    assert!(updated.contains(":DISCARDED_AT: ["));

    // Discard with delete
    let out_del = org_cmd()
        .current_dir(&ws)
        .args([
            "fleeting",
            "discard",
            "id:f3333333-0000-0000-0000-000000000003",
            "--delete",
        ])
        .output()
        .expect("run discard delete");
    assert!(out_del.status.success());
    assert!(!f1.exists());
}

/// Verifies `org fleeting process` steps interactively through the queue from stdin.
#[test]
fn slice3_process_handles_interactive_stdin_decisions() {
    let temp = TempDir::new().unwrap();
    let (ws, vault) = setup_workspace(&temp);

    let f1 = vault.join("fleeting/20261112100001-thought-task.org");
    write_file(
        &f1,
        ":PROPERTIES:\n:ID:       f4444444-0000-0000-0000-000000000001\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Idea for task\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nActionable item.\n",
    );

    let f2 = vault.join("fleeting/20261112100002-thought-discard.org");
    write_file(
        &f2,
        ":PROPERTIES:\n:ID:       f4444444-0000-0000-0000-000000000002\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Idea for discard\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nNot useful.\n",
    );

    // Stdin script: 't' for task on note 1, 'd' for discard on note 2
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_org"))
        .current_dir(&ws)
        .args(["fleeting", "process"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn org fleeting process");

    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"t\nd\n").expect("write choices to stdin");
    }

    let output = child.wait_with_output().expect("wait on child process");
    println!(
        "PROCESS STDOUT:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    println!(
        "PROCESS STDERR:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());

    // Note 1 converted to task
    let f1_content = fs::read_to_string(&f1).unwrap();
    assert!(f1_content.contains(":STATUS:"));
    assert!(f1_content.contains("converted-to-task"));

    // Note 2 discarded
    let f2_content = fs::read_to_string(&f2).unwrap();
    assert!(f2_content.contains(":STATUS:"));
    assert!(f2_content.contains("discarded"));
}

// endregion
