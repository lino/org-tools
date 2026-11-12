// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for ORGFMT-WP-38:
//! Auto-Incremental `WorkspaceCache` (Delta Read + Write-Through Sync) and
//! File/Element Random/Seeded `org update add-id`.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use assert_cmd::Command;
use org_tools_core::cache::WorkspaceCache;
use tempfile::TempDir;

// region Helpers

/// Returns a configured [`Command`] targeting the umbrella `org` binary.
fn org() -> Command {
    Command::cargo_bin("org").expect("cargo binary 'org' must be built")
}

/// Creates a temporary workspace populated with `count` minimal `.org` files.
fn populate_synthetic_vault(count: usize) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir creation failed");
    let root = dir.path().to_path_buf();
    for idx in 0..count {
        let file_path = root.join(format!("note_{idx:04}.org"));
        let content =
            format!("#+TITLE: Note {idx:04}\n\n* Task {idx}\nSome body text for entry {idx}.\n");
        fs::write(&file_path, content).expect("failed writing test note");
    }
    (dir, root)
}

// endregion

// region Slice 1: WorkspaceCache Delta Read & Write-Through Sync

/// Verifies that a warm no-op sync on 500 files skips file reads and AST parsing within budget.
#[test]
fn slice1_warm_no_op_sync_within_perf_budget() {
    let (_guard, root) = populate_synthetic_vault(500);

    // Initial cold build
    let (cache, cold_tel) =
        WorkspaceCache::open_and_sync(&root).expect("initial open_and_sync failed");
    assert_eq!(cold_tel.cache_misses, 500);
    assert_eq!(cold_tel.cache_hits, 0);
    drop(cache);

    // Warm incremental sync
    let started = Instant::now();
    let (_cache, warm_tel) =
        WorkspaceCache::open_and_sync(&root).expect("warm open_and_sync failed");
    let elapsed = started.elapsed();

    assert_eq!(warm_tel.files_scanned, 500);
    assert_eq!(warm_tel.cache_hits, 500);
    assert_eq!(warm_tel.cache_misses, 0);
    assert!(
        elapsed.as_millis() < 100,
        "warm no-op sync took {} ms (expected < 100 ms)",
        elapsed.as_millis()
    );
}

/// Verifies that touching a file without content change updates mtime without AST re-parsing.
#[test]
fn slice1_touch_without_byte_change_updates_mtime_without_ast_reparse() {
    let (_guard, root) = populate_synthetic_vault(10);
    let (mut cache, _) = WorkspaceCache::open_and_sync(&root).expect("initial sync failed");

    // Rewrite note_0000.org with exact same bytes to simulate external touch
    let note0 = root.join("note_0000.org");
    let orig_content = fs::read(&note0).expect("failed reading note_0000");
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&note0, &orig_content).expect("failed rewriting note_0000");

    let tel = cache.refresh().expect("refresh failed");
    assert_eq!(tel.files_scanned, 10);
    assert_eq!(tel.cache_hits, 10);
    assert_eq!(tel.cache_misses, 0);
}

/// Verifies that modified, added, and deleted files sync transactionally in a single pass.
#[test]
fn slice1_modified_added_deleted_sync_transactionally() {
    let (_guard, root) = populate_synthetic_vault(5);
    let (mut cache, _) = WorkspaceCache::open_and_sync(&root).expect("initial sync failed");

    // Add note_0005.org
    let new_note = root.join("note_0005.org");
    fs::write(
        &new_note,
        ":PROPERTIES:\n:ID: new-node-5\n:END:\n#+TITLE: Note 5\n* Item 5\n",
    )
    .expect("failed writing note_0005");

    // Modify note_0001.org
    let mod_note = root.join("note_0001.org");
    fs::write(
        &mod_note,
        "#+TITLE: Modified Note 1\n* New Heading\nNew body\n",
    )
    .expect("failed modifying note_0001");

    // Delete note_0002.org
    let del_note = root.join("note_0002.org");
    fs::remove_file(&del_note).expect("failed deleting note_0002");

    let tel = cache.refresh().expect("refresh failed");
    assert_eq!(tel.files_scanned, 5);
    assert_eq!(tel.cache_hits, 3);
    assert_eq!(tel.cache_misses, 2);

    let ids = cache
        .complete_ids("new-node-5")
        .expect("complete_ids failed");
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].id, "new-node-5");
    assert_eq!(ids[0].level, 0);
}

/// Verifies both level-0 file :PROPERTIES: nodes and heading nodes are indexed.
#[test]
fn slice1_level_0_file_nodes_and_headings_indexed() {
    let dir = TempDir::new().expect("tempdir creation failed");
    let root = dir.path();
    let note = root.join("dual.org");
    fs::write(
        &note,
        "# Comment line\n:PROPERTIES:\n:ID:       file-level-id-01\n:END:\n#+TITLE: Dual Note\n* Sub Heading\n:PROPERTIES:\n:ID:       heading-id-01\n:END:\nBody\n",
    )
    .expect("failed writing dual.org");

    let (cache, _) = WorkspaceCache::open_and_sync(root).expect("open_and_sync failed");
    let file_cand = cache
        .complete_ids("file-level-id-01")
        .expect("lookup file id failed");
    assert_eq!(file_cand.len(), 1);
    assert_eq!(file_cand[0].level, 0);
    assert_eq!(file_cand[0].line, 1);

    let heading_cand = cache
        .complete_ids("heading-id-01")
        .expect("lookup heading id failed");
    assert_eq!(heading_cand.len(), 1);
    assert_eq!(heading_cand[0].level, 1);
    assert_eq!(heading_cand[0].line, 6);
}

/// Verifies WorkspaceCache::sync_written_files updates cache inline without full workspace scan.
#[test]
fn slice1_sync_written_files_inline_updates_cache() {
    let (_guard, root) = populate_synthetic_vault(5);
    let (cache, _) = WorkspaceCache::open_and_sync(&root).expect("initial sync failed");
    drop(cache);

    // Mutate one file externally
    let note0 = root.join("note_0000.org");
    fs::write(
        &note0,
        ":PROPERTIES:\n:ID: inline-written-id\n:END:\n#+TITLE: Written Note\n* Item\n",
    )
    .expect("failed mutating note0");

    let synced = WorkspaceCache::sync_written_files(&root, std::slice::from_ref(&note0))
        .expect("sync_written_files failed");
    assert!(synced);

    let (cache2, _) = WorkspaceCache::open_and_sync(&root).expect("reopen failed");
    let cand = cache2
        .complete_ids("inline-written-id")
        .expect("complete_ids failed");
    assert_eq!(cand.len(), 1);
    assert_eq!(cand[0].level, 0);
}

// endregion

// region Slice 2: File & Element ID Assignment (org update add-id)

/// Verifies `org update add-id --file` inserts a level-0 drawer after comments and before title.
#[test]
fn slice2_add_id_file_inserts_level_0_drawer_after_comments_before_title() {
    let dir = TempDir::new().expect("tempdir creation failed");
    let note = dir.path().join("zettel.org");
    fs::write(
        &note,
        "# Copyright 2026\n# SPDX-License-Identifier: GPL-3.0-or-later\n#+TITLE: New Zettel\n* Section\nSome text\n",
    )
    .expect("failed writing zettel.org");

    org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "--file",
            "--format",
            "json",
            "zettel.org",
        ])
        .assert()
        .success();

    let content = fs::read_to_string(&note).expect("failed reading zettel.org");
    assert!(content.contains(":PROPERTIES:"));
    assert!(content.contains(":ID:"));

    // Check placement: drawer must be before #+TITLE: and after comments
    let prop_pos = content.find(":PROPERTIES:").expect("PROPERTIES not found");
    let title_pos = content.find("#+TITLE:").expect("TITLE not found");
    let comment_pos = content.find("# Copyright 2026").expect("comment not found");
    assert!(comment_pos < prop_pos);
    assert!(prop_pos < title_pos);

    // Headings must NOT be touched
    assert!(!content.contains("* Section\n:PROPERTIES:"));

    // Second run must be an idempotent no-op
    let out = org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "--file",
            "--format",
            "json",
            "zettel.org",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).expect("invalid json");
    assert_eq!(json["files_modified"], 0);
    assert_eq!(json["records"][0]["created"], false);
}

/// Verifies `org update add-id --headings --max-level 2` respects heading depth filters.
#[test]
fn slice2_add_id_headings_respects_max_level() {
    let dir = TempDir::new().expect("tempdir creation failed");
    let note = dir.path().join("hierarchy.org");
    fs::write(&note, "* Level 1\n** Level 2\n*** Level 3\n**** Level 4\n")
        .expect("failed writing hierarchy.org");

    org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "--headings",
            "--max-level",
            "2",
            "hierarchy.org",
        ])
        .assert()
        .success();

    let content = fs::read_to_string(&note).expect("failed reading hierarchy.org");
    assert!(content.contains("* Level 1\n:PROPERTIES:"));
    assert!(content.contains("** Level 2\n:PROPERTIES:"));
    assert!(!content.contains("*** Level 3\n:PROPERTIES:"));
    assert!(!content.contains("**** Level 4\n:PROPERTIES:"));
}

/// Verifies deterministic UUIDv5 seed via `--uuidv5-seed` with and without `--write-seed`.
#[test]
fn slice2_add_id_deterministic_uuidv5_seed_and_write_seed() {
    let dir = TempDir::new().expect("tempdir creation failed");
    let note1 = dir.path().join("note1.org");
    fs::write(&note1, "#+TITLE: Note 1\n").expect("failed writing note1.org");

    // Without --write-seed: writes :ID:, does NOT write :ID_SEED:
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "--file",
            "--uuidv5-seed",
            "urn:kb:test:seed-01",
            "note1.org",
        ])
        .assert()
        .success();

    let c1 = fs::read_to_string(&note1).expect("failed reading note1.org");
    assert!(c1.contains(":ID:"));
    assert!(!c1.contains(":ID_SEED:"));

    // With --write-seed: writes both :ID: and :ID_SEED:
    let note2 = dir.path().join("note2.org");
    fs::write(&note2, "#+TITLE: Note 2\n").expect("failed writing note2.org");
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "--file",
            "--uuidv5-seed",
            "urn:kb:test:seed-02",
            "--write-seed",
            "note2.org",
        ])
        .assert()
        .success();

    let c2 = fs::read_to_string(&note2).expect("failed reading note2.org");
    assert!(c2.contains(":ID:"));
    assert!(c2.contains(":ID_SEED:"));
    assert!(c2.contains("urn:kb:test:seed-02"));
}

/// Verifies Scenario 2: minting random file :ID: and validating an incoming link via inline cache sync.
#[test]
fn slice2_scenario_2_file_id_and_link_validation_with_inline_cache_sync() {
    let dir = TempDir::new().expect("tempdir creation failed");
    let zettel_dir = dir.path().join("zettel");
    fs::create_dir_all(&zettel_dir).expect("failed creating zettel dir");

    let new_note = zettel_dir.join("new_note.org");
    fs::write(&new_note, "#+TITLE: New Note\nBody prose here.\n")
        .expect("failed writing new_note.org");

    let index_note = zettel_dir.join("index_note.org");
    fs::write(&index_note, "#+TITLE: Index Note\n* Index\n")
        .expect("failed writing index_note.org");

    // Run org update add-id --file zettel/new_note.org --format json
    let out = org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "--file",
            "--format",
            "json",
            "zettel/new_note.org",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&out).expect("invalid json output");
    assert_eq!(json["cache_synced"], true);
    let minted_id = json["records"][0]["id"]
        .as_str()
        .expect("minted id must be string");

    // Insert [[id:<returned-uuid>][New Note]] into index_note.org
    let link_text = format!("[[id:{minted_id}][New Note]]");
    let mut index_content = fs::read_to_string(&index_note).expect("failed reading index_note");
    index_content.push_str(&format!("- Related: {link_text}\n"));
    fs::write(&index_note, &index_content).expect("failed updating index_note");

    // Run org fmt check zettel/index_note.org; should exit 0 with 0 warnings because cache was updated inline
    org()
        .current_dir(dir.path())
        .args(["fmt", "check", "zettel/index_note.org"])
        .assert()
        .success();
}

// endregion
