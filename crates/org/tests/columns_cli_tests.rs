// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! CLI integration tests for `org query columns` (`ORGFMT-WP-19` / `SPEC-ORGFMT-04`).

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use tempfile::TempDir;

/// Builds a `Command` targeting the compiled `org` binary.
fn org_cmd() -> Command {
    Command::cargo_bin("org").expect("org binary should build")
}

/// Writes `content` to `filename` inside a fresh `TempDir` and returns `(dir, file_path)`.
fn write_fixture(filename: &str, content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir should be created");
    let file = dir.path().join(filename);
    fs::write(&file, content).expect("fixture should be written");
    (dir, file)
}

/// Verifies `#+COLUMNS:` parsing (`%25ITEM %TODO %EFFORT(Effort){:} %CLOCKSUM %POINTS{+}`)
/// and bottom-up `{:}` duration, `{+}` numeric, and `CLOCKSUM` rollups from children to parent.
#[test]
fn columns_parses_in_buffer_spec_and_rolls_up_duration_numeric_and_clocksum() {
    let content = concat!(
        "#+COLUMNS: %25ITEM %TODO %10EFFORT(Effort){:} %CLOCKSUM %POINTS{+}\n",
        "* TODO Roadmap Project :work:roadmap:\n",
        "SCHEDULED: <2026-03-10 Tue> DEADLINE: <2026-03-20 Fri>\n",
        "** DONE [#A] Backend API\n",
        ":PROPERTIES:\n",
        ":ID: backend-1\n",
        ":EFFORT: 0:45\n",
        ":POINTS: 5\n",
        ":END:\n",
        ":LOGBOOK:\n",
        "CLOCK: [2026-03-01 Sun 09:00]--[2026-03-01 Sun 09:45] =>  0:45\n",
        ":END:\n",
        "** TODO [#B] Frontend UI\n",
        ":PROPERTIES:\n",
        ":ID: frontend-2\n",
        ":EFFORT: 1:30\n",
        ":POINTS: 8.5\n",
        ":END:\n",
        ":LOGBOOK:\n",
        "CLOCK: [2026-03-02 Mon 10:00]--[2026-03-02 Mon 11:30] =>  1:30\n",
        ":END:\n",
    );
    let (_dir, file) = write_fixture("roadmap.org", content);

    let output = org_cmd()
        .args([
            "query",
            "columns",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON output");
    let cols = json["columns"].as_array().expect("columns array");
    assert_eq!(cols.len(), 5);
    assert_eq!(cols[0]["property"], "ITEM");
    assert_eq!(cols[0]["width"], 25);
    assert_eq!(cols[2]["property"], "EFFORT");
    assert_eq!(cols[2]["title"], "Effort");
    assert_eq!(cols[2]["width"], 10);
    assert_eq!(cols[2]["summary"], ":");
    assert_eq!(cols[3]["property"], "CLOCKSUM");
    assert_eq!(cols[4]["property"], "POINTS");
    assert_eq!(cols[4]["summary"], "+");

    let rows = json["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["level"], 1);
    assert_eq!(rows[0]["values"]["ITEM"], "Roadmap Project");
    assert_eq!(rows[0]["values"]["TODO"], "TODO");
    assert_eq!(rows[0]["values"]["EFFORT"], "2:15");
    assert_eq!(rows[0]["values"]["CLOCKSUM"], "2:15");
    assert_eq!(rows[0]["values"]["POINTS"], "13.5");

    assert_eq!(rows[1]["locator"], "id:backend-1");
    assert_eq!(rows[1]["level"], 2);
    assert_eq!(rows[1]["values"]["EFFORT"], "0:45");
    assert_eq!(rows[1]["values"]["CLOCKSUM"], "0:45");
    assert_eq!(rows[1]["values"]["POINTS"], "5");
}

/// Verifies entry-level `:COLUMNS:` property overriding file-level `#+COLUMNS:` when scoped via `--locator`.
#[test]
fn entry_level_columns_property_overrides_file_columns_with_locator() {
    let content = concat!(
        "#+COLUMNS: %25ITEM %TODO\n",
        "* Unrelated Section\n",
        "* TODO Scoped Subtree\n",
        ":PROPERTIES:\n",
        ":ID: subtree-root\n",
        ":COLUMNS: %20ITEM(Subtask) %COST{+} %EFFORT{:}\n",
        ":END:\n",
        "** DONE Part One\n",
        ":PROPERTIES:\n",
        ":COST: 10\n",
        ":EFFORT: 1h\n",
        ":END:\n",
        "** TODO Part Two\n",
        ":PROPERTIES:\n",
        ":COST: 5\n",
        ":EFFORT: 30m\n",
        ":END:\n",
    );
    let (_dir, file) = write_fixture("scoped.org", content);

    let output = org_cmd()
        .args([
            "query",
            "columns",
            file.to_str().unwrap(),
            "--locator",
            "id:subtree-root",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    let cols = json["columns"].as_array().expect("columns array");
    assert_eq!(cols.len(), 3);
    assert_eq!(cols[0]["property"], "ITEM");
    assert_eq!(cols[0]["title"], "Subtask");
    assert_eq!(cols[1]["property"], "COST");
    assert_eq!(cols[1]["summary"], "+");
    assert_eq!(cols[2]["property"], "EFFORT");
    assert_eq!(cols[2]["summary"], ":");

    let rows = json["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["values"]["ITEM"], "Scoped Subtree");
    assert_eq!(rows[0]["values"]["COST"], "15");
    assert_eq!(rows[0]["values"]["EFFORT"], "1:30");
}

/// Verifies `{X}` completion rollup (`[done/total]`) from child TODO states and entry checklists.
#[test]
fn checkbox_summary_type_rolls_up_todo_children_and_checklists() {
    let content = concat!(
        "#+COLUMNS: %25ITEM %TODO %PROGRESS(Progress){X}\n",
        "* TODO Release Milestone\n",
        "** DONE Write spec\n",
        "** DONE Implement parser\n",
        "** TODO Run verification\n",
        "- [X] Unit tests\n",
        "- [X] Integration tests\n",
        "- [ ] Release notes\n",
    );
    let (_dir, file) = write_fixture("progress.org", content);

    let output = org_cmd()
        .args([
            "query",
            "columns",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    let rows = json["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["values"]["ITEM"], "Release Milestone");
    assert_eq!(rows[0]["values"]["PROGRESS"], "[2/3]");
    assert_eq!(rows[3]["values"]["ITEM"], "Run verification");
    assert_eq!(rows[3]["values"]["PROGRESS"], "[2/3]");
}

/// Verifies `--format human`, `--format org`, `--format md`, default fallback, and exit codes.
#[test]
fn formats_human_org_md_default_spec_and_exit_codes() {
    let content = concat!(
        "* TODO [#A] Alpha Task :dev:urgent:\n",
        "SCHEDULED: <2026-04-01 Wed> DEADLINE: <2026-04-05 Sun>\n",
        "** DONE Beta Child\n",
    );
    let (_dir, file) = write_fixture("formats.org", content);

    let human = String::from_utf8(
        org_cmd()
            .args([
                "query",
                "columns",
                file.to_str().unwrap(),
                "--format",
                "human",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(human.contains("ITEM") && human.contains("PRIORITY") && human.contains("TAGS"));
    assert!(human.contains("Alpha Task") && human.contains(":dev:urgent:"));

    let org_out = String::from_utf8(
        org_cmd()
            .args([
                "query",
                "columns",
                file.to_str().unwrap(),
                "--columns",
                "%20ITEM(Task) %TODO %SCHEDULED %DEADLINE",
                "--format",
                "org",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(org_out.starts_with("| Task"));
    assert!(org_out.contains("|---") && org_out.contains("+---"));
    assert!(org_out.contains("<2026-04-01 Wed>") && org_out.contains("<2026-04-05 Sun>"));

    let md_out = String::from_utf8(
        org_cmd()
            .args(["query", "columns", file.to_str().unwrap(), "--format", "md"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(md_out.starts_with("| ITEM"));
    assert!(md_out.lines().nth(1).unwrap().contains("| ---"));
    assert!(md_out.contains("Alpha Task") && md_out.contains("Beta Child"));

    // Unresolvable locator returns EXIT_ERROR (2).
    org_cmd()
        .args([
            "query",
            "columns",
            file.to_str().unwrap(),
            "--locator",
            "id:does-not-exist",
        ])
        .assert()
        .code(2);

    // Empty .org file with no headings returns EXIT_ISSUES (1).
    let (_empty_dir, empty_file) = write_fixture("empty.org", "#+TITLE: Empty\n");
    org_cmd()
        .args(["query", "columns", empty_file.to_str().unwrap()])
        .assert()
        .code(1);
}

/// Verifies `org_tools_core::columns` format compilation, fallback defaults, and duration helpers.
#[test]
fn core_columns_parser_and_duration_helpers() {
    use org_tools_core::columns::{
        format_duration_hmm, parse_columns_format, parse_duration_minutes, SummaryType,
    };

    let specs = parse_columns_format("%25ITEM(Task Title) %10EFFORT(Effort){:} %COST{+} %DONE{X}");
    assert_eq!(specs.len(), 4);
    assert_eq!(specs[0].property, "ITEM");
    assert_eq!(specs[0].title, "Task Title");
    assert_eq!(specs[0].width, Some(25));
    assert_eq!(specs[1].summary, Some(SummaryType::DurationSum));
    assert_eq!(specs[2].summary, Some(SummaryType::NumericSum));
    assert_eq!(specs[3].summary, Some(SummaryType::Checkbox));

    let fallback = parse_columns_format("");
    assert_eq!(fallback.len(), 4);
    assert_eq!(fallback[0].property, "ITEM");
    assert_eq!(parse_duration_minutes("1h 30m"), Some(90));
    assert_eq!(format_duration_hmm(135), "2:15");
}
