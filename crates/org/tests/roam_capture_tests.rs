// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural integration tests for `org roam capture` and `[capture.templates]` (`ORGFMT-WP-23`).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{
    expand_template_vars, resolve_effective_templates, CaptureTargetKind, CaptureTemplateLayer,
    TemplateVars,
};
use org_tools_core::config::Config;
use serde_json::Value;
use tempfile::TempDir;

// region Helpers

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

/// Creates a portable shell script in `dir` and marks it executable on Unix.
#[cfg(unix)]
fn write_editor_script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join(name);
    write_file(&script, &format!("#!/bin/sh\n{body}\n"));
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}

// endregion

// region Slice 1: Built-In & Multi-Layer [capture.templates] + --list-templates

/// Verifies built-in templates merge with global, workspace, and per-vault `.org-tools.toml` layers.
#[test]
fn slice1_merges_builtin_global_workspace_and_vault_templates_and_expands_vars() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let global_cfg = root.join("global.toml");
    write_file(
        &global_cfg,
        "[capture.templates.reading]\ndescription = \"Global reading note\"\nkeys = \"r\"\ntarget = \"reading/${slug}.org\"\nkind = \"file\"\nroam_type = \"literature\"\nfiletags = [\"reading\"]\nbody = \"* Summary\\n${body}\\n\"\n",
    );

    let ws_dir = root.join("ws");
    let work_vault = ws_dir.join("work");
    let priv_vault = ws_dir.join("private");
    fs::create_dir_all(&work_vault).unwrap();
    fs::create_dir_all(&priv_vault).unwrap();

    write_file(
        &ws_dir.join("templates/adr.org"),
        "* Context\n${body}\n\n* Decision\nParent: ${parent}\n",
    );
    write_file(
        &ws_dir.join(".org-tools.toml"),
        r##"
[roam]
default_directory = "work"
[roam.directories]
work = "./work"
private = "./private"

[capture.templates.adr]
description = "Architecture decision record"
keys = "a"
target = "adr/${year}/${month}/${date}-${slug}.org"
kind = "file"
roam_type = "adr"
todo_header = "DRAFT REVIEW | ACCEPTED"
head = "#+STARTUP: content"
body_file = "./templates/adr.org"
edit = false
properties = { STATUS = "proposed", CAPTURED_AT = "${timestamp_inactive}" }
"##,
    );
    write_file(
        &work_vault.join(".org-tools.toml"),
        "[capture.templates.standup]\ndescription = \"Work standup entry\"\nkeys = \"s\"\ntarget = \"gtd/standup.org\"\nkind = \"entry\"\ntarget_heading = \"* Standup Log\"\nfiletags = [\"standup\", \"work\"]\nbody = \"${body}\"\n",
    );

    let cfg = Config::load_for_roam_dir_with_global(&ws_dir, &work_vault, Some(&global_cfg))
        .expect("load layered config");
    let templates = resolve_effective_templates(&cfg);

    for name in [
        "zettel",
        "concept",
        "technical",
        "howto",
        "person",
        "place",
        "project",
        "fleeting",
        "daily",
        "inbox",
    ] {
        assert_eq!(templates[name].source_layer, CaptureTemplateLayer::Builtin);
    }
    assert_eq!(
        templates["reading"].source_layer,
        CaptureTemplateLayer::Global
    );
    assert_eq!(templates["reading"].keys.as_deref(), Some("r"));
    assert_eq!(
        templates["adr"].source_layer,
        CaptureTemplateLayer::Workspace
    );
    assert_eq!(templates["adr"].kind, CaptureTargetKind::File);
    assert_eq!(templates["adr"].edit, Some(false));
    assert_eq!(
        templates["standup"].source_layer,
        CaptureTemplateLayer::Vault
    );
    assert_eq!(templates["standup"].kind, CaptureTargetKind::Entry);
    assert_eq!(
        templates["standup"].target_heading.as_deref(),
        Some("* Standup Log")
    );

    let mut vars = TemplateVars::new(
        "11111111-2222-3333-4444-555555555555",
        "Event Sourcing Patterns",
        "event-sourcing-patterns",
        "work",
        (2026, 11, 8, 14, 30, 0),
    );
    vars.body = "Append-only log.".to_string();
    vars.medium = "[[id:aaaa][Kleppmann 2017]]".to_string();
    vars.location = "ch. 11".to_string();
    vars.parent = "[[id:bbbb][Distributed Systems]]".to_string();
    vars.custom
        .insert("TEAM".to_string(), "platform".to_string());

    let expanded = expand_template_vars(
        "${id}|${title}|${slug}|${date}|${year}|${month}|${ timestamp }|${timestamp_inactive}|${timestamp_active}|${roam_dir}|${body}|${medium}|${location}|${parent}|${TEAM}",
        &vars,
    );
    assert!(expanded.contains("11111111-2222-3333-4444-555555555555|Event Sourcing Patterns|event-sourcing-patterns|2026-11-08|2026|11|20261108143000|[2026-11-08 Sun 14:30]|<2026-11-08 Sun 14:30>|work|Append-only log.|[[id:aaaa][Kleppmann 2017]]|ch. 11|[[id:bbbb][Distributed Systems]]|platform"));

    let out = org_cmd()
        .current_dir(&ws_dir)
        .env("ORG_TOOLS_CONFIG", &global_cfg)
        .args([
            "roam",
            "capture",
            "--list-templates",
            "--roam-dir",
            "work",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).expect("valid json");
    let list = json["templates"].as_array().expect("templates array");
    for (n, layer) in [
        ("zettel", "builtin"),
        ("reading", "global"),
        ("adr", "workspace"),
        ("standup", "vault"),
    ] {
        assert!(list
            .iter()
            .any(|t| t["name"] == n && t["source_layer"] == layer));
    }

    let priv_out = org_cmd()
        .current_dir(&ws_dir)
        .env("ORG_TOOLS_CONFIG", &global_cfg)
        .args([
            "roam",
            "capture",
            "--list-templates",
            "--roam-dir",
            "private",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let priv_json: Value = serde_json::from_slice(&priv_out).expect("valid json");
    assert!(!priv_json["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "standup"));
}

// endregion

// region Slice 2: Template Selection, $EDITOR Lifecycle, Abort Guard & Atomic Writes

/// Verifies headless capture (`--no-edit` / `--body` / `--stdin`), key/type selection, and link resolution.
#[test]
fn slice2_headless_capture_by_key_and_type_resolves_links_and_entry_targets() {
    let temp = TempDir::new().unwrap();
    let ws_dir = temp.path();
    let work_vault = ws_dir.join("work");
    fs::create_dir_all(&work_vault).unwrap();

    write_file(
        &ws_dir.join(".org-tools.toml"),
        "[roam]\ndefault_directory = \"work\"\n[roam.directories]\nwork = \"./work\"\n[capture.templates.standup]\ndescription = \"Standup entry\"\nkeys = \"s\"\ntarget = \"gtd/standup.org\"\nkind = \"entry\"\ntarget_heading = \"* Standup Log\"\nfiletags = [\"standup\"]\n",
    );
    write_file(
        &work_vault.join("zettel/luhmann-slip-box.org"),
        ":PROPERTIES:\n:ID:       aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\n:TYPE:     concept\n:END:\n#+title: Luhmann Slip-Box\n\n* Thesis\nAtomic linked notes.\n",
    );
    write_file(
        &work_vault.join("zettel/ahrens-book.org"),
        ":PROPERTIES:\n:ID:       bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb\n:TYPE:     literature\n:END:\n#+title: How to Take Smart Notes\n",
    );

    org_cmd()
        .current_dir(ws_dir)
        .args([
            "roam",
            "capture",
            "Folgezettel Branching",
            "--parent",
            "Luhmann Slip-Box",
            "--medium",
            "How to Take Smart Notes",
            "--location",
            "p. 42",
            "--alias",
            "Branching Sequence",
            "--ref",
            "https://zettelkasten.de",
            "--tags",
            "zettelkasten,methodology",
            "--body",
            "Folgezettel creates local context neighbourhoods.",
            "--no-edit",
        ])
        .assert()
        .success();

    let zettel_raw =
        fs::read_to_string(work_vault.join("zettel/folgezettel-branching.org")).unwrap();
    for expected in [
        ":TYPE:            concept",
        ":ROAM_ALIASES:    \"Branching Sequence\"",
        ":ROAM_REFS:       https://zettelkasten.de",
        ":SOURCE_MEDIUM:   [[id:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb][How to Take Smart Notes]]",
        ":MEDIUM_LOCATION: p. 42",
        ":PARENT_ZETTEL:   [[id:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa][Luhmann Slip-Box]]",
        "#+title: Folgezettel Branching",
        "#+filetags: :zettelkasten:methodology:",
        "* Thesis\nFolgezettel creates local context neighbourhoods.",
        "* Related Concepts & Links\n- [[id:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa][Luhmann Slip-Box]]",
    ] {
        assert!(zettel_raw.contains(expected), "missing: {expected}");
    }

    org_cmd()
        .current_dir(ws_dir)
        .args([
            "roam",
            "capture",
            "Shipped federated cache",
            "-t",
            "s",
            "--body",
            "All multi-vault tests green.",
            "--no-edit",
        ])
        .assert()
        .success();

    let standup_raw = fs::read_to_string(work_vault.join("gtd/standup.org")).unwrap();
    assert!(standup_raw.contains("* Standup Log"));
    assert!(standup_raw.contains("** Shipped federated cache"));
    assert!(standup_raw.contains(":ID:"));
    assert!(standup_raw.contains("All multi-vault tests green."));
}

/// Verifies `$EDITOR` success saves edited title/body, while non-zero exit or empty buffer aborts with exit code 1.
#[cfg(unix)]
#[test]
fn slice2_editor_spawn_save_and_abort_exit_codes() {
    let temp = TempDir::new().unwrap();
    let ws_dir = temp.path();
    let vault = ws_dir.join("vault");
    fs::create_dir_all(&vault).unwrap();

    write_file(
        &ws_dir.join(".org-tools.toml"),
        "[roam]\ndefault_directory = \"vault\"\n[roam.directories]\nvault = \"./vault\"\n",
    );

    let fail_editor = write_editor_script(ws_dir, "fail-editor.sh", "exit 1");
    org_cmd()
        .current_dir(ws_dir)
        .env("VISUAL", &fail_editor)
        .args(["roam", "capture", "Aborted Note", "--edit"])
        .assert()
        .code(1);
    assert!(!vault.join("zettel/aborted-note.org").exists());

    let empty_editor = write_editor_script(ws_dir, "empty-editor.sh", ": > \"$1\"");
    org_cmd()
        .current_dir(ws_dir)
        .env("VISUAL", &empty_editor)
        .args(["roam", "capture", "Empty Note", "--edit"])
        .assert()
        .code(1);
    assert!(!vault.join("zettel/empty-note.org").exists());

    let noop_editor = write_editor_script(ws_dir, "noop-editor.sh", "exit 0");
    org_cmd()
        .current_dir(ws_dir)
        .env("VISUAL", &noop_editor)
        .args(["roam", "capture", "--edit"])
        .assert()
        .code(1);

    let good_editor = write_editor_script(
        ws_dir,
        "good-editor.sh",
        "sed 's/#+title: Draft Title/#+title: Refined Editor Title/' \"$1\" > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"\necho \"Edited in $EDITOR.\" >> \"$1\"",
    );
    org_cmd()
        .current_dir(ws_dir)
        .env("VISUAL", &good_editor)
        .args(["roam", "capture", "Draft Title", "--edit"])
        .assert()
        .success();

    let content = fs::read_to_string(vault.join("zettel/draft-title.org")).unwrap();
    assert!(content.contains("#+title: Refined Editor Title"));
    assert!(content.contains("Edited in"));
}

// endregion

// region Slice 3: Cache Sync, Daily Note Logging (log_to_daily) & MutationResult JSON

/// Verifies `org roam capture` updates `.cache/org-cache.db`, appends to daily note when `log_to_daily = true`, and emits `MutationResult` JSON.
#[test]
fn slice3_updates_cache_logs_to_daily_note_and_emits_mutation_json() {
    let temp = TempDir::new().unwrap();
    let ws_dir = temp.path();
    let vault = ws_dir.join("work");
    fs::create_dir_all(vault.join("daily")).unwrap();

    write_file(
        &ws_dir.join(".org-tools.toml"),
        "[roam]\ndefault_directory = \"work\"\nlog_to_daily = true\n[roam.directories]\nwork = \"./work\"\n",
    );
    let _ = WorkspaceCache::rebuild(&vault).expect("initial cache build");

    let out = org_cmd()
        .current_dir(ws_dir)
        .env("ORG_TOOLS_NOW", "2026-11-08T15:45:00")
        .args([
            "roam",
            "capture",
            "CRDT State Convergence",
            "--type",
            "technical",
            "--body",
            "Join-semilattice merge guarantees convergence.",
            "--no-edit",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let res: Value = serde_json::from_slice(&out).expect("valid MutationResult JSON");
    assert_eq!(res["command"], "roam capture");
    assert_eq!(res["files_modified"], 1);
    assert_eq!(res["entries_affected"], 1);
    assert_eq!(res["dry_run"], false);
    assert!(res["details"].as_array().is_some_and(|d| !d.is_empty()));
    assert!(res["telemetry"]["duration_us"].is_number());
    assert!(vault.join("tech/crdt-state-convergence.org").is_file());

    let daily_raw = fs::read_to_string(vault.join("daily/2026-11-08.org")).unwrap();
    assert!(daily_raw.contains("* Captured today"));
    assert!(daily_raw.contains("- 15:45 [technical] :: [[id:"));
    assert!(daily_raw.contains("][CRDT State Convergence]]"));

    let status = WorkspaceCache::status(&vault).expect("cache status");
    assert!(status.exists && status.nodes_count >= 1);
}

// endregion
