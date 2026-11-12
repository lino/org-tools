// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for vault-wide ID link validation, `--id-scope`, and hygiene lints (`ORGFMT-WP-39`).

use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

/// Constructs a cargo command running the `org` umbrella binary.
fn org_cmd() -> Command {
    Command::cargo_bin("org").expect("cargo binary 'org' must build")
}

// region Slice 1: Vault-Wide ID Scope Resolution Tests

/// Verifies Scenario 1: single-file `org fmt check` resolves cross-file vault IDs by default,
/// reports only missing UUIDs, and flags both cross-file IDs when `--id-scope file` is passed.
#[test]
fn slice1_vault_id_scope_cross_directory_resolution_and_file_scope() {
    let temp = TempDir::new().expect("temp directory required");
    let vault = temp.path();
    fs::write(
        vault.join("kb.toml"),
        "[workspace]\nname = \"test-vault\"\n",
    )
    .expect("kb.toml must be written");

    // Valid target note in projects/proj/specs/spec.org.
    let specs_dir = vault.join("projects/proj/specs");
    fs::create_dir_all(&specs_dir).expect("specs dir created");
    fs::write(
        specs_dir.join("spec.org"),
        ":PROPERTIES:\n:ID:       c3f090d8-21e9-56d9-ac56-e7ea489b5f50\n:END:\n#+TITLE: Spec Document\n\n* Specification\nDetails.\n",
    )
    .expect("spec.org must be written");

    // Checked note in planning/work/proj/work-packages.org.
    let work_dir = vault.join("planning/work/proj");
    fs::create_dir_all(&work_dir).expect("work dir created");
    let wp_file = work_dir.join("work-packages.org");
    fs::write(
        &wp_file,
        "* Work Package\n:PROPERTIES:\n:ID: defd3ecd-ed3c-5cbe-bb73-9d0907dc99f6\n:END:\n\nSee [[id:c3f090d8-21e9-56d9-ac56-e7ea489b5f50][Valid Spec]] and [[id:deadbeef-0000-0000-0000-000000000000][Missing Spec]].\n",
    )
    .expect("work-packages.org must be written");

    // Default --id-scope vault: resolves valid spec ID from cache, reports ONLY deadbeef.
    let assert_default = org_cmd()
        .args(["fmt", "check"])
        .arg(&wp_file)
        .assert()
        .failure();

    let default_output = String::from_utf8_lossy(&assert_default.get_output().stdout).to_string();
    assert!(
        default_output.contains("deadbeef-0000-0000-0000-000000000000"),
        "default vault scope must report missing ID"
    );
    assert!(
        !default_output.contains("c3f090d8-21e9-56d9-ac56-e7ea489b5f50"),
        "default vault scope must resolve valid cross-directory spec ID without warning"
    );

    // Explicit --id-scope file: flags BOTH cross-file IDs as unresolved.
    let assert_file_scope = org_cmd()
        .args(["fmt", "check", "--id-scope", "file"])
        .arg(&wp_file)
        .assert()
        .failure();

    let file_output = String::from_utf8_lossy(&assert_file_scope.get_output().stdout).to_string();
    assert!(
        file_output.contains("deadbeef-0000-0000-0000-000000000000"),
        "--id-scope file must report deadbeef"
    );
    assert!(
        file_output.contains("c3f090d8-21e9-56d9-ac56-e7ea489b5f50"),
        "--id-scope file must report cross-file spec ID as unresolved"
    );
}

/// Verifies that single-file `org fmt check` exits 0 when all cross-file vault IDs are valid.
#[test]
fn slice1_vault_id_scope_clean_file_exits_0() {
    let temp = TempDir::new().expect("temp directory required");
    let vault = temp.path();
    fs::write(
        vault.join("kb.toml"),
        "[workspace]\nname = \"clean-vault\"\n",
    )
    .expect("kb.toml written");

    let specs_dir = vault.join("specs");
    fs::create_dir_all(&specs_dir).expect("specs dir created");
    fs::write(
        specs_dir.join("spec.org"),
        ":PROPERTIES:\n:ID:       valid-target-uuid-1\n:END:\n#+TITLE: Target\n",
    )
    .expect("spec.org written");

    let caller_file = vault.join("caller.org");
    fs::write(
        &caller_file,
        "* Caller\nSee [[id:valid-target-uuid-1][Target Node]].\n",
    )
    .expect("caller.org written");

    org_cmd()
        .args(["fmt", "check"])
        .arg(&caller_file)
        .assert()
        .success();
}

/// Verifies `W036` (`unresolved-edna-ids`) resolution honors `--id-scope vault` and `--id-scope file`.
#[test]
fn slice1_edna_ids_resolution_with_id_scope() {
    let temp = TempDir::new().expect("temp directory required");
    let vault = temp.path();
    fs::write(
        vault.join("kb.toml"),
        "[workspace]\nname = \"edna-vault\"\n",
    )
    .expect("kb.toml written");

    let blocker_file = vault.join("blockers.org");
    fs::write(
        &blocker_file,
        "* TODO Upstream Task\n:PROPERTIES:\n:ID:       upstream-task-id-1\n:END:\n",
    )
    .expect("blockers.org written");

    let task_file = vault.join("downstream.org");
    fs::write(
        &task_file,
        concat!(
            "* TODO Downstream Task\n",
            ":PROPERTIES:\n",
            ":BLOCKER: ids(\"upstream-task-id-1\")\n",
            ":TRIGGER: ids(\"missing-trigger-uuid\") todo!(\"NEXT\")\n",
            ":END:\n",
        ),
    )
    .expect("downstream.org written");

    // Under vault scope, upstream-task-id-1 resolves, missing-trigger-uuid emits W036.
    let vault_assert = org_cmd()
        .args(["fmt", "check"])
        .arg(&task_file)
        .assert()
        .failure();

    let vault_out = String::from_utf8_lossy(&vault_assert.get_output().stdout).to_string();
    assert!(vault_out.contains("missing-trigger-uuid"));
    assert!(!vault_out.contains("upstream-task-id-1"));

    // Under file scope, upstream-task-id-1 is also unresolved.
    let file_assert = org_cmd()
        .args(["fmt", "check", "--id-scope", "file"])
        .arg(&task_file)
        .assert()
        .failure();

    let file_out = String::from_utf8_lossy(&file_assert.get_output().stdout).to_string();
    assert!(file_out.contains("upstream-task-id-1"));
    assert!(file_out.contains("missing-trigger-uuid"));
}

/// Verifies duplicate `:ID:` detection across files under `vault` scope vs `file` scope.
#[test]
fn slice1_duplicate_id_resolution_with_id_scope() {
    let temp = TempDir::new().expect("temp directory required");
    let vault = temp.path();
    fs::write(vault.join("kb.toml"), "[workspace]\nname = \"dup-vault\"\n")
        .expect("kb.toml written");

    let file1 = vault.join("file1.org");
    fs::write(
        &file1,
        "* Item One\n:PROPERTIES:\n:ID:       colliding-id-uuid-1\n:END:\n",
    )
    .expect("file1.org written");

    let file2 = vault.join("file2.org");
    fs::write(
        &file2,
        "* Item Two\n:PROPERTIES:\n:ID:       colliding-id-uuid-1\n:END:\n",
    )
    .expect("file2.org written");

    // Vault scope flags cross-file duplicate ID collision.
    let assert_vault = org_cmd()
        .args(["fmt", "check"])
        .arg(&file2)
        .assert()
        .failure();

    let vault_out = String::from_utf8_lossy(&assert_vault.get_output().stdout).to_string();
    assert!(
        vault_out.contains("duplicate :ID: \"colliding-id-uuid-1\""),
        "vault scope must flag cross-file duplicate ID"
    );

    // File scope does not flag cross-file duplicate (only intra-file duplicates).
    org_cmd()
        .args(["fmt", "check", "--id-scope", "file"])
        .arg(&file2)
        .assert()
        .success();
}

/// Verifies `--id-scope dir` narrows resolution strictly to files in the target directory.
#[test]
fn slice1_id_scope_dir_narrows_to_target_directory() {
    let temp = TempDir::new().expect("temp directory required");
    let vault = temp.path();
    fs::write(vault.join("kb.toml"), "[workspace]\nname = \"dir-vault\"\n")
        .expect("kb.toml written");

    let dir_a = vault.join("dir_a");
    let dir_b = vault.join("dir_b");
    fs::create_dir_all(&dir_a).expect("dir_a created");
    fs::create_dir_all(&dir_b).expect("dir_b created");

    fs::write(
        dir_b.join("target.org"),
        "* Target\n:PROPERTIES:\n:ID:       target-in-b\n:END:\n",
    )
    .expect("target.org written");

    let caller_file = dir_a.join("caller.org");
    fs::write(
        &caller_file,
        "* Caller\nSee [[id:target-in-b][Target in B]].\n",
    )
    .expect("caller.org written");

    // Under vault scope, target-in-b resolves across directories.
    org_cmd()
        .args(["fmt", "check"])
        .arg(&caller_file)
        .assert()
        .success();

    // Under dir scope, target-in-b is outside dir_a and therefore unresolved.
    let assert_dir = org_cmd()
        .args(["fmt", "check", "--id-scope", "dir"])
        .arg(&caller_file)
        .assert()
        .failure();

    let dir_out = String::from_utf8_lossy(&assert_dir.get_output().stdout).to_string();
    assert!(
        dir_out.contains("target-in-b"),
        "dir scope must not resolve IDs defined outside dir_a"
    );
}

// endregion

// region Slice 2: W023 & W004 Fixes and Hygiene Lint Tests

/// Verifies `W023` (`link-to-local-file`) splits bracketed description cleanly at `][`,
/// and skips comments, drawers, and inline verbatim spans.
#[test]
fn slice2_w023_link_to_local_file_splits_description_and_skips_markup() {
    let temp = TempDir::new().expect("temp directory required");
    let dir = temp.path();

    let target_file = dir.join("existing_target.org");
    fs::write(&target_file, "* Existing Target\n").expect("target written");

    let test_file = dir.join("check_links.org");
    fs::write(
        &test_file,
        concat!(
            ":PROPERTIES:\n",
            ":SOURCE: [[file:nonexistent_in_drawer.org]]\n",
            ":END:\n",
            "# [[file:nonexistent_in_comment.org]]\n",
            "Literal code ~[[file:nonexistent_in_code.org]]~ and =[[file:nonexistent_in_verb.org]]=\n",
            "Valid link with label: [[file:existing_target.org][Descriptive Label with [brackets] inside]].\n",
            "Broken link with label: [[file:actually_missing.org][Broken Label]].\n",
        ),
    )
    .expect("test_file written");

    let assert_cmd = org_cmd()
        .args(["fmt", "check", "--id-scope", "file"])
        .arg(&test_file)
        .assert()
        .failure();

    let output = String::from_utf8_lossy(&assert_cmd.get_output().stdout).to_string();
    assert!(
        output.contains("actually_missing.org"),
        "missing local file must be flagged"
    );
    assert!(
        !output.contains("existing_target.org"),
        "existing local file with bracketed description must not trigger false positive"
    );
    assert!(
        !output.contains("nonexistent_in_drawer.org"),
        "drawer links must be skipped"
    );
    assert!(
        !output.contains("nonexistent_in_comment.org"),
        "comment links must be skipped"
    );
    assert!(
        !output.contains("nonexistent_in_code.org"),
        "inline code links must be skipped"
    );
    assert!(
        !output.contains("nonexistent_in_verb.org"),
        "inline verbatim links must be skipped"
    );
}

/// Verifies `W004` (`orphaned-footnotes`) ignores verbatim markup, comments, and drawers,
/// and reports accurate 1-based line and column for undefined footnote references.
#[test]
fn slice2_w004_orphaned_footnotes_skips_verbatim_and_reports_accurate_line_col() {
    let temp = TempDir::new().expect("temp directory required");
    let dir = temp.path();
    let test_file = dir.join("footnotes.org");
    fs::write(
        &test_file,
        concat!(
            ":PROPERTIES:\n",
            ":NOTE: [fn:in_drawer]\n",
            ":END:\n",
            "# [fn:in_comment]\n",
            "Inline code ~[fn:in_code]~ and verbatim =[fn:in_verb]=.\n",
            "Real text with [fn:undefined_ref] on line 6.\n",
        ),
    )
    .expect("test_file written");

    let assert_cmd = org_cmd()
        .args(["fmt", "check"])
        .arg(&test_file)
        .assert()
        .failure();

    let output = String::from_utf8_lossy(&assert_cmd.get_output().stdout).to_string();
    assert!(
        output.contains("undefined_ref"),
        "undefined footnote must be flagged"
    );
    assert!(
        output.contains(":6:"),
        "undefined footnote diagnostic must report accurate line 6: {output}"
    );
    assert!(
        !output.contains("in_drawer"),
        "drawer footnote syntax must be ignored"
    );
    assert!(
        !output.contains("in_comment"),
        "comment footnote syntax must be ignored"
    );
    assert!(
        !output.contains("in_code"),
        "code footnote syntax must be ignored"
    );
    assert!(
        !output.contains("in_verb"),
        "verbatim footnote syntax must be ignored"
    );
}

/// Verifies opt-in `W038` (`zettel-task-hygiene`), `W039` (`ephemeral-header`), and `W040` (`oversized-table-cell`).
#[test]
fn slice2_opt_in_zettel_and_authoring_hygiene_lints() {
    let temp = TempDir::new().expect("temp directory required");
    let dir = temp.path();

    // Default configuration: none of W038, W039, W040 are active.
    let note_file = dir.join("concept_note.org");
    fs::write(
        &note_file,
        concat!(
            ":PROPERTIES:\n",
            ":ID:   concept-uuid-1\n",
            ":TYPE: concept\n",
            ":END:\n",
            "#+TITLE: Concept Note\n",
            "#+AUTHOR: Jane Doe\n",
            "#+DATE: 2026-11-12\n\n",
            "* TODO Active Task in Concept\n",
            "- [ ] Checklist item in concept\n\n",
            "| This is an excessively long table cell that contains more than forty characters of prose in one cell |\n",
        ),
    )
    .expect("note_file written");

    // Without opt-in, standard check passes cleanly.
    org_cmd()
        .args(["fmt", "check", "--id-scope", "file"])
        .arg(&note_file)
        .assert()
        .success();

    // With opt-in configuration in .org-tools.toml:
    let config_file = dir.join(".org-tools.toml");
    fs::write(
        &config_file,
        concat!(
            "[lint]\n",
            "enable = [\"W038\", \"W039\", \"W040\"]\n",
            "[lint.authoring]\n",
            "max_table_cell_chars = 40\n",
        ),
    )
    .expect("config written");

    let assert_opt_in = org_cmd()
        .args(["fmt", "check", "--id-scope", "file", "--config"])
        .arg(&config_file)
        .arg(&note_file)
        .assert()
        .failure();

    let output = String::from_utf8_lossy(&assert_opt_in.get_output().stdout).to_string();
    assert!(
        output.contains("W038")
            || output.contains("zettel-task-hygiene")
            || output.contains("concept"),
        "opt-in W038 must flag TODO or checklist in concept note: {output}"
    );
    assert!(
        output.contains("W039") || output.contains("ephemeral-header") || output.contains("AUTHOR"),
        "opt-in W039 must flag ephemeral headers: {output}"
    );
    assert!(
        output.contains("W040")
            || output.contains("oversized-table-cell")
            || output.contains("table cell"),
        "opt-in W040 must flag oversized table cell: {output}"
    );
}

// endregion
