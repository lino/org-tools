// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP Roam and GTD next actions, capture, and fleeting note workflow integration tests.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use super::{write_file_str, McpTestClient};

/// Seeds a multi-vault workspace with `work` and `personal` directories and roam/GTD tasks.
fn seed_multi_vault_workspace(root: &Path) {
    let toml = "[roam.directories]\nwork = \"vault_work\"\npersonal = \"vault_personal\"\n";
    write_file_str(&root.join(".org-tools.toml"), toml);
    let work_tasks = "#+title: Work Tasks\n#+filetags: :agenda:\n\n* NEXT [#A] Fix database deadlock :@code:\n:PROPERTIES:\n:ID:       11111111-0000-0000-0000-000000000001\n:EFFORT:   1:00\n:ENERGY:   high\n:WORKMODE: deep\n:GOAL:     Performance\n:END:\nResolve transaction isolation contention.\n\n* TODO Write test suite :@code:\n:PROPERTIES:\n:ID:       11111111-0000-0000-0000-000000000002\n:EFFORT:   30m\n:ENERGY:   low\n:WORKMODE: shallow\n:END:\nAdd integration test coverage.\n";
    let personal_tasks = "#+title: Personal Tasks\n#+filetags: :agenda:\n\n* NEXT [#B] Buy groceries :@errands:\n:PROPERTIES:\n:ID:       22222222-0000-0000-0000-000000000001\n:EFFORT:   45m\n:ENERGY:   low\n:END:\nProduce and pantry staples.\n";
    write_file_str(&root.join("vault_work/gtd/tasks.org"), work_tasks);
    write_file_str(&root.join("vault_personal/gtd/home.org"), personal_tasks);
}

/// Verifies `org_query_next` multi-vault filtering, effort/energy constraints, and `--pick` action.
#[test]
fn slice4_org_query_next_supports_filters_pick_and_multi_vault() {
    let dir = TempDir::new().unwrap();
    seed_multi_vault_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);

    let all_next = client.call_ok(1, "org_query_next", json!({}));
    assert_eq!(all_next["count"], 3);

    let work_next = client.call_ok(2, "org_query_next", json!({ "roam_dir": "work" }));
    assert_eq!(work_next["count"], 2);
    assert_eq!(work_next["next_actions"][0]["roam_dir"], "work");
    assert_eq!(work_next["next_actions"][0]["project"], "Work Tasks");

    let picked = client.call_ok(
        3,
        "org_query_next",
        json!({ "roam_dir": "work", "pick": true }),
    );
    assert_eq!(picked["count"], 1);
    assert_eq!(picked["next_actions"][0]["title"], "Fix database deadlock");

    client.assert_err(4, "org_query_next", json!({ "roam_dir": "unknown_vault" }));

    let errands = client.call_ok(5, "org_query_next", json!({ "context": "@errands" }));
    assert_eq!(errands["count"], 1);
    assert_eq!(errands["next_actions"][0]["title"], "Buy groceries");

    let high_energy = client.call_ok(6, "org_query_next", json!({ "energy": "high" }));
    assert_eq!(high_energy["count"], 1);
    assert_eq!(
        high_energy["next_actions"][0]["title"],
        "Fix database deadlock"
    );

    let short_effort = client.call_ok(7, "org_query_next", json!({ "max_effort": "45m" }));
    assert_eq!(short_effort["count"], 2);

    client.close_cleanly();
}

/// Verifies `org_roam_capture` creates notes headlessly with dry-run support, vault routing, and warm cache updates.
#[test]
fn slice4_org_roam_capture_creates_notes_headlessly_with_dry_run_and_cache_sync() {
    let dir = TempDir::new().unwrap();
    seed_multi_vault_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);

    let dry_args = json!({
        "title": "Quantum Computing",
        "type": "zettel",
        "tags": "physics,research",
        "body": "Superposition principles.",
        "roam_dir": "work",
        "dry_run": true
    });
    let dry_res = client.call_ok(1, "org_roam_capture", dry_args);
    assert_eq!(dry_res["dry_run"], true);
    assert_eq!(dry_res["title"], "Quantum Computing");
    assert_eq!(dry_res["roam_dir"], "work");
    let target_file = PathBuf::from(dry_res["file"].as_str().unwrap());
    assert!(!target_file.exists(), "dry_run must not write files");

    let real_args = json!({
        "title": "Deterministic Finite Automata",
        "type": "zettel",
        "tags": "cs,theory",
        "body": "DFA state transitions.",
        "roam_dir": "work"
    });
    let real_res = client.call_ok(2, "org_roam_capture", real_args);
    assert_eq!(real_res["dry_run"], false);
    let captured_id = real_res["id"].as_str().unwrap();
    let written_file = PathBuf::from(real_res["file"].as_str().unwrap());
    assert!(written_file.is_file(), "file must be created on disk");
    let content = fs::read_to_string(&written_file).unwrap();
    assert!(content.contains("#+title: Deterministic Finite Automata"));
    assert!(content.contains("DFA state transitions."));

    let node_query = client.call_ok(3, "org_roam_node", json!({ "query": captured_id }));
    assert_eq!(node_query["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(
        node_query["nodes"][0]["title"],
        "Deterministic Finite Automata"
    );

    client.assert_err(
        4,
        "org_roam_capture",
        json!({
            "title": "Bad Vault",
            "roam_dir": "nonexistent"
        }),
    );

    // Confinement test: slug with parent traversal outside vault/workspace must be rejected
    client.assert_err(
        5,
        "org_roam_capture",
        json!({
            "title": "Evil Escape",
            "slug": "../../escaped",
            "roam_dir": "work"
        }),
    );
    assert!(!dir.path().join("escaped.org").exists());

    client.close_cleanly();
}

/// Verifies `org_fleeting_list` and `org_fleeting_transform` ("to-zettel", "to-task", "discard").
#[test]
fn slice4_org_fleeting_list_and_transform_workflows() {
    let dir = TempDir::new().unwrap();
    seed_multi_vault_workspace(dir.path());
    let mut client = McpTestClient::spawn_ws(&dir);

    let fleeting_dir = dir.path().join("vault_work/fleeting");
    fs::create_dir_all(&fleeting_dir).unwrap();
    let f1 = fleeting_dir.join("20261112120000-fleeting-thought.org");
    let f1_content = ":PROPERTIES:\n:ID:       f1111111-2222-3333-4444-555555555555\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Ephemeral Idea\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nConsider graph database for roam backlinks.\n";
    write_file_str(&f1, f1_content);

    let list_res = client.call_ok(1, "org_fleeting_list", json!({ "status": "unrefined" }));
    assert_eq!(list_res["count"], 1);
    assert_eq!(list_res["items"][0]["title"], "Ephemeral Idea");

    let dry_trans = client.call_ok(
        2,
        "org_fleeting_transform",
        json!({
            "target": "f1111111-2222-3333-4444-555555555555",
            "action": "to-zettel",
            "roam_dir": "work",
            "dry_run": true
        }),
    );
    assert_eq!(dry_trans["dry_run"], true);
    assert_eq!(dry_trans["zettel_title"], "Ephemeral Idea");
    let src_after_dry = fs::read_to_string(&f1).unwrap();
    assert!(src_after_dry.contains(":STATUS:   unrefined"));

    let real_trans = client.call_ok(
        3,
        "org_fleeting_transform",
        json!({
            "target": "f1111111-2222-3333-4444-555555555555",
            "action": "to-zettel",
            "roam_dir": "work"
        }),
    );
    assert_eq!(real_trans["action"], "to-zettel");
    let zettel_file = PathBuf::from(real_trans["zettel_file"].as_str().unwrap());
    assert!(zettel_file.is_file());
    let zettel_text = fs::read_to_string(&zettel_file).unwrap();
    assert!(zettel_text.contains("#+title: Ephemeral Idea"));
    let src_after_real = fs::read_to_string(&f1).unwrap();
    assert!(src_after_real.contains("reformed"));
    assert!(src_after_real.contains(":PERMANENT_NOTE:"));

    let f2 = fleeting_dir.join("20261112120001-task-thought.org");
    let f2_content = ":PROPERTIES:\n:ID:       f2222222-2222-3333-4444-555555555555\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Task Thought\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nActionable bug fix needed.\n";
    write_file_str(&f2, f2_content);

    let task_trans = client.call_ok(
        4,
        "org_fleeting_transform",
        json!({
            "target": "f2222222-2222-3333-4444-555555555555",
            "action": "to-task",
            "roam_dir": "work",
            "context": "@code"
        }),
    );
    assert_eq!(task_trans["action"], "to-task");
    let inbox_file = PathBuf::from(task_trans["inbox_file"].as_str().unwrap());
    assert!(inbox_file.is_file());
    let inbox_text = fs::read_to_string(&inbox_file).unwrap();
    assert!(inbox_text.contains("* TODO Task Thought"));
    assert!(inbox_text.contains(":@code:"));
    let f2_after = fs::read_to_string(&f2).unwrap();
    assert!(f2_after.contains("converted-to-task"));

    let f3 = fleeting_dir.join("20261112120002-discard-thought.org");
    let f3_content = ":PROPERTIES:\n:ID:       f3333333-2222-3333-4444-555555555555\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Discard Thought\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nNoise.\n";
    write_file_str(&f3, f3_content);

    let discard_trans = client.call_ok(
        5,
        "org_fleeting_transform",
        json!({
            "target": "f3333333-2222-3333-4444-555555555555",
            "action": "discard",
            "roam_dir": "work"
        }),
    );
    assert_eq!(discard_trans["action"], "discard");
    let f3_after = fs::read_to_string(&f3).unwrap();
    assert!(f3_after.contains("discarded"));

    // Confinement test: destination "to" with parent traversal must be rejected
    client.assert_err(
        6,
        "org_fleeting_transform",
        json!({
            "target": "f2222222-2222-3333-4444-555555555555",
            "action": "to-task",
            "roam_dir": "work",
            "to": "../../escaped.org"
        }),
    );
    assert!(!dir.path().join("escaped.org").exists());

    // Confinement test: destination "to" in /tmp or absolute outside must be rejected
    client.assert_err(
        7,
        "org_fleeting_transform",
        json!({
            "target": "f2222222-2222-3333-4444-555555555555",
            "action": "to-task",
            "roam_dir": "work",
            "to": "/tmp/escaped.org"
        }),
    );

    // Confinement test: zettel transform with slug parent traversal must be rejected
    let f4 = fleeting_dir.join("20261112120003-evil-slug.org");
    let f4_content = ":PROPERTIES:\n:ID:       f4444444-2222-3333-4444-555555555555\n:TYPE:     fleeting\n:STATUS:   unrefined\n:END:\n#+title: Evil Slug\n#+filetags: :fleeting:\n\n* Fleeting Thoughts\nEscape attempt.\n";
    write_file_str(&f4, f4_content);
    client.assert_err(
        8,
        "org_fleeting_transform",
        json!({
            "target": "f4444444-2222-3333-4444-555555555555",
            "action": "to-zettel",
            "roam_dir": "work",
            "slug": "../../escaped"
        }),
    );
    assert!(!dir.path().join("escaped.org").exists());

    client.close_cleanly();
}
