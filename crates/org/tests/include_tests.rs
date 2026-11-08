// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural CLI and lint integration tests for `#+INCLUDE:` expansion and `E003` (`ORGFMT-WP-20`).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

// region Helpers

/// Sample UUID used for cross-file `#+INCLUDE:` heading locator tests.
const CHILD_SECTION_UUID: &str = "22222222-3333-4444-5555-666666666666";

/// Sample UUID used for parent root heading resolution in `org show`.
const PARENT_ROOT_UUID: &str = "77777777-8888-9999-aaaa-bbbbbbbbbbbb";

/// Constructs an `assert_cmd` runner for the workspace `org` binary.
fn org() -> Command {
    Command::cargo_bin("org").expect("org binary must compile")
}

/// Writes UTF-8 content to `path`, creating parent directories when needed.
fn write_org(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, content).expect("write org fixture");
}

// endregion

// region Slice 1: Cross-File #+INCLUDE: Expansion in org show & org export md

/// Verifies `org show --expand-includes` and `org export md --expand-includes` with `::*Section`, `:minlevel 2`, and `:lines "1-3"`.
#[test]
fn show_and_export_md_expand_includes_with_section_minlevel_and_lines() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    write_org(
        &dir.path().join("child.org"),
        "#+TITLE: Child Doc\n* Intro\nIgnored intro.\n* Section\nSection line 1.\n** Nested Child\nNested body.\n* Trailing\nIgnored trailing.\n",
    );
    write_org(
        &dir.path().join("snippet.org"),
        "Line one.\nLine two.\nLine three.\nLine four.\nLine five.\n",
    );
    let parent = dir.path().join("parent.org");
    write_org(
        &parent,
        &format!("* Root Heading\n:PROPERTIES:\n:ID: {PARENT_ROOT_UUID}\n:END:\nLead paragraph.\n#+INCLUDE: \"child.org::*Section\" :minlevel 2\n#+include: \"snippet.org\" :lines \"1-3\"\n"),
    );

    let show_bytes = org()
        .args([
            "show",
            &format!("id:{PARENT_ROOT_UUID}"),
            "--workspace",
            ws,
            "--expand-includes",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown = String::from_utf8(show_bytes).unwrap();
    assert!(shown.contains("** Section\nSection line 1.\n*** Nested Child\nNested body.\n"));
    assert!(shown.contains("Line one.\nLine two.\nLine three.\n") && !shown.contains("Line four."));
    assert!(
        !shown.contains("Ignored intro.")
            && !shown.contains("Ignored trailing.")
            && !shown.contains("#+INCLUDE:")
    );

    let md_bytes = org()
        .args([
            "export",
            "md",
            parent.to_str().unwrap(),
            "--workspace",
            ws,
            "--expand-includes",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(md_bytes).unwrap();
    assert!(
        md.contains("# Root Heading\n")
            && md.contains("## Section\n")
            && md.contains("### Nested Child\n")
    );
    assert!(md.contains("Line one.\nLine two.\nLine three.\n") && !md.contains("Line four."));
}

/// Verifies `:only-contents t`, `::#custom-id`, `::id:<uuid>`, and recursive multi-file `#+INCLUDE:` expansion.
#[test]
fn expand_includes_supports_only_contents_custom_id_uuid_and_nested_includes() {
    let dir = TempDir::new().unwrap();
    let leaf = dir.path().join("leaf.org");
    write_org(
        &leaf,
        &format!(
            "* Leaf Target\n:PROPERTIES:\n:ID: {CHILD_SECTION_UUID}\n:CUSTOM_ID: leaf-cid\n:END:\nLeaf body A.\nLeaf body B.\n"
        ),
    );

    let mid = dir.path().join("mid.org");
    write_org(
        &mid,
        "* Mid Section\n:PROPERTIES:\n:CUSTOM_ID: mid-sec\n:END:\nMid intro.\n#+INCLUDE: \"leaf.org::#leaf-cid\" :only-contents t\n",
    );

    let top = dir.path().join("top.org");
    write_org(
        &top,
        &format!(
            "* Top Root\n:PROPERTIES:\n:ID: {PARENT_ROOT_UUID}\n:END:\n#+INCLUDE: \"mid.org::#mid-sec\" :only-contents t\n#+INCLUDE: \"leaf.org::id:{CHILD_SECTION_UUID}\" :minlevel 2\n"
        ),
    );

    let json_bytes = org()
        .args([
            "show",
            &format!("id:{PARENT_ROOT_UUID}"),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--expand-includes",
            "--format",
            "json",
            "--include-markdown",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let val: Value = serde_json::from_slice(&json_bytes).unwrap();
    let content = val["content"].as_str().unwrap();
    assert!(content.contains("Mid intro.\n"));
    assert!(!content.contains("* Mid Section"));
    assert!(content.contains("Leaf body A.\nLeaf body B.\n"));
    assert!(content.contains("** Leaf Target\n"));
    let md = val["markdown"].as_str().unwrap();
    assert!(md.contains("## Leaf Target\n"));
    assert!(md.contains("Leaf body A."));
}

// endregion

// region Slice 2: Cycle & Path Traversal Rejection (EXIT_ERROR = 2)

/// Verifies recursive include cycles (`a.org` -> `b.org` -> `a.org`) fail with `EXIT_ERROR` (`2`).
#[test]
fn expand_includes_rejects_recursive_cycles_with_exit_code_2() {
    let dir = TempDir::new().unwrap();
    let a_file = dir.path().join("a.org");
    let b_file = dir.path().join("b.org");
    write_org(
        &a_file,
        &format!("* Node A\n:PROPERTIES:\n:ID: {PARENT_ROOT_UUID}\n:END:\n#+INCLUDE: \"b.org\"\n"),
    );
    write_org(&b_file, "* Node B\n#+INCLUDE: \"a.org\"\n");

    org()
        .args([
            "show",
            &format!("id:{PARENT_ROOT_UUID}"),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--expand-includes",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cycle"));

    org()
        .args([
            "export",
            "md",
            a_file.to_str().unwrap(),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--expand-includes",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cycle"));
}

/// Verifies path traversal outside the workspace (`../outside.org`) and missing targets return `EXIT_ERROR` (`2`).
#[test]
fn expand_includes_rejects_path_traversal_and_missing_targets_with_exit_code_2() {
    let outer = TempDir::new().unwrap();
    write_org(
        &outer.path().join("outside.org"),
        "* Secret\nOutside workspace.\n",
    );
    let ws = outer.path().join("workspace");
    let ws_s = ws.to_str().unwrap();
    let main_file = ws.join("main.org");
    write_org(
        &main_file,
        &format!(
            "* Main\n:PROPERTIES:\n:ID: {PARENT_ROOT_UUID}\n:END:\n#+INCLUDE: \"../outside.org\"\n"
        ),
    );

    org()
        .args([
            "show",
            &format!("id:{PARENT_ROOT_UUID}"),
            "--workspace",
            ws_s,
            "--expand-includes",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("outside.org"));
    org()
        .args([
            "export",
            "md",
            main_file.to_str().unwrap(),
            "--workspace",
            ws_s,
            "--expand-includes",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("outside.org"));

    let unresolved = ws.join("unresolved.org");
    write_org(&ws.join("target.org"), "* Existing\nBody.\n");
    write_org(
        &unresolved,
        "* Unresolved\n#+INCLUDE: \"target.org::*NonExistentHeading\"\n",
    );
    org()
        .args([
            "export",
            "md",
            unresolved.to_str().unwrap(),
            "--workspace",
            ws_s,
            "--expand-includes",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("NonExistentHeading"));
}

// endregion

// region Slice 3: Lint Rule E003 (include-validity)

/// Verifies `org fmt check` emits `E003` (`include-validity`) on missing files, bad targets, traversal, and cycles.
#[test]
fn fmt_check_emits_e003_include_validity_on_broken_and_cyclic_includes() {
    let outer = TempDir::new().unwrap();
    write_org(&outer.path().join("outside.org"), "* Secret\n");
    let ws = outer.path().join("ws");
    write_org(
        &ws.join("valid.org"),
        "* Valid Section\n:PROPERTIES:\n:CUSTOM_ID: valid-cid\n:END:\nContent.\n",
    );

    let (cycle_a, cycle_b) = (ws.join("cycle_a.org"), ws.join("cycle_b.org"));
    write_org(&cycle_a, "* Cycle A\n\n#+INCLUDE: \"cycle_b.org\"\n");
    write_org(&cycle_b, "* Cycle B\n\n#+INCLUDE: \"cycle_a.org\"\n");

    let broken = ws.join("broken.org");
    write_org(
        &broken,
        concat!(
            "* Broken Includes\n\n",
            "#+INCLUDE: \"valid.org::*Valid Section\" :minlevel 2\n",
            "#+INCLUDE: \"missing_file.org\"\n",
            "#+INCLUDE: \"valid.org::*Missing Heading\"\n",
            "#+INCLUDE: \"../outside.org\"\n",
            "#+begin_src org\n#+INCLUDE: \"ignored_in_src_block.org\"\n#+end_src\n",
        ),
    );

    let broken_out = org()
        .args(["fmt", "check", "--format", "json", broken.to_str().unwrap()])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let broken_diags: Vec<Value> = serde_json::from_slice(&broken_out).unwrap();
    let e003_broken: Vec<&Value> = broken_diags
        .iter()
        .filter(|d| d["rule_id"] == "E003" && d["rule"] == "include-validity")
        .collect();
    assert_eq!(e003_broken.len(), 3, "got: {broken_diags:?}");
    assert_eq!(
        (
            e003_broken[0]["line"].as_u64(),
            e003_broken[1]["line"].as_u64(),
            e003_broken[2]["line"].as_u64()
        ),
        (Some(4), Some(5), Some(6))
    );

    let cycle_out = org()
        .args([
            "fmt",
            "check",
            "--format",
            "json",
            cycle_a.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let cycle_diags: Vec<Value> = serde_json::from_slice(&cycle_out).unwrap();
    let e003_cycle: Vec<&Value> = cycle_diags
        .iter()
        .filter(|d| d["rule_id"] == "E003" && d["rule"] == "include-validity")
        .collect();
    assert_eq!(e003_cycle.len(), 1, "got: {cycle_diags:?}");
    assert!(e003_cycle[0]["message"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("cycle"));
}

/// Verifies Wave 3 documentation across `README.org` and `docs/` is synchronized and passes `org fmt check`.
#[test]
fn wave3_docs_are_synchronized_and_fmt_check_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let readme = fs::read_to_string(root.join("README.org")).unwrap();
    let usage = fs::read_to_string(root.join("docs/user/usage.org")).unwrap();
    let vision = fs::read_to_string(root.join("docs/product/vision.org")).unwrap();
    let arch = fs::read_to_string(root.join("docs/technical/architecture.org")).unwrap();

    for token in [
        "LOGBOOK",
        "LAST_REPEAT",
        "org-habit",
        "org clock in",
        "EFFORT",
        "org refile",
        "org query columns",
        "--expand-includes",
        "include-validity",
    ] {
        assert!(
            readme.contains(token) && usage.contains(token),
            "README.org and docs/user/usage.org must document '{token}'"
        );
    }
    assert!(vision.contains("--expand-includes") && vision.contains("include-validity"));
    assert!(arch.contains("include.rs") && arch.contains("--expand-includes"));

    org()
        .current_dir(root)
        .args(["fmt", "check", "README.org", "docs/"])
        .assert()
        .success();
}

// endregion
