// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for level-0 file nodes, BM25 FTS5 search, query predicates, rich show, and mutations.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

/// Constructs an assert_cmd [`Command`] pointing to the workspace `org` binary.
pub(crate) fn org() -> Command {
    Command::cargo_bin("org").unwrap()
}

/// Helper creating a test vault with level-0 file nodes and nested headings.
pub(crate) fn create_test_vault(dir: &Path) {
    let zettel1 = dir.join("zettel1.org");
    fs::write(
        &zettel1,
        ":PROPERTIES:\n\
:ID:       11111111-1111-1111-1111-111111111111\n\
:ROAM_ALIASES: \"My Zettel\" Alpha\n\
:END:\n\
#+TITLE: Compiler Architecture\n\
#+FILETAGS: :kb:rust:\n\
- type :: note\n\
- category :: architecture\n\
- tag :: fast\n\
- tag :: pure\n\
\n\
High performance query engine designed for codebases.\n\
See also [[id:22222222-2222-2222-2222-222222222222][Child Task]] for execution.\n",
    )
    .unwrap();

    let notes = dir.join("notes.org");
    fs::write(
        &notes,
        "#+TITLE: Work Notes\n\
#+FILETAGS: :work:\n\
\n\
* Parent Heading                                                     :urgent:\n\
** Child Task\n\
:PROPERTIES:\n\
:ID:       22222222-2222-2222-2222-222222222222\n\
:END:\n\
- step :: first\n\
- [ ] Checklist item 1\n\
- [X] Checklist item 2\n\
\n\
Body of child task discussing query planner.\n",
    )
    .unwrap();
}

#[test]
fn slice1_empty_query_level_0_nodes_and_predicates() {
    let dir = TempDir::new().unwrap();
    create_test_vault(dir.path());

    // 1. Empty query returns all entries including level-0 file node.
    let assert = org()
        .args(["query", "search", "", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("valid JSON array");
    let arr = json.as_array().expect("array of entries");
    assert!(
        arr.len() >= 3,
        "expected at least 3 entries (1 file node + 2 headings), got {}",
        arr.len()
    );

    // Verify level-0 file node properties.
    let file_node = arr
        .iter()
        .find(|e| e["level"].as_i64() == Some(0))
        .expect("level-0 file node present");
    assert_eq!(file_node["title"].as_str(), Some("Compiler Architecture"));
    assert_eq!(
        file_node["locator"].as_str(),
        Some("id:11111111-1111-1111-1111-111111111111")
    );
    let f_tags: Vec<&str> = file_node["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(f_tags.contains(&"kb"));
    assert!(f_tags.contains(&"rust"));

    // Verify vulpea_meta on file node.
    let vm = &file_node["vulpea_meta"];
    assert!(vm.is_object(), "vulpea_meta should be an object");
    assert_eq!(vm["type"][0].as_str(), Some("note"));
    let tags = vm["tag"]
        .as_array()
        .expect("multi-value tag in vulpea_meta");
    assert_eq!(tags.len(), 2);

    // Verify heading effective tags inheritance.
    let child_node = arr
        .iter()
        .find(|e| e["title"].as_str() == Some("Child Task"))
        .expect("Child Task present");
    let eff_tags: Vec<&str> = child_node["effective_tags"]
        .as_array()
        .expect("effective_tags present")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        eff_tags.contains(&"work"),
        "child inherits filetags: {:?}",
        eff_tags
    );
    assert!(
        eff_tags.contains(&"urgent"),
        "child inherits parent tags: {:?}",
        eff_tags
    );

    // 2. Query filter "level:0".
    let assert_l0 = org()
        .args(["query", "search", "level:0", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();
    let l0_json: Value =
        serde_json::from_slice(&assert_l0.get_output().stdout).expect("valid JSON");
    let l0_arr = l0_json.as_array().unwrap();
    assert_eq!(l0_arr.len(), 1);
    assert_eq!(l0_arr[0]["level"].as_i64(), Some(0));

    // 3. Query filter "has:ID".
    let assert_id = org()
        .args(["query", "search", "has:ID", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();
    let id_json: Value =
        serde_json::from_slice(&assert_id.get_output().stdout).expect("valid JSON");
    assert_eq!(id_json.as_array().unwrap().len(), 2);

    // 4. Query filter "meta:type=note" and "has-meta:category".
    let assert_meta = org()
        .args(["query", "search", "meta:type=note", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();
    let meta_json: Value =
        serde_json::from_slice(&assert_meta.get_output().stdout).expect("valid JSON");
    assert_eq!(meta_json.as_array().unwrap().len(), 1);

    let assert_has_meta = org()
        .args(["query", "search", "has-meta:category", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();
    let has_meta_json: Value =
        serde_json::from_slice(&assert_has_meta.get_output().stdout).expect("valid JSON");
    assert_eq!(has_meta_json.as_array().unwrap().len(), 1);

    // 5. Query filter multi-value "meta:tag=pure".
    let assert_tag_meta = org()
        .args(["query", "search", "meta:tag=pure", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();
    let tag_meta_json: Value =
        serde_json::from_slice(&assert_tag_meta.get_output().stdout).expect("valid JSON");
    assert_eq!(tag_meta_json.as_array().unwrap().len(), 1);

    // 6. Query filter multi-value "property:ROAM_ALIASES=Alpha".
    let assert_alias = org()
        .args([
            "query",
            "search",
            "property:ROAM_ALIASES=Alpha",
            "--format",
            "json",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let alias_json: Value =
        serde_json::from_slice(&assert_alias.get_output().stdout).expect("valid JSON");
    assert_eq!(alias_json.as_array().unwrap().len(), 1);

    // 7. Boolean tag expressions "+rust -work".
    let assert_tags = org()
        .args(["query", "search", "+rust -work", "--format", "json"])
        .arg(dir.path())
        .assert()
        .success();
    let tags_json: Value =
        serde_json::from_slice(&assert_tags.get_output().stdout).expect("valid JSON");
    let tags_arr = tags_json.as_array().unwrap();
    assert_eq!(tags_arr.len(), 1);
    assert_eq!(tags_arr[0]["title"].as_str(), Some("Compiler Architecture"));

    // 8. FTS5 ranked search via --fts.
    let assert_fts = org()
        .args([
            "query",
            "search",
            "--fts",
            "compiler design",
            "--format",
            "json",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let fts_json: Value =
        serde_json::from_slice(&assert_fts.get_output().stdout).expect("valid JSON");
    let fts_arr = fts_json.as_array().unwrap();
    assert!(!fts_arr.is_empty(), "FTS query returned results");
    assert_eq!(fts_arr[0]["title"].as_str(), Some("Compiler Architecture"));
    assert!(
        fts_arr[0].get("rank").is_some(),
        "rank present in FTS output"
    );
    assert!(
        fts_arr[0].get("snippet").is_some(),
        "snippet present in FTS output"
    );
}

#[test]
fn slice2_rich_org_show_and_canonical_digest() {
    let dir = TempDir::new().unwrap();
    create_test_vault(dir.path());

    // 1. Rich show on level-0 file node.
    let assert_show = org()
        .args([
            "show",
            "id:11111111-1111-1111-1111-111111111111",
            "--format",
            "json",
            "--workspace",
        ])
        .arg(dir.path())
        .assert()
        .success();

    let show_val: Value =
        serde_json::from_slice(&assert_show.get_output().stdout).expect("valid JSON");
    assert_eq!(show_val["level"].as_i64(), Some(0));
    assert_eq!(show_val["title"].as_str(), Some("Compiler Architecture"));
    assert!(show_val.get("filetags").is_some());
    assert!(show_val.get("effective_tags").is_some());
    assert!(show_val.get("vulpea_meta").is_some());
    assert!(show_val.get("links").is_some());
    assert!(show_val.get("body").is_some());
    let digest = show_val["digest"].as_str().expect("digest present");
    assert!(digest.starts_with("sha256:"));

    // Verify links parsed properly.
    let links = show_val["links"].as_array().expect("links array");
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0]["target"].as_str(),
        Some("22222222-2222-2222-2222-222222222222")
    );
    assert_eq!(links[0]["description"].as_str(), Some("Child Task"));

    // 2. Test --exclude-properties preserves digest across excluded property modifications.
    let digest1 = digest.to_string();

    // Add an excluded property to the file node drawer.
    let zettel1_path = dir.path().join("zettel1.org");
    let content = fs::read_to_string(&zettel1_path).unwrap();
    let modified = content.replace(":END:", ":APPROVAL_DIGEST: sha256:abcdef1234567890\n:END:");
    fs::write(&zettel1_path, modified).unwrap();

    let assert_show_excluded = org()
        .args([
            "show",
            "id:11111111-1111-1111-1111-111111111111",
            "--format",
            "json",
            "--exclude-properties",
            "APPROVAL_DIGEST,APPROVAL_RECEIPT",
            "--workspace",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let show_ex_val: Value =
        serde_json::from_slice(&assert_show_excluded.get_output().stdout).expect("valid JSON");
    let digest2 = show_ex_val["digest"].as_str().expect("digest present");
    assert_eq!(
        digest1, digest2,
        "digest should match when excluded property is omitted from hash"
    );

    // 3. Query search with --include flag.
    let assert_incl = org()
        .args([
            "query",
            "search",
            "level:0",
            "--include",
            "body,vulpea_meta,checkboxes,links,digest",
            "--format",
            "json",
        ])
        .arg(dir.path())
        .assert()
        .success();
    let incl_val: Value =
        serde_json::from_slice(&assert_incl.get_output().stdout).expect("valid JSON");
    let incl_entry = &incl_val.as_array().unwrap()[0];
    assert!(incl_entry.get("body").is_some());
    assert!(incl_entry.get("vulpea_meta").is_some());
    assert!(incl_entry.get("links").is_some());
    assert!(incl_entry.get("digest").is_some());
}

#[path = "kb_search_and_show_tests/mutations.rs"]
mod mutations;
