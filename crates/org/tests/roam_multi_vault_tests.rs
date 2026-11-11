// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for multi-directory `org-roam` configuration, federated cache, and TODO sequences.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use org_tools_core::cache::RoamWorkspace;
use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::rules::format::tag_alignment::TagAlignment;
use org_tools_core::rules::heading::{TodoKeywords, DEFAULT_TODO_KEYWORDS};
use org_tools_core::rules::lint::duplicate_id::DuplicateId;
use org_tools_core::rules::lint::invalid_id_seed::InvalidIdSeed;
use org_tools_core::rules::lint::priority_validity::PriorityValidity;
use org_tools_core::rules::lint::undeclared_tag::UndeclaredTag;
use org_tools_core::rules::{FormatContext, FormatRule, LintContext, LintRule};
use org_tools_core::source::SourceFile;
use org_tools_core::state::{set_state_with_options, SetStateOptions};
use serde_json::Value;
use tempfile::TempDir;

// region Slice 1: Hierarchical Config & Roam Directories

/// Creates an `assert_cmd::Command` targeting the `org` binary.
fn org_cmd() -> Command {
    Command::cargo_bin("org").expect("org binary must compile")
}

/// Writes `content` to `path`, ensuring parent directories exist.
fn write_fixture(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed creating parent directories");
    }
    fs::write(path, content).expect("failed writing test file");
}

/// Verifies hierarchical config loading: global, workspace, and per-directory overrides.
#[test]
fn test_slice1_hierarchical_config_loading() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let global_cfg = root.join("global_config.toml");
    write_fixture(
        &global_cfg,
        r#"
[format]
blank_lines = true
max_consecutive_blank_lines = 3

[agenda]
deadline_warning_days = 30
"#,
    );

    let ws_dir = root.join("workspace");
    let roam_dir = ws_dir.join("roam_vault");
    fs::create_dir_all(&roam_dir).unwrap();

    write_fixture(
        &ws_dir.join(".org-tools.toml"),
        r#"
[agenda]
deadline_warning_days = 21

[todo]
sequences = ["TODO NEXT | DONE"]
"#,
    );

    write_fixture(
        &roam_dir.join(".org-tools.toml"),
        r#"
[todo]
sequences = ["TASK PROGRESS | FINISHED"]
"#,
    );

    let loaded =
        Config::load_for_dir_with_global(&roam_dir, Some(&global_cfg)).expect("load must succeed");
    assert!(loaded.format.blank_lines);
    assert_eq!(loaded.format.max_consecutive_blank_lines, 3);
    assert_eq!(loaded.agenda.deadline_warning_days, 21);

    let kw = loaded
        .todo
        .to_todo_keywords()
        .expect("todo fallback must exist");
    assert!(kw.todo.contains(&"TASK".to_string()));
    assert!(kw.todo.contains(&"PROGRESS".to_string()));
    assert!(kw.done.contains(&"FINISHED".to_string()));
    assert!(!kw.todo.contains(&"NEXT".to_string()));
}

/// Verifies `[roam.directories]` parses named tables, string arrays, tilde/HOME expansion, and env override.
#[test]
fn test_slice1_roam_directories_formats_and_expansion() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    write_fixture(
        &root.join(".org-tools.toml"),
        r#"
[roam]
default_directory = "work"

[roam.directories]
work = "./vault_work"
private = "~/org/private"
home_var = "$HOME/org/home_vault"
"#,
    );

    let loaded = Config::load_for_dir(root).expect("must parse named directories");
    let dirs = loaded.roam_directories();
    assert_eq!(dirs.len(), 3);
    assert!(dirs
        .iter()
        .any(|(name, p)| name == "work" && p == &root.join("vault_work")));
    assert!(dirs
        .iter()
        .any(|(name, p)| name == "private" && p.ends_with("org/private") && !p.starts_with("~")));
    assert!(dirs.iter().any(|(name, p)| {
        name == "home_var"
            && p.ends_with("org/home_vault")
            && !p.to_string_lossy().contains("$HOME")
    }));

    let (def_name, def_path) = loaded.default_roam_dir().expect("default dir must resolve");
    assert_eq!(def_name, "work");
    assert_eq!(def_path, root.join("vault_work"));

    let resolved = loaded.resolve_roam_dir("work");
    assert_eq!(
        resolved,
        Some((Some("work".to_string()), root.join("vault_work")))
    );

    // Also verify string array syntax in a subdirectory config.
    let arr_dir = root.join("array_ws");
    fs::create_dir_all(&arr_dir).unwrap();
    write_fixture(
        &arr_dir.join(".org-tools.toml"),
        r#"
[roam]
directories = ["./alpha", "./beta"]
"#,
    );
    let arr_cfg = Config::load_for_dir(&arr_dir).expect("must parse array directories");
    let arr_dirs = arr_cfg.roam_directories();
    assert_eq!(arr_dirs.len(), 2);
    assert_eq!(arr_dirs[0], ("alpha".to_string(), arr_dir.join("alpha")));
    assert_eq!(arr_dirs[1], ("beta".to_string(), arr_dir.join("beta")));
}

// endregion

// region Slice 2: Additive TODO Sequences, Repeater Reset & Rule Propagation

/// Verifies default keywords contain `WAIT` and `BLOCKED` and `TodoKeywords` tracks sequences.
#[test]
fn test_slice2_todo_keywords_defaults_and_sequences() {
    assert!(DEFAULT_TODO_KEYWORDS.contains(&"WAIT"));
    assert!(DEFAULT_TODO_KEYWORDS.contains(&"BLOCKED"));

    let def = TodoKeywords::default();
    assert!(def.todo.contains(&"WAIT".to_string()));
    assert!(def.todo.contains(&"BLOCKED".to_string()));
    assert!(!def.sequences.is_empty());
}

/// Verifies additive `#+TODO:`, `#+SEQ_TODO:`, `#+TYP_TODO:`, `#+SETUPFILE:`, and `[todo]` fallback.
#[test]
fn test_slice2_additive_todo_setupfile_and_config_fallback() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    write_fixture(&root.join("setup.org"), "#+TODO: BUG VERIFY | FIXED\n");
    let doc_file = root.join("doc.org");
    write_fixture(
        &doc_file,
        concat!(
            "#+SETUPFILE: setup.org\n",
            "#+TODO: TASK PROGRESS | DONE\n",
            "#+SEQ_TODO: DRAFT REVIEW | PUBLISHED\n",
            "#+TYP_TODO: ALICE BOB | MERGED\n",
            "* TASK Task 1\n",
            "* BUG Bug 1\n",
            "* ALICE Assigned\n",
        ),
    );

    let src = SourceFile::new(&doc_file, fs::read_to_string(&doc_file).unwrap());
    let doc = OrgDocument::from_source(&src);

    for kw in [
        "BUG", "VERIFY", "TASK", "PROGRESS", "DRAFT", "REVIEW", "ALICE", "BOB",
    ] {
        assert!(
            doc.todo_keywords.contains(kw),
            "missing active keyword {kw}"
        );
        assert!(!doc.todo_keywords.is_done(kw));
    }
    for done in ["FIXED", "DONE", "PUBLISHED", "MERGED"] {
        assert!(
            doc.todo_keywords.is_done(done),
            "missing done keyword {done}"
        );
    }
    assert_eq!(doc.entries[0].keyword.as_deref(), Some("TASK"));
    assert_eq!(doc.entries[1].keyword.as_deref(), Some("BUG"));
    assert_eq!(doc.entries[2].keyword.as_deref(), Some("ALICE"));

    // Verify `[todo]` config fallback when no in-file `#+TODO:` is present.
    let cfg_dir = root.join("with_cfg");
    fs::create_dir_all(&cfg_dir).unwrap();
    write_fixture(
        &cfg_dir.join(".org-tools.toml"),
        "[todo]\nsequences = [\" custom_todo(c) | custom_done(d) \"]\n",
    );
    let fb_file = cfg_dir.join("fallback.org");
    let fb_raw = "* custom_todo Check config fallback\n";
    write_fixture(&fb_file, fb_raw);
    let fb_src = SourceFile::new(&fb_file, fb_raw.to_string());
    let fb_doc = OrgDocument::from_source(&fb_src);
    assert!(fb_doc.todo_keywords.contains("custom_todo"));
    assert_eq!(fb_doc.entries[0].keyword.as_deref(), Some("custom_todo"));
}

/// Verifies repeating tasks reset to the first not-done keyword of the entry's own sequence.
#[test]
fn test_slice2_repeater_resets_to_entry_sequence() {
    let raw = concat!(
        "#+TODO: TASK PROGRESS | DONE\n",
        "#+TODO: HABIT PRACTICE | FINISHED\n",
        "* PRACTICE Weekly habit\n",
        "SCHEDULED: <2026-03-01 Sun +1w>\n",
    );
    let src = SourceFile::new("fixture.org", raw.to_string());
    let doc = OrgDocument::from_source(&src);
    let options = SetStateOptions {
        add_closed: true,
        advance_repeaters: true,
        today: Some((2026, 3, 2)),
    };
    let res = set_state_with_options(&src, &doc, None, "FINISHED", options)
        .expect("transition must succeed");
    assert!(res.content.contains("* HABIT Weekly habit"));
    assert!(!res.content.contains("* TASK Weekly habit"));
}

/// Verifies `F007`, `W030`, `W033`, `E002`, and `W035` use the document's effective `TodoKeywords`.
#[test]
fn test_slice2_rules_use_effective_todo_keywords() {
    let raw = concat!(
        "#+TODO: PROJACTIVE | PROJDONE\n",
        "#+PRIORITIES: A B B\n",
        "#+TAGS: allowed\n",
        "* PROJACTIVE [#C] Heading One :forbidden:\n",
        ":PROPERTIES:\n",
        ":ID: 11111111-1111-4111-8111-111111111111\n",
        ":ID_SEED: invalid-seed\n",
        ":END:\n",
        "* PROJACTIVE Heading Two :allowed:\n",
        ":PROPERTIES:\n",
        ":ID: 11111111-1111-4111-8111-111111111111\n",
        ":END:\n",
    );
    let src = SourceFile::new("rules.org", raw.to_string());
    let cfg = Config::default();
    let f_ctx = FormatContext::new(&src, &cfg);
    let l_ctx = LintContext::new(&src, &cfg);

    // F007 tag-alignment recognizes PROJACTIVE heading and aligns tags.
    let f007_fixes = TagAlignment.format(&f_ctx);
    assert!(
        !f007_fixes.is_empty(),
        "F007 must align tags on custom TODO heading"
    );

    // W030 priority-validity detects [#C] after custom TODO keyword PROJACTIVE.
    let w030_diags = PriorityValidity.check(&l_ctx);
    assert_eq!(
        w030_diags.len(),
        1,
        "W030 must flag [#C] on PROJACTIVE heading"
    );

    // W033 undeclared-tag detects :forbidden: on PROJACTIVE heading.
    let w033_diags = UndeclaredTag.check(&l_ctx);
    assert_eq!(
        w033_diags.len(),
        1,
        "W033 must flag :forbidden: on PROJACTIVE heading"
    );

    // E002 duplicate-id and W035 invalid-id-seed run cleanly with custom TODO headings.
    let e002_diags = DuplicateId.check(&l_ctx);
    assert_eq!(e002_diags.len(), 1, "E002 must flag duplicate :ID:");
    let w035_diags = InvalidIdSeed.check(&l_ctx);
    assert_eq!(w035_diags.len(), 1, "W035 must flag invalid :ID_SEED:");
}

// endregion

// region Slice 3: Federated RoamWorkspace & CLI

/// Verifies `RoamWorkspace::open` opens `RoamWorkspace::Federated` across multiple vaults.
#[test]
fn test_slice3_federated_roam_workspace_and_cli_selectors() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let vault_work = root.join("vault_work");
    let vault_priv = root.join("vault_priv");
    fs::create_dir_all(&vault_work).unwrap();
    fs::create_dir_all(&vault_priv).unwrap();

    write_fixture(
        &vault_priv.join("priv_note.org"),
        concat!(
            ":PROPERTIES:\n",
            ":ID: bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb\n",
            ":END:\n",
            "#+TITLE: Private Note\n\n",
            "Private body.\n",
        ),
    );
    write_fixture(
        &vault_work.join("work_note.org"),
        concat!(
            ":PROPERTIES:\n",
            ":ID: aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\n",
            ":END:\n",
            "#+TITLE: Work Note\n\n",
            "Cross-vault link to [[id:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb][Private Note]].\n",
        ),
    );

    let ws = RoamWorkspace::open(&[vault_work.clone(), vault_priv.clone()], false)
        .expect("federated open must succeed");
    assert!(matches!(ws, RoamWorkspace::Federated { .. }));

    // Independent per-vault SQLite cache files must exist on disk.
    assert!(vault_work.join(".cache/org-cache.db").is_file());
    assert!(vault_priv.join(".cache/org-cache.db").is_file());

    let nodes_work = ws.resolve("Work Note").expect("resolve Work Note");
    assert_eq!(nodes_work.len(), 1);
    assert_eq!(nodes_work[0].roam_dir.as_deref(), Some("vault_work"));

    let nodes_priv = ws.resolve("Private Note").expect("resolve Private Note");
    assert_eq!(nodes_priv.len(), 1);
    assert_eq!(nodes_priv[0].roam_dir.as_deref(), Some("vault_priv"));

    // Cross-vault backlinks & outgoing links.
    let backlinks = ws.backlinks(&nodes_priv).expect("cross-vault backlinks");
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].source_title.as_deref(), Some("Work Note"));

    let outgoing = ws
        .outgoing(&nodes_work)
        .expect("cross-vault outgoing links");
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].target_title.as_deref(), Some("Private Note"));
    assert!(outgoing[0].resolved);

    // Configure named roam directories in `.org-tools.toml` and test CLI `--roam-dir`.
    write_fixture(
        &root.join(".org-tools.toml"),
        r#"
[roam.directories]
work = "./vault_work"
private = "./vault_priv"
"#,
    );

    let out = org_cmd()
        .current_dir(root)
        .args([
            "roam",
            "node",
            "--roam-dir",
            "work",
            "--roam-dir",
            "private",
            "--format",
            "json",
            "Work Note",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["nodes"][0]["roam_dir"], "work");

    // Unknown `--roam-dir` vault name must exit with code 2.
    org_cmd()
        .current_dir(root)
        .args(["roam", "node", "--roam-dir", "unknown_vault", "Work Note"])
        .assert()
        .code(2);
}

// endregion
