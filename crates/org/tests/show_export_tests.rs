// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=710 reason="Comprehensive end-to-end CLI and schema contract test suite for org show and org export md"

//! Behavioural CLI and schema contract tests for `org show` and `org export md` (`ORGFMT-WP-05`).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use org_tools_core::files::join_slash_rel;
use predicates::prelude::*;
use tempfile::TempDir;

// region Constants & Path Helpers

/// Sample UUID used for level-0 file node resolution tests.
const FILE_NODE_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// Sample UUID used for heading node resolution tests.
const HEADING_NODE_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// Sample UUID used for cross-file target heading link tests.
const TARGET_HEADING_UUID: &str = "99999999-8888-7777-6666-555555555555";

/// Constructs an `assert_cmd` runner for the workspace `org` binary.
fn org() -> Command {
    Command::cargo_bin("org").expect("org binary must compile")
}

/// Writes UTF-8 content to `path`, creating parent directories when needed.
fn write_org(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, content).expect("write org fixture");
}

/// Runs `org` with `args`, asserts exit code `0`, and returns UTF-8 stdout.
fn run_org_stdout(args: &[&str]) -> String {
    let bytes = org()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(bytes).expect("valid UTF-8 stdout")
}

/// Loads `org schema <name>` and returns `(required_keys, allowed_property_keys)`.
fn load_schema_keys(name: &str, items_array: bool) -> (Vec<String>, Vec<String>) {
    let raw = run_org_stdout(&["schema", name]);
    let root: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let target = if items_array { &root["items"] } else { &root };
    assert_eq!(target["additionalProperties"].as_bool(), Some(false));
    let props = target["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let req = target["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    (req, props)
}

/// Asserts that `obj` contains all `required` keys and zero keys outside `allowed`.
fn assert_strict_object_keys(
    obj: &serde_json::Map<String, serde_json::Value>,
    required: &[&str],
    allowed: &[&str],
) {
    let allowed_set: BTreeSet<&str> = allowed.iter().copied().collect();
    for &req in required {
        assert!(obj.contains_key(req), "missing required JSON key '{req}'");
    }
    for key in obj.keys() {
        assert!(
            allowed_set.contains(key.as_str()),
            "unexpected JSON property '{key}' violates additionalProperties: false"
        );
    }
}

// endregion

// region Slice 1: Scoped org show Subtree Extractor

/// Verifies `org show` resolves `id:`, `#custom-id`, `*/Outline/Path`, and `::line` locators.
#[test]
fn show_resolves_heading_by_id_custom_id_outline_and_line() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("plan.org");
    write_org(
        &file,
        &format!(
            "* Intro\nIntro text.\n* TODO [#A] Target Feature :core:\nSCHEDULED: <2026-05-01 Fri>\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:CUSTOM_ID: target-feature\n:END:\nParent prose.\n** Subtask One\nChild prose.\n* Trailing Sibling\nOutside subtree.\n"
        ),
    );

    let locators = [
        format!("id:{HEADING_NODE_UUID}"),
        format!("{}::#target-feature", file.display()),
        format!("{}::*/Target Feature", file.display()),
        format!("{}::3", file.display()),
    ];

    for loc in &locators {
        let out = org()
            .args(["show", loc, "--workspace", dir.path().to_str().unwrap()])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("* TODO [#A] Target Feature :core:\n"));
        assert!(text.contains("Parent prose.\n** Subtask One\nChild prose.\n"));
        assert!(!text.contains("Intro text."));
        assert!(!text.contains("Trailing Sibling"));
    }
}

/// Verifies `--no-children` stops right before the first child heading `level > entry.level`.
#[test]
fn show_no_children_stops_before_first_child_heading() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tree.org");
    write_org(
        &file,
        &format!(
            "* Parent\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:END:\nOwn body line 1.\nOwn body line 2.\n** Child Heading\nChild body.\n*** Grandchild\n* Next Root\n"
        ),
    );

    let out = org()
        .args([
            "show",
            &format!("id:{HEADING_NODE_UUID}"),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--no-children",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("* Parent\n"));
    assert!(text.contains("Own body line 2.\n"));
    assert!(!text.contains("** Child Heading"));
    assert!(!text.contains("Grandchild"));
}

/// Verifies `org show` resolves level-0 `org-roam` v2 file nodes with and without `--no-children`.
#[test]
fn show_resolves_level_zero_file_node_with_and_without_children() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("roam_note.org");
    let raw = format!(
        ":PROPERTIES:\n:ID: {FILE_NODE_UUID}\n:END:\n#+TITLE: Roam Architecture\n\nPreamble overview paragraph.\n\n* First Section\nSection body.\n"
    );
    write_org(&file, &raw);

    let full = org()
        .args([
            "show",
            &format!("id:{FILE_NODE_UUID}"),
            "--workspace",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(full).unwrap(), raw);

    let preamble_only = org()
        .args([
            "show",
            &format!("id:{FILE_NODE_UUID}"),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--no-children",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let preamble_str = String::from_utf8(preamble_only).unwrap();
    assert!(preamble_str.contains("#+TITLE: Roam Architecture\n"));
    assert!(preamble_str.contains("Preamble overview paragraph.\n"));
    assert!(!preamble_str.contains("* First Section"));
}

/// Verifies `--max-chars` truncates cleanly on Unicode scalar boundaries without panicking.
#[test]
fn show_max_chars_truncates_utf8_multibyte_cleanly_without_panic() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("utf8.org");
    write_org(
        &file,
        &format!(
            "* Unicode Heading\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:END:\nGrüße 日本語 🚀🦀αβγδεζηθικλμνξοπρστυφχψω!\n"
        ),
    );

    for limit in [1usize, 7, 19, 35, 58, 61] {
        let out = org()
            .args([
                "show",
                &format!("id:{HEADING_NODE_UUID}"),
                "--workspace",
                dir.path().to_str().unwrap(),
                "--max-chars",
                &limit.to_string(),
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let rendered = String::from_utf8(out).expect("must remain valid UTF-8");
        assert!(rendered.chars().count() <= limit);
    }

    let json_bytes = org()
        .args([
            "show",
            &format!("id:{HEADING_NODE_UUID}"),
            "--workspace",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
            "--max-chars",
            "24",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let val: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
    assert_eq!(val["truncated"].as_bool(), Some(true));
    assert_eq!(val["char_count"].as_u64(), Some(24));
    assert_eq!(val["content"].as_str().unwrap().chars().count(), 24);
}

/// Verifies `org schema show` and `org show --format json` strictly match `org-show-output.schema.json`.
#[test]
fn show_json_envelope_and_schema_contract() {
    let (schema_req, schema_props) = load_schema_keys("show", false);
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("spec.org");
    write_org(
        &file,
        &format!(
            "* NEXT [#B] Build Exporter :export:md:\nDEADLINE: <2026-06-15 Mon 14:30>\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:EFFORT: 2h\n:END:\n- [X] Parse headings\n- [-] Align tables\n- [ ] Resolve IDs\n** Sub-item\n- [ ] Child checkbox\n"
        ),
    );

    let out = run_org_stdout(&[
        "show",
        &format!("id:{HEADING_NODE_UUID}"),
        "--workspace",
        dir.path().to_str().unwrap(),
        "--no-children",
        "--format",
        "json",
    ]);
    let val: serde_json::Value = serde_json::from_str(&out).unwrap();
    let req_refs: Vec<&str> = schema_req.iter().map(String::as_str).collect();
    let prop_refs: Vec<&str> = schema_props.iter().map(String::as_str).collect();
    assert_strict_object_keys(val.as_object().unwrap(), &req_refs, &prop_refs);

    assert_eq!(
        val["locator"].as_str(),
        Some(&*format!("id:{HEADING_NODE_UUID}"))
    );
    assert_eq!(val["line"].as_u64(), Some(1));
    assert_eq!(val["end_line"].as_u64(), Some(9));
    assert_eq!(val["level"].as_u64(), Some(1));
    assert_eq!(val["title"].as_str(), Some("Build Exporter"));
    assert_eq!(val["keyword"].as_str(), Some("NEXT"));
    assert_eq!(val["priority"].as_str(), Some("B"));
    assert_eq!(val["no_children"].as_bool(), Some(true));
    assert_eq!(val["truncated"].as_bool(), Some(false));
    assert_eq!(
        val["planning"]["deadline"].as_str(),
        Some("2026-06-15 14:30")
    );
    assert_eq!(val["checklist_summary"]["total"].as_u64(), Some(3));
    assert_eq!(val["checklist_summary"]["checked"].as_u64(), Some(1));
    assert_eq!(val["checklist_summary"]["partial"].as_u64(), Some(1));
    assert_eq!(val["checklist_summary"]["unchecked"].as_u64(), Some(1));
    assert_eq!(val["checklist"].as_array().unwrap().len(), 3);
    assert!(val["telemetry"]["files_scanned"].as_u64().unwrap() >= 1);
}

/// Verifies ancestor `.fritz.toml` workspace discovery and `EXIT_ERROR` (`2`) on bad/missing locators.
#[test]
fn show_ancestor_workspace_discovery_and_missing_locator_error() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".fritz.toml"),
        "[workspace]\nproject = \"demo\"\n",
    )
    .unwrap();
    write_org(
        &join_slash_rel(dir.path(), "docs/target.org"),
        &format!(
            "* Architecture\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:END:\nFound via root.\n"
        ),
    );
    let nested_cwd = join_slash_rel(dir.path(), "sub/work");
    fs::create_dir_all(&nested_cwd).unwrap();

    org()
        .current_dir(&nested_cwd)
        .args(["show", &format!("id:{HEADING_NODE_UUID}")])
        .assert()
        .success()
        .stdout(predicate::str::contains("Found via root."));

    org()
        .args([
            "show",
            "id:00000000-0000-0000-0000-000000000000",
            "--workspace",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .code(2);

    org().args(["show", "not-a-locator"]).assert().code(2);
}

// endregion

// region Slice 1b: Query JSON Checklist Output & Schema

/// Verifies `org query search --format json` exposes `checklist_summary` and `checklist` per schema.
#[test]
fn query_search_json_exposes_checklist_and_summary_matching_schema() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tasks.org");
    write_org(
        &file,
        "* TODO Ship WP-05\n- [X] Red tests\n- [-] Green implementation\n- [ ] Final gates\n",
    );

    let (req_keys, prop_keys) = load_schema_keys("query", true);
    let out = run_org_stdout(&[
        "query",
        "search",
        "todo:TODO",
        "--format",
        "json",
        file.to_str().unwrap(),
    ]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
    assert_eq!(arr.len(), 1);

    let req_refs: Vec<&str> = req_keys.iter().map(String::as_str).collect();
    let prop_refs: Vec<&str> = prop_keys.iter().map(String::as_str).collect();
    assert_strict_object_keys(arr[0].as_object().unwrap(), &req_refs, &prop_refs);
    assert_eq!(arr[0]["checklist_summary"]["total"].as_u64(), Some(3));
    assert_eq!(arr[0]["checklist_summary"]["checked"].as_u64(), Some(1));
    assert_eq!(arr[0]["checklist_summary"]["partial"].as_u64(), Some(1));
    assert_eq!(arr[0]["checklist_summary"]["unchecked"].as_u64(), Some(1));
    assert_eq!(arr[0]["checklist"][0]["state"].as_str(), Some("checked"));
    assert_eq!(arr[0]["checklist"][0]["text"].as_str(), Some("Red tests"));
}

// endregion

// region Slice 2: Pure-Rust Mermaid-Aware Markdown Exporter

/// Verifies drawer stripping, `#+TITLE:` rendering, tag padding cleanup, and GFM checklists.
#[test]
fn export_md_strips_drawers_renders_title_headings_and_checklists() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("guide.org");
    write_org(
        &file,
        &format!(
            ":PROPERTIES:\n:ID: {FILE_NODE_UUID}\n:END:\n#+TITLE: Operations Manual\n#+OPTIONS: toc:2\n\nIntroductory paragraph.\n\n* TODO [#A] Preflight Checks                                    :ops:prod:\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:END:\n:LOGBOOK:\nCLOCK: [2026-05-01 Fri 09:00]--[2026-05-01 Fri 09:30] =>  0:30\n:END:\n- [ ] Verify backups\n- [X] Check alerts\n- [-] Rotate keys\n"
        ),
    );

    let out = org()
        .args(["export", "md", file.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();

    assert!(!md.contains(":PROPERTIES:"));
    assert!(!md.contains(":LOGBOOK:"));
    assert!(!md.contains(FILE_NODE_UUID));
    assert!(!md.contains(HEADING_NODE_UUID));
    assert!(!md.contains("#+OPTIONS:"));
    assert!(md.contains("# Operations Manual\n"));
    assert!(md.contains("## TODO [#A] Preflight Checks :ops:prod:\n"));
    assert!(md.contains("- [ ] Verify backups\n"));
    assert!(md.contains("- [x] Check alerts\n"));
    assert!(md.contains("- [-] Rotate keys\n"));
}

/// Verifies `#+begin_src mermaid` and general code blocks fence cleanly with verbatim bodies.
#[test]
fn export_md_fences_mermaid_and_code_blocks_stripping_babel_headers() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("diagrams.org");
    write_org(
        &file,
        "* Flow\n#+begin_src mermaid :file flow.png :exports results\nflowchart LR\n  A[\"Start *raw*\"] --> B[\"/keep/ =literal=\"]\n#+end_src\n\n#+BEGIN_SRC rust :tangle src/lib.rs\nfn demo() { let x = \"*not bold* /not italic/\"; }\n#+END_SRC\n",
    );

    let out = org()
        .args(["export", "md", file.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();

    assert!(md.contains(
        "```mermaid\nflowchart LR\n  A[\"Start *raw*\"] --> B[\"/keep/ =literal=\"]\n```\n"
    ));
    assert!(!md.contains(":file flow.png"));
    assert!(md.contains("```rust\nfn demo() { let x = \"*not bold* /not italic/\"; }\n```\n"));
}

/// Verifies Org pipe tables convert to aligned GFM pipe tables with right-aligned numeric columns.
#[test]
fn export_md_aligns_gfm_pipe_tables_with_numeric_columns() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("metrics.org");
    write_org(
        &file,
        "| Service | Latency |Owner|\n|---+---+---|\n| =org-cli= | 12 |Alice|\n|---+---+---|\n| exporter | 4 |Bob|\n",
    );

    let out = org()
        .args(["export", "md", file.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = md.lines().filter(|l| l.starts_with('|')).collect();

    assert_eq!(
        lines.len(),
        4,
        "expected header + 1 separator + 2 data rows: {md}"
    );
    assert!(lines[0].contains("Service") && lines[0].contains("Latency"));
    assert!(
        lines[1].contains("---:"),
        "expected right-aligned numeric separator in: {}",
        lines[1]
    );
    assert!(lines[2].contains("`org-cli`"));
    let widths: BTreeSet<usize> = lines.iter().map(|l| l.len()).collect();
    assert_eq!(
        widths.len(),
        1,
        "all GFM table rows must be aligned to equal width:\n{md}"
    );
}

/// Verifies cross-node `[[id:...]]` links, local `.org` -> `.md` links, URLs, and inline emphasis.
#[test]
fn export_md_resolves_cross_node_id_links_and_inline_markup() {
    let dir = TempDir::new().unwrap();
    let other = dir.path().join("other.org");
    write_org(
        &other,
        &format!(
            ":PROPERTIES:\n:ID: {FILE_NODE_UUID}\n:END:\n#+TITLE: Target Architecture\n\n* My Heading\n:PROPERTIES:\n:ID: {TARGET_HEADING_UUID}\n:END:\nTarget details.\n"
        ),
    );
    let source = dir.path().join("index.org");
    write_org(
        &source,
        &format!(
            "* Overview\n:PROPERTIES:\n:ID: {HEADING_NODE_UUID}\n:END:\nRead [[id:{TARGET_HEADING_UUID}][the section]], [[id:{FILE_NODE_UUID}]], [[id:{HEADING_NODE_UUID}][self]], and [[id:deadbeef][unresolved]].\nVisit [[https://example.com/a/b][Example]] or [[file:guide.org][Guide]] with =inline *raw*=, ~verbatim~, *bold*, and /italic/.\n"
        ),
    );

    let out = org()
        .args([
            "export",
            "md",
            source.to_str().unwrap(),
            "--workspace",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();

    assert!(md.contains("[the section](other.md#my-heading)"));
    assert!(md.contains("[Target Architecture](other.md)"));
    assert!(md.contains("[self](#overview)"));
    assert!(md.contains("[unresolved](#id-deadbeef)"));
    assert!(md.contains("[Example](https://example.com/a/b)"));
    assert!(md.contains("[Guide](guide.md)"));
    assert!(md.contains("`inline *raw*`"));
    assert!(md.contains("`verbatim`"));
    assert!(md.contains("**bold**"));
    assert!(md.contains("*italic*"));
}

/// Verifies directory export mirroring (`-o <dir>`) and `org show --format md`.
#[test]
fn export_md_directory_output_mirrors_relative_paths_and_show_md_works() {
    let dir = TempDir::new().unwrap();
    let src_dir = dir.path().join("src_docs");
    let out_dir = dir.path().join("site_docs");
    write_org(
        &src_dir.join("index.org"),
        &format!("#+TITLE: Home\n\nSee [[id:{TARGET_HEADING_UUID}][Deep Guide]].\n"),
    );
    write_org(
        &join_slash_rel(&src_dir, "nested/deep.org"),
        &format!("* Deep Section\n:PROPERTIES:\n:ID: {TARGET_HEADING_UUID}\n:END:\nBody.\n"),
    );

    run_org_stdout(&[
        "export",
        "md",
        src_dir.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
    ]);

    let home_md = fs::read_to_string(out_dir.join("index.md")).expect("index.md created");
    let deep_md = fs::read_to_string(join_slash_rel(&out_dir, "nested/deep.md"))
        .expect("nested/deep.md created");
    assert!(home_md.contains("[Deep Guide](nested/deep.md#deep-section)"));
    assert!(deep_md.contains("# Deep Section\n"));

    let single_out = dir.path().join("single.md");
    run_org_stdout(&[
        "export",
        "md",
        join_slash_rel(&src_dir, "nested/deep.org")
            .to_str()
            .unwrap(),
        "-o",
        single_out.to_str().unwrap(),
    ]);
    assert!(single_out.is_file());

    let shown_md = run_org_stdout(&[
        "show",
        &format!("id:{TARGET_HEADING_UUID}"),
        "--workspace",
        src_dir.to_str().unwrap(),
        "--format",
        "md",
    ]);
    assert!(shown_md.contains("# Deep Section\n"));
    assert!(!shown_md.contains(":PROPERTIES:"));
}

/// Verifies level-0 file node JSON `--include-markdown` and `export md` empty-directory error.
#[test]
fn show_file_node_json_include_markdown_and_export_empty_dir_errors() {
    let dir = TempDir::new().unwrap();
    let note = dir.path().join("note.org");
    write_org(
        &note,
        &format!(
            ":PROPERTIES:\n:ID: {FILE_NODE_UUID}\n:END:\n#+TITLE: Root Note\n- [X] Top item\n\n* Child\n"
        ),
    );

    let out = org()
        .env("ORG_TOOLS_WORKSPACE", dir.path())
        .args([
            "show",
            &format!("id:{FILE_NODE_UUID}"),
            "--format",
            "json",
            "--no-children",
            "--include-markdown",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let val: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(val["level"].as_u64(), Some(0));
    assert_eq!(val["title"].as_str(), Some("Root Note"));
    assert_eq!(val["checklist_summary"]["checked"].as_u64(), Some(1));
    let md = val["markdown"].as_str().expect("markdown field included");
    assert!(md.contains("# Root Note\n"));
    assert!(md.contains("- [x] Top item\n"));

    let empty = TempDir::new().unwrap();
    org()
        .args(["export", "md", empty.path().to_str().unwrap()])
        .assert()
        .code(2);
}

/// Verifies cross-subdirectory `[[id:...]]` links and relative `.fritz.toml` workspace discovery.
#[test]
fn export_md_cross_subdir_links_and_relative_workspace_discovery() {
    let dir = TempDir::new().unwrap();
    write_org(&dir.path().join(".fritz.toml"), "[workspace]\n");
    write_org(
        &join_slash_rel(dir.path(), "specs/a.org"),
        &format!("* Spec\nSee [[id:{TARGET_HEADING_UUID}][Work Spec]].\n"),
    );
    write_org(
        &join_slash_rel(dir.path(), "work/b.org"),
        &format!("* Target Section\n:PROPERTIES:\n:ID: {TARGET_HEADING_UUID}\n:END:\nDetails.\n"),
    );

    org()
        .current_dir(dir.path())
        .args(["export", "md", ".", "-o", "out"])
        .assert()
        .success();
    let mirrored = fs::read_to_string(join_slash_rel(dir.path(), "out/specs/a.md")).unwrap();
    assert!(
        mirrored.contains("[Work Spec](../work/b.md#target-section)"),
        "expected ../work/b.md#target-section in mirrored export, got: {mirrored}"
    );

    org()
        .current_dir(dir.path())
        .args(["export", "md", "specs/a.org"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[Work Spec](../work/b.md#target-section)",
        ));

    org()
        .current_dir(dir.path())
        .args([
            "export",
            "md",
            "specs/a.org",
            "--workspace",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[Work Spec](../work/b.md#target-section)",
        ));
}

// endregion
