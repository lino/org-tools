// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests for property and link mutations on file nodes and heading entries.

use std::fs;

use serde_json::Value;
use tempfile::TempDir;

use super::{create_test_vault, org};

// region Slice 2: Property and Link Mutations

#[test]
fn slice2_property_and_link_mutations() {
    let dir = TempDir::new().unwrap();
    create_test_vault(dir.path());

    // 1. Set property on level-0 file node.
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "set-property",
            "id:11111111-1111-1111-1111-111111111111",
            "STATUS",
            "draft",
        ])
        .assert()
        .success();

    // Verify property was set.
    let zettel1_path = dir.path().join("zettel1.org");
    let content = fs::read_to_string(&zettel1_path).unwrap();
    assert!(
        content.contains(":STATUS:       draft"),
        "content should contain STATUS with F005 alignment: {content}"
    );

    // Verify org fmt check passes.
    org()
        .current_dir(dir.path())
        .args(["fmt", "check", dir.path().to_str().unwrap()])
        .assert()
        .success();

    // 2. Delete property on level-0 file node.
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "delete-property",
            "id:11111111-1111-1111-1111-111111111111",
            "STATUS",
        ])
        .assert()
        .success();

    let content_del = fs::read_to_string(&zettel1_path).unwrap();
    assert!(!content_del.contains("STATUS"));
    org()
        .current_dir(dir.path())
        .args(["fmt", "check", dir.path().to_str().unwrap()])
        .assert()
        .success();

    // 3. Insert link under a new section.
    org()
        .args([
            "update",
            "link",
            "id:11111111-1111-1111-1111-111111111111",
            "--to",
            "id:22222222-2222-2222-2222-222222222222",
            "--section",
            "See Also",
            "--workspace",
        ])
        .arg(dir.path())
        .assert()
        .success();

    let content_linked = fs::read_to_string(&zettel1_path).unwrap();
    assert!(
        content_linked
            .contains("* See Also\n- [[id:22222222-2222-2222-2222-222222222222][Child Task]]")
            || content_linked.contains(
                "* See Also\n\n- [[id:22222222-2222-2222-2222-222222222222][Child Task]]"
            )
            || content_linked.contains("- [[id:22222222-2222-2222-2222-222222222222][Child Task]]"),
        "link inserted in See Also section: {content_linked}"
    );

    // Verify idempotency: running again should not add a duplicate link.
    org()
        .args([
            "update",
            "link",
            "id:11111111-1111-1111-1111-111111111111",
            "--to",
            "id:22222222-2222-2222-2222-222222222222",
            "--section",
            "See Also",
            "--workspace",
        ])
        .arg(dir.path())
        .assert()
        .success();

    let content_idempotent = fs::read_to_string(&zettel1_path).unwrap();
    let count = content_idempotent
        .matches("[[id:22222222-2222-2222-2222-222222222222")
        .count();
    assert_eq!(
        count, 2,
        "idempotent link insertion should not duplicate link in section: {content_idempotent}"
    );

    // Verify org fmt check still clean.
    org()
        .current_dir(dir.path())
        .args(["fmt", "check", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn link_heading_entry_scoped_before_next_heading() {
    let dir = TempDir::new().unwrap();
    let doc_path = dir.path().join("doc.org");
    let target1_path = dir.path().join("target1.org");
    let target2_path = dir.path().join("target2.org");

    fs::write(
        &doc_path,
        "#+TITLE: Multi-heading Document\n\n\
* First Heading\n\
:PROPERTIES:\n\
:ID:       aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\n\
:END:\n\
Initial body of first heading.\n\n\
* Second Heading\n\
:PROPERTIES:\n\
:ID:       bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb\n\
:END:\n\
Initial body of second heading.\n",
    )
    .unwrap();

    fs::write(
        &target1_path,
        ":PROPERTIES:\n\
:ID:       cccccccc-cccc-cccc-cccc-cccccccccccc\n\
:END:\n\
#+TITLE: Target Section Note\n",
    )
    .unwrap();

    fs::write(
        &target2_path,
        ":PROPERTIES:\n\
:ID:       dddddddd-dddd-dddd-dddd-dddddddddddd\n\
:END:\n\
#+TITLE: Target Direct Note\n",
    )
    .unwrap();

    // 1. Insert link under a section on * First Heading.
    org()
        .args([
            "update",
            "link",
            "id:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "--to",
            "id:cccccccc-cccc-cccc-cccc-cccccccccccc",
            "--section",
            "Related",
            "--workspace",
        ])
        .arg(dir.path())
        .assert()
        .success();

    let content_after_section = fs::read_to_string(&doc_path).unwrap();
    let first_idx = content_after_section.find("* First Heading").unwrap();
    let second_idx = content_after_section.find("* Second Heading").unwrap();
    let section_link_idx = content_after_section
        .find("[[id:cccccccc-cccc-cccc-cccc-cccccccccccc][Target Section Note]]")
        .expect("section link must be present");

    assert!(
        section_link_idx > first_idx && section_link_idx < second_idx,
        "section link must be inside First Heading and BEFORE Second Heading. Content:\n{content_after_section}"
    );
    assert!(
        content_after_section.contains("** Related"),
        "child section for level-1 heading must be level-2 (** Related)"
    );

    // 2. Insert direct link without --section on * First Heading.
    org()
        .args([
            "update",
            "link",
            "id:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "--to",
            "id:dddddddd-dddd-dddd-dddd-dddddddddddd",
            "--workspace",
        ])
        .arg(dir.path())
        .assert()
        .success();

    let content_after_direct = fs::read_to_string(&doc_path).unwrap();
    let second_idx2 = content_after_direct.find("* Second Heading").unwrap();
    let direct_link_idx = content_after_direct
        .find("[[id:dddddddd-dddd-dddd-dddd-dddddddddddd][Target Direct Note]]")
        .expect("direct link must be present");

    assert!(
        direct_link_idx > first_idx && direct_link_idx < second_idx2,
        "direct link must be inside First Heading and BEFORE Second Heading. Content:\n{content_after_direct}"
    );

    // Second heading content must remain completely unchanged.
    assert!(
        content_after_direct.contains("* Second Heading\n:PROPERTIES:\n:ID:       bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb\n:END:\nInitial body of second heading.\n"),
        "second heading must remain intact"
    );
}

#[test]
fn set_property_on_file_with_title_creates_drawer_before_title() {
    let dir = TempDir::new().unwrap();
    let note_path = dir.path().join("note.org");

    fs::write(
        &note_path,
        "# Comment line\n#+TITLE: My Note\n#+FILETAGS: :draft:\n\nProse in document.\n",
    )
    .unwrap();

    // Set property on file node (targeting file path).
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "set-property",
            note_path.to_str().unwrap(),
            "STATUS",
            "active",
        ])
        .assert()
        .success();

    let content = fs::read_to_string(&note_path).unwrap();
    let drawer_idx = content
        .find(":PROPERTIES:\n:STATUS: active\n:END:")
        .expect("properties drawer must be present");
    let title_idx = content
        .find("#+TITLE: My Note")
        .expect("title must be present");

    assert!(
        drawer_idx < title_idx,
        ":PROPERTIES: drawer must appear BEFORE #+TITLE:. Content:\n{content}"
    );

    // Verify it parses back as a level-0 file property in org query.
    let assert_query = org()
        .args([
            "query",
            "search",
            "property:STATUS=active",
            "--format",
            "json",
        ])
        .arg(dir.path())
        .assert()
        .success();

    let query_val: Value =
        serde_json::from_slice(&assert_query.get_output().stdout).expect("valid JSON");
    let results = query_val.as_array().expect("array of results");
    assert_eq!(results.len(), 1, "must find exactly 1 matching file node");
    assert_eq!(results[0]["level"], 0, "matched node must be level 0");
    assert_eq!(results[0]["properties"]["STATUS"], "active");

    // Verify fmt check passes.
    org()
        .current_dir(dir.path())
        .args(["fmt", "check", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

// endregion
