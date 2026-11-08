// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=1600 reason="Integration CLI test suite covering all top-level org subcommands"

//! End-to-end integration tests for the `org` command-line binary.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn org() -> Command {
    Command::cargo_bin("org").unwrap()
}

#[test]
fn check_clean_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("clean.org");
    fs::write(&file, "* Heading\n\nSome text.\n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();
}

#[test]
fn check_dirty_file_exits_1() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("dirty.org");
    fs::write(&file, "* Heading   \nSome text  \n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("trailing-whitespace"));
}

#[test]
fn format_in_place() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("format_me.org");
    fs::write(&file, "* Heading   \ntext  \n").unwrap();

    org()
        .args(["fmt", "format"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Formatted:"));

    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* Heading\ntext\n");
}

#[test]
fn format_check_mode() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("check_format.org");
    fs::write(&file, "* Heading   \n").unwrap();

    org()
        .args(["fmt", "format", "--check"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Would reformat"));

    // File should NOT be modified.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* Heading   \n");
}

#[test]
fn format_stdout_mode() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("stdout.org");
    fs::write(&file, "text   \n").unwrap();

    org()
        .args(["fmt", "format", "--stdout"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout("text\n");

    // File should NOT be modified.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "text   \n");
}

#[test]
fn check_json_output() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("json.org");
    fs::write(&file, "text   \n").unwrap();

    let output = org()
        .args(["fmt", "check", "--format", "json"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(parsed.is_array());
    assert!(!parsed.as_array().unwrap().is_empty());
}

#[test]
fn check_recursive_directory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("a.org"), "text   \n").unwrap();
    fs::write(sub.join("b.org"), "clean\n").unwrap();
    fs::write(sub.join("not_org.txt"), "text   \n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(dir.path().to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("a.org"))
        .stdout(predicate::str::contains("trailing-whitespace"));
}

#[test]
fn no_files_found_exits_2() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("not_org.txt"), "text\n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(dir.path().to_str().unwrap())
        .assert()
        .code(2);
}

#[test]
fn check_sample_fixture() {
    org()
        .args(["fmt", "check", "../../tests/fixtures/sample.org"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("heading-level-gap"))
        .stdout(predicate::str::contains("missing-src-language"))
        .stdout(predicate::str::contains("misplaced-property-drawer"));
}

#[test]
fn format_table_alignment() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("table.org");
    fs::write(
        &file,
        "| Name | Age |\n|---+---|\n| Alice | 30 |\n| Bob | 5 |\n",
    )
    .unwrap();

    org()
        .args(["fmt", "format", "--stdout"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout("| Name  | Age |\n|-------+-----|\n| Alice |  30 |\n| Bob   |   5 |\n");
}

#[test]
fn check_fix_mode() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("fix_me.org");
    fs::write(&file, "* Heading   \ntext  \n").unwrap();

    org()
        .args(["fmt", "check", "--fix"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Fixed:"));

    // File should be modified — trailing whitespace removed.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* Heading\ntext\n");
}

// --- update add-id tests ---

#[test]
fn add_id_to_all_entries() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ids.org");
    fs::write(&file, "* A\n* B\n* C\n").unwrap();

    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 3 IDs"));

    let content = fs::read_to_string(&file).unwrap();
    // All three headings should have :ID: properties.
    assert_eq!(content.matches(":ID:").count(), 3);
    assert_eq!(content.matches(":PROPERTIES:").count(), 3);
    assert_eq!(content.matches(":END:").count(), 3);
}

#[test]
fn add_id_idempotent() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("idem.org");
    fs::write(&file, "* A\n* B\n").unwrap();

    // First run adds IDs.
    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 2 IDs"));

    // Second run is a no-op.
    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("All entries already have IDs"));
}

#[test]
fn add_id_dry_run() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("dry.org");
    fs::write(&file, "* A\n* B\n").unwrap();

    org()
        .args(["update", "add-id", "--dry-run"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Would add 2 IDs"));

    // File should be unchanged.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* A\n* B\n");
}

#[test]
fn add_id_locator_single_entry() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("single.org");
    fs::write(&file, "* A\n* B\n* C\n").unwrap();

    let locator = format!("{}::*/B", file.display());
    org()
        .args(["update", "add-id", &locator])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 ID"));

    let content = fs::read_to_string(&file).unwrap();
    // Only B should have an ID.
    assert_eq!(content.matches(":ID:").count(), 1);
    assert!(content.contains("* A\n* B\n:PROPERTIES:"));
}

#[test]
fn add_id_locator_recursive() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tree.org");
    fs::write(&file, "* A\n** B\n*** C\n* D\n").unwrap();

    let locator = format!("{}::*/A", file.display());
    org()
        .args(["update", "add-id", "-r", &locator])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 3 IDs"));

    let content = fs::read_to_string(&file).unwrap();
    // A, B, C should have IDs; D should not.
    assert_eq!(content.matches(":ID:").count(), 3);
    // D should still be a plain heading.
    assert!(content.contains("* D\n") || content.ends_with("* D"));
}

#[test]
fn add_id_custom_format() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("custom.org");
    fs::write(&file, "* Hello World\n").unwrap();

    org()
        .args([
            "update",
            "add-id",
            "--id-format",
            "{file_stem}-{title_slug}-{level}",
        ])
        .arg(file.to_str().unwrap())
        .assert()
        .success();

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains(":ID: custom-hello-world-1\n"));
}

#[test]
fn add_id_existing_drawer() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("drawer.org");
    fs::write(&file, "* Task\n:PROPERTIES:\n:EFFORT: 1:00\n:END:\nBody\n").unwrap();

    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 ID"));

    let content = fs::read_to_string(&file).unwrap();
    // Should have one :PROPERTIES: block (not two).
    assert_eq!(content.matches(":PROPERTIES:").count(), 1);
    assert_eq!(content.matches(":END:").count(), 1);
    // :ID: should be inside the existing drawer, before :EFFORT:.
    let id_pos = content.find(":ID:").unwrap();
    let effort_pos = content.find(":EFFORT:").unwrap();
    assert!(id_pos < effort_pos);
}

// --- query search tests ---

#[test]
fn query_search_todo() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tasks.org");
    fs::write(
        &file,
        "* TODO Buy groceries\n* DONE Write report\n* TODO Fix bug\n",
    )
    .unwrap();

    org()
        .args(["query", "search", "todo:TODO"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Buy groceries"))
        .stdout(predicate::str::contains("Fix bug"));
}

#[test]
fn query_search_no_match() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tasks.org");
    fs::write(&file, "* DONE Finished task\n").unwrap();

    // No TODO entries match, so exit code is 1.
    org()
        .args(["query", "search", "todo:TODO"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1);
}

// --- query agenda tests ---

#[test]
fn query_agenda_scheduled() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("agenda.org");
    // Use today's date (computed the same way the CLI does) to guarantee
    // the scheduled item falls within the default 7-day window.
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let today_days = now_secs / 86400;
    // tomorrow = today + 1 day
    let tomorrow_days = today_days + 1;
    // Convert days-since-epoch to a date using the same algorithm as the CLI.
    let days_to_ymd = |z: i64| -> (i64, i64, i64) {
        let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as i64;
        let m = if mp < 10 {
            mp as i64 + 3
        } else {
            mp as i64 - 9
        };
        let y = if m <= 2 { y + 1 } else { y };
        (y, m, d)
    };
    let (y, m, d) = days_to_ymd(tomorrow_days);
    let date_str = format!("{y:04}-{m:02}-{d:02}");

    let content = format!("* TODO Meeting\nSCHEDULED: <{date_str}>\n",);
    fs::write(&file, &content).unwrap();

    org()
        .args(["query", "agenda"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Meeting"));
}

#[test]
fn query_agenda_empty() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("no_agenda.org");
    fs::write(&file, "* Just a heading\nSome body text.\n").unwrap();

    // No scheduled/deadline items means exit code 1.
    org()
        .args(["query", "agenda"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1);
}

// --- clock report tests ---

#[test]
fn clock_report_shows_time() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("clocked.org");
    fs::write(
        &file,
        concat!(
            "* Task with time\n",
            ":LOGBOOK:\n",
            "CLOCK: [2025-01-15 Wed 09:00]--[2025-01-15 Wed 11:30] =>  2:30\n",
            ":END:\n",
        ),
    )
    .unwrap();

    org()
        .args(["clock", "report"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Task with time"))
        .stdout(predicate::str::contains("2:30"));
}

#[test]
fn clock_report_json_output() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("clocked_json.org");
    fs::write(
        &file,
        concat!(
            "* Tracked task\n",
            ":LOGBOOK:\n",
            "CLOCK: [2025-01-15 Wed 09:00]--[2025-01-15 Wed 10:00] =>  1:00\n",
            ":END:\n",
        ),
    )
    .unwrap();

    let output = org()
        .args(["clock", "report", "--format", "json"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(parsed.is_object() || parsed.is_array());
}

#[test]
fn clock_report_empty() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("no_clocks.org");
    fs::write(&file, "* No clock entries\nJust text.\n").unwrap();

    // No clocked time results in exit code 1.
    org()
        .args(["clock", "report"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("No clocked time"));
}

// --- clock status tests ---

#[test]
fn clock_status_running() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("running.org");
    fs::write(
        &file,
        concat!(
            "* Active task\n",
            ":LOGBOOK:\n",
            "CLOCK: [2025-01-15 Wed 09:00]\n",
            ":END:\n",
        ),
    )
    .unwrap();

    org()
        .args(["clock", "status"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Active task"));
}

#[test]
fn clock_status_no_running() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("no_running.org");
    fs::write(
        &file,
        concat!(
            "* Finished task\n",
            ":LOGBOOK:\n",
            "CLOCK: [2025-01-15 Wed 09:00]--[2025-01-15 Wed 10:00] =>  1:00\n",
            ":END:\n",
        ),
    )
    .unwrap();

    // No running clocks means exit code 1.
    org()
        .args(["clock", "status"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("No running clocks"));
}

// --- export ical tests ---

#[test]
fn export_ical_vcalendar() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("cal.org");
    fs::write(&file, "* TODO Meeting\nSCHEDULED: <2025-03-15 Sat 14:00>\n").unwrap();

    org()
        .args(["export", "ical"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("BEGIN:VCALENDAR"))
        .stdout(predicate::str::contains("END:VCALENDAR"));
}

#[test]
fn export_ical_contains_event() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("event.org");
    fs::write(&file, "* Team standup\nSCHEDULED: <2025-06-01 Sun 09:30>\n").unwrap();

    org()
        .args(["export", "ical"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Team standup"));
}

// --- export jscal tests ---

#[test]
fn export_jscal_valid_json() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("jscal.org");
    fs::write(&file, "* TODO Review PR\nDEADLINE: <2025-04-01 Tue>\n").unwrap();

    let output = org()
        .args(["export", "jscal"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(parsed.is_array());
}

#[test]
fn export_jscal_contains_entry() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("jscal_entry.org");
    fs::write(
        &file,
        "* Sprint planning\nSCHEDULED: <2025-05-10 Sat 10:00>\n",
    )
    .unwrap();

    org()
        .args(["export", "jscal"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Sprint planning"));
}

// --- update set-state tests ---

#[test]
fn update_set_state_changes_keyword() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("state.org");
    fs::write(&file, "* TODO Task to finish\n").unwrap();

    org()
        .args(["update", "set-state", "DONE"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Changed"));

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("* DONE Task to finish"));
    assert!(!content.contains("* TODO Task to finish"));
}

#[test]
fn update_set_state_dry_run() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("state_dry.org");
    fs::write(&file, "* TODO Keep me\n").unwrap();

    org()
        .args(["update", "set-state", "DONE", "--dry-run"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Would change"));

    // File should be unchanged.
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("* TODO Keep me"));
}

// --- update add-todo tests ---

#[test]
fn update_add_todo_inserts_heading() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("add_todo.org");
    fs::write(&file, "* Existing heading\n").unwrap();

    org()
        .args(["update", "add-todo", "New task"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added entry"));

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("* TODO New task"));
}

#[test]
fn update_add_todo_dry_run() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("add_todo_dry.org");
    fs::write(&file, "* Existing\n").unwrap();

    org()
        .args(["update", "add-todo", "Dry run task", "--dry-run"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Would add entry"));

    // File should be unchanged.
    let content = fs::read_to_string(&file).unwrap();
    assert!(!content.contains("Dry run task"));
}

// --- update add-cookie tests ---

#[test]
fn update_add_cookie_recursive() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("cookies.org");
    fs::write(
        &file,
        concat!("* Project\n", "** TODO Task A\n", "** DONE Task B\n",),
    )
    .unwrap();

    org()
        .args(["update", "add-cookie", "--recursive"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();

    let content = fs::read_to_string(&file).unwrap();
    // The parent heading should now have a cookie.
    assert!(content.contains("[") && content.contains("]"));
}

// --- archive tests ---

#[test]
fn archive_dry_run_lists_entries() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("archive.org");
    fs::write(
        &file,
        concat!(
            "* DONE Completed task\n",
            "CLOSED: [2025-01-15 Wed 10:00]\n",
            "* TODO Active task\n",
        ),
    )
    .unwrap();

    org()
        .args(["archive", "--dry-run"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Completed task"));

    // File should be unchanged because of --dry-run.
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("DONE Completed task"));
    assert!(content.contains("TODO Active task"));
}

// --- completions tests ---

#[test]
fn completions_bash() {
    org()
        .args(["--completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("complete"))
        .stdout(predicate::str::contains("org"));
}

#[test]
fn completions_zsh() {
    org()
        .args(["--completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("org"));
}

// ---------------------------------------------------------------------------
// GTD subcommands
// ---------------------------------------------------------------------------

fn gtd_file() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("gtd.org");
    fs::write(
        &file,
        "\
#+TODO: TODO NEXT WAITING | DONE CANCELLED
* TODO Inbox item
* TODO Tagged task :@home:
* NEXT Office action :@office:work:
* NEXT Uncontexted action
* WAITING Vendor response :work:
:PROPERTIES:
:WAITING_FOR: John at Acme Corp
:END:
* TODO Blocked task :work:
:PROPERTIES:
:BLOCKER: ids(\"dep-1\")
:END:
* TODO Dependency
:PROPERTIES:
:ID: dep-1
:END:
* TODO Project with children
** DONE Phase 1
CLOSED: [2026-03-18 Tue 09:00]
** WAITING Phase 2
:PROPERTIES:
:WAITING_FOR: Vendor
:END:
* DONE Already done
CLOSED: [2026-03-20 Thu 14:00]
",
    )
    .unwrap();
    (dir, file)
}

#[test]
fn query_next_shows_actionable() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "next"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Office action"))
        .stdout(predicate::str::contains("Uncontexted action"));
}

#[test]
fn query_next_context_filter() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "next", "--context", "@office"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Office action"))
        .stdout(predicate::str::contains("Uncontexted").not());
}

#[test]
fn query_inbox_shows_untagged_unscheduled() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "inbox"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Inbox item"))
        .stdout(predicate::str::contains("Tagged task").not());
}

#[test]
fn query_waiting_shows_waiting_entries() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "waiting"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Vendor response"))
        .stdout(predicate::str::contains("Waiting for: John at Acme Corp"));
}

#[test]
fn query_blocked_shows_details() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "blocked"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Blocked task"))
        .stdout(predicate::str::contains("Blocked by:"))
        .stdout(predicate::str::contains("Dependency"));
}

#[test]
fn query_blocked_json_has_blocking_entries() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "blocked", "--format", "json"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("blocking_entries"))
        .stdout(predicate::str::contains("must be done"));
}

#[test]
fn query_stuck_shows_stuck_projects() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "stuck"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Project with children"))
        .stdout(predicate::str::contains("Children:"));
}

#[test]
fn query_next_json_output() {
    let (_dir, file) = gtd_file();
    org()
        .args(["query", "next", "--format", "json"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("\"title\""))
        .stdout(predicate::str::contains("Office action"));
}

#[test]
fn query_inbox_empty_exits_1() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("all_tagged.org");
    fs::write(&file, "* TODO Task :work:\n* DONE Done task\n").unwrap();
    org()
        .args(["query", "inbox"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1);
}

// ---------------------------------------------------------------------------
// Table formula calculation (org update calc)
// ---------------------------------------------------------------------------

#[test]
fn calc_simple_formula() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("calc.org");
    fs::write(
        &file,
        "\
| Item  | Price | Qty | Total |
|-------+-------+-----+-------|
| Apple |  1.50 |   3 |       |
| Pear  |  2.00 |   5 |       |
#+TBLFM: $4=$2*$3
",
    )
    .unwrap();

    org()
        .args(["update", "calc"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("update 2 cells"));

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("4.5"), "expected 4.5 in: {content}");
    assert!(content.contains("10"), "expected 10 in: {content}");
}

#[test]
fn calc_dry_run() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("calc_dry.org");
    let original = "\
| A |  B |
|---+----|
| 1 | 10 |
| 2 | 20 |
|---+----|
|   |    |
#+TBLFM: @>$2=vsum(@I$2..@II$2)
";
    fs::write(&file, original).unwrap();

    org()
        .args(["update", "calc", "--dry-run"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Would update"));

    // File should be unchanged.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, original);
}

#[test]
fn calc_elisp_exits_3() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("calc_elisp.org");
    fs::write(
        &file,
        "\
| A | B |
|---+---|
| 1 |   |
#+TBLFM: $2=lisp:(* $1 2)
",
    )
    .unwrap();

    org()
        .args(["update", "calc"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("requires Emacs"));
}

#[test]
fn calc_with_constants() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("calc_const.org");
    fs::write(
        &file,
        "\
#+CONSTANTS: pi=3.14159
| Radius | Area |
|--------+------|
|      5 |      |
#+TBLFM: $2=$1*$1*pi
",
    )
    .unwrap();

    org()
        .args(["update", "calc"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();

    let content = fs::read_to_string(&file).unwrap();
    // 5 * 5 * 3.14159 ≈ 78.54
    assert!(
        content.contains("78.54") || content.contains("78.5"),
        "expected ~78.54 in: {content}"
    );
}

// ---------------------------------------------------------------------------
// Schema output
// ---------------------------------------------------------------------------

#[test]
fn schema_query() {
    org()
        .args(["schema", "query"])
        .assert()
        .success()
        .stdout(predicate::str::contains("$schema"))
        .stdout(predicate::str::contains("org-query-output"));
}

#[test]
fn schema_diagnostic() {
    org()
        .args(["schema", "diagnostic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("severity"));
}

#[test]
fn schema_mutation() {
    org()
        .args(["schema", "mutation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("entries_affected"));
}

#[test]
fn schema_unknown_exits_2() {
    org()
        .args(["schema", "nonexistent"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown schema"));
}

#[test]
fn calc_json_output() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("calc_json.org");
    fs::write(
        &file,
        "\
| A | B |
|---+---|
| 1 | 2 |
#+TBLFM: $2=$1+1
",
    )
    .unwrap();

    org()
        .args(["update", "calc", "--format", "json"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("\"command\""))
        .stdout(predicate::str::contains("\"entries_affected\""));
}

// ---------------------------------------------------------------------------
// ORGFMT-WP-02: Uniform MutationResult JSON, Property/Tag Setters, Deterministic IDs
// ---------------------------------------------------------------------------

/// Validates that a JSON value conforms to `org-mutation-output.schema.json`.
fn assert_mutation_envelope(val: &serde_json::Value, expected_cmd: &str, expected_dry_run: bool) {
    assert_eq!(val["command"].as_str(), Some(expected_cmd));
    assert_eq!(val["dry_run"].as_bool(), Some(expected_dry_run));
    assert!(val["files_modified"].is_u64());
    assert!(val["entries_affected"].is_u64());
    assert!(val["errors"].is_array());
    let details = val["details"].as_array().expect("details must be array");
    for item in details {
        assert!(item["file"].is_string());
        assert!(item["line"].as_u64().unwrap_or(0) >= 1);
        assert!(item["action"].is_string());
        assert!(item["detail"].is_string() || item["detail"].is_null());
    }
    let telemetry = val["telemetry"]
        .as_object()
        .expect("telemetry object must be present");
    assert!(telemetry["duration_us"].is_u64());
    assert!(telemetry["files_scanned"].is_u64());
    assert_eq!(telemetry["cache_hits"].as_u64(), Some(0));
    assert_eq!(telemetry["cache_misses"].as_u64(), Some(0));
}

/// Runs `org` with `args` and parses stdout as JSON.
fn run_org_json(args: &[&str]) -> serde_json::Value {
    let out = org()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

/// Verifies `org update add-id --format json` emits a valid `MutationResult`.
#[test]
fn slice1_add_id_json_envelope() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ids.org");
    fs::write(&file, "* First\n* Second\n").unwrap();

    let json = run_org_json(&[
        "update",
        "add-id",
        "--format",
        "json",
        file.to_str().unwrap(),
    ]);
    assert_mutation_envelope(&json, "add-id", false);
    assert_eq!(json["files_modified"], 1);
    assert_eq!(json["entries_affected"], 2);
    assert_eq!(json["details"].as_array().unwrap().len(), 2);
}

/// Verifies `set-state`, `add-todo`, `add-cookie`, and `archive` emit `MutationResult` JSON.
#[test]
fn slice1_existing_mutators_json_envelope() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tasks.org");
    fs::write(&file, "* Project\n** TODO Child A\n** DONE Child B\n").unwrap();
    let path = file.to_str().unwrap();

    let cookie_json = run_org_json(&[
        "update",
        "add-cookie",
        "--recursive",
        "--format",
        "json",
        path,
    ]);
    assert_mutation_envelope(&cookie_json, "add-cookie", false);
    assert_eq!(cookie_json["entries_affected"], 1);

    let todo_json = run_org_json(&["update", "add-todo", "Added item", "--format", "json", path]);
    assert_mutation_envelope(&todo_json, "add-todo", false);
    assert_eq!(todo_json["entries_affected"], 1);

    let state_loc = format!("{}::*/Added item", file.display());
    let state_json = run_org_json(&[
        "update",
        "set-state",
        "DONE",
        &state_loc,
        "--format",
        "json",
    ]);
    assert_mutation_envelope(&state_json, "set-state", false);
    assert_eq!(state_json["entries_affected"], 1);

    let arch_json = run_org_json(&["archive", "--dry-run", "--format", "json", path]);
    assert_mutation_envelope(&arch_json, "archive", true);
    assert_eq!(arch_json["entries_affected"], 2);
}

/// Verifies `set-property` and `delete-property` align drawers (`F005`) and preserve body text.
#[test]
fn slice2_set_and_delete_property_cli() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("props.org");
    fs::write(
        &file,
        "* TODO Review spec\nSCHEDULED: <2026-11-08 Sun>\nKeep this body paragraph intact.\n",
    )
    .unwrap();
    let loc = format!("{}::*/Review spec", file.display());
    let path = file.to_str().unwrap();

    let j1 = run_org_json(&[
        "update",
        "set-property",
        &loc,
        "EFFORT",
        "1:30",
        "--format",
        "json",
    ]);
    assert_mutation_envelope(&j1, "set-property", false);
    assert_eq!(j1["entries_affected"], 1);

    org()
        .args(["update", "set-property", &loc, "CUSTOM_ID", "spec-review"])
        .assert()
        .success();
    let after_set = fs::read_to_string(&file).unwrap();
    assert!(after_set.contains(":EFFORT:    1:30\n:CUSTOM_ID: spec-review\n"));
    assert!(after_set.contains("Keep this body paragraph intact.\n"));
    org().args(["fmt", "check", path]).assert().success();

    let j2 = run_org_json(&[
        "update",
        "delete-property",
        &loc,
        "CUSTOM_ID",
        "--format",
        "json",
    ]);
    assert_mutation_envelope(&j2, "delete-property", false);
    assert_eq!(j2["entries_affected"], 1);

    let after_del = fs::read_to_string(&file).unwrap();
    assert!(after_del.contains(":PROPERTIES:\n:EFFORT: 1:30\n:END:\n"));
    org().args(["fmt", "check", path]).assert().success();
}

/// Verifies `set-tags` supports colon/comma/clear inputs and aligns to column 77 (`F007`).
#[test]
fn slice3_set_tags_cli_aligns_and_clears() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tags.org");
    fs::write(&file, "* TODO Ship release\n").unwrap();
    let loc = format!("{}::*/Ship release", file.display());

    let out = org()
        .args([
            "update",
            "set-tags",
            &loc,
            ":ops:release:",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_mutation_envelope(&json, "set-tags", false);
    assert_eq!(json["entries_affected"], 1);

    let with_tags = fs::read_to_string(&file).unwrap();
    assert_eq!(with_tags.trim_end().len(), 77);
    assert!(with_tags.trim_end().ends_with(":ops:release:"));
    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();

    org()
        .args(["update", "set-tags", &loc, "alpha,beta"])
        .assert()
        .success();
    let comma_tags = fs::read_to_string(&file).unwrap();
    assert!(comma_tags.trim_end().ends_with(":alpha:beta:"));
    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();

    org()
        .args(["update", "set-tags", &loc, "--clear"])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&file).unwrap(), "* TODO Ship release\n");
    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();
}

/// Verifies deterministic UUIDv5 golden vectors 1 and 2 via `org update add-id --deterministic`.
#[test]
fn slice4_deterministic_add_id_golden_vectors() {
    let dir = TempDir::new().unwrap();
    let f1 = dir.path().join("tasks.org");
    fs::write(
        &f1,
        "* TODO Execute task\n:PROPERTIES:\n:CUSTOM_ID: ORG-TASK-01\n:END:\n",
    )
    .unwrap();
    org()
        .args([
            "update",
            "add-id",
            "--deterministic",
            "--scope",
            "core",
            "--kind",
            "task",
            f1.to_str().unwrap(),
        ])
        .assert()
        .success();
    let c1 = fs::read_to_string(&f1).unwrap();
    assert!(c1.contains("eb32a5a1-a982-5f20-a264-1c0657b69171"));
    assert!(c1.contains("urn:org:core:task:org-task-01"));
    org()
        .args(["fmt", "check"])
        .arg(f1.to_str().unwrap())
        .assert()
        .success();

    let f2 = dir.path().join("issues.org");
    fs::write(&f2, "* ORGFMT-101\n").unwrap();
    org()
        .args([
            "update",
            "add-id",
            "--deterministic",
            "--scope",
            "orgfmt",
            "--kind",
            "issue",
            f2.to_str().unwrap(),
        ])
        .assert()
        .success();
    let c2 = fs::read_to_string(&f2).unwrap();
    assert!(c2.contains("e903bb85-3326-5094-8a57-f598003bf5c1"));
    assert!(c2.contains("urn:org:orgfmt:issue:orgfmt-101"));
}

/// Verifies author-scoped deterministic UUIDv5 golden vectors 3 & 4 and env var overrides.
#[test]
fn slice4_deterministic_author_vectors_and_env_overrides() {
    let dir = TempDir::new().unwrap();
    for (author, expected_id) in [
        ("alice", "e49367db-0dc6-5641-a0bc-963456dd0d7f"),
        ("bob", "b6efcd06-ea6b-5bfb-aecf-573171a84005"),
    ] {
        let f = dir.path().join(format!("note_{author}.org"));
        fs::write(&f, "* 20261106-cache-race\n").unwrap();
        let p = f.to_str().unwrap();
        let args = [
            "update",
            "add-id",
            "--deterministic",
            "--scope",
            "core",
            "--kind",
            "note",
            "--author",
            author,
            p,
        ];
        org().args(args).assert().success();
        assert!(fs::read_to_string(&f).unwrap().contains(expected_id));
    }

    let f5 = dir.path().join("env_override.org");
    fs::write(&f5, "* Custom Prefix Item\n").unwrap();
    let env_args = [
        "update",
        "add-id",
        "--deterministic",
        "--scope",
        "ops",
        "--kind",
        "runbook",
        f5.to_str().unwrap(),
    ];
    org()
        .env("ORG_ID_PREFIX", "urn:acme")
        .args(env_args)
        .assert()
        .success();
    assert!(fs::read_to_string(&f5)
        .unwrap()
        .contains("urn:acme:ops:runbook:custom-prefix-item"));
}

/// Verifies `add-todo --id auto` under `--parent` locator stays `org fmt check` clean.
#[test]
fn slice4_add_todo_auto_id_under_parent_fmt_clean() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("project.org");
    fs::write(&file, "* Active Epic\n").unwrap();
    let parent_loc = format!("{}::*/Active Epic", file.display());

    org()
        .args([
            "update",
            "add-todo",
            "Child Work Package",
            "--parent",
            &parent_loc,
            "--id",
            "auto",
            "--property",
            "EFFORT=1:00",
            "--body",
            "Child body paragraph.",
        ])
        .assert()
        .success();

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("** TODO Child Work Package"));
    assert!(content.contains(":ID:"));
    assert!(!content.contains(":ID_SEED:"));
    assert!(content.contains(":EFFORT: 1:00"));
    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();
}

/// Verifies `org update add-todo` with `--id deterministic`, `--property`, `--tags`, and `--body`.
#[test]
fn slice4_add_todo_deterministic_properties_and_body_fmt_clean() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("backlog.org");

    let out = org()
        .args([
            "update",
            "add-todo",
            "Support Deterministic IDs",
            file.to_str().unwrap(),
            "--tags",
            "core,identity",
            "--id",
            "deterministic",
            "--scope",
            "orgfmt",
            "--kind",
            "issue",
            "--property",
            "CUSTOM_ID=ORGFMT-101",
            "--property",
            "EFFORT=2:00",
            "--body",
            "Deterministic RFC 9562 UUIDv5 minting with :ID_SEED: provenance.",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_mutation_envelope(&json, "add-todo", false);
    assert_eq!(json["entries_affected"], 1);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("e903bb85-3326-5094-8a57-f598003bf5c1"));
    assert!(content.contains("urn:org:orgfmt:issue:orgfmt-101"));
    assert!(content.contains(":EFFORT:    2:00"));
    assert!(content.contains("Deterministic RFC 9562 UUIDv5 minting with :ID_SEED: provenance."));

    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();
}

/// Verifies file-level `:ID:` locators (`org-roam` v2) do not panic in `update` commands.
#[test]
fn file_node_id_locator_mutations_handle_sentinel_cleanly() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("roam-note.org");
    fs::write(
        &file,
        ":PROPERTIES:\n:ID: 11111111-2222-4333-8444-555555555555\n:END:\n#+TITLE: Roam Note\n",
    )
    .unwrap();

    // `add-todo` targeting a file-level `id:<uuid>` appends a level-1 heading cleanly.
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-todo",
            "Top-level item under roam note",
            "id:11111111-2222-4333-8444-555555555555",
        ])
        .assert()
        .success();
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("* TODO Top-level item under roam note"));

    // `add-id` without `--recursive` on a file-level node is a clean no-op (exit 0).
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "add-id",
            "id:11111111-2222-4333-8444-555555555555",
        ])
        .assert()
        .success();

    // `set-state` on a file-level node exits cleanly with EXIT_ERROR (2), never panicking.
    org()
        .current_dir(dir.path())
        .args([
            "update",
            "set-state",
            "DONE",
            "id:11111111-2222-4333-8444-555555555555",
        ])
        .assert()
        .code(2);
}

/// Verifies `org archive` leaves the source file intact and exits 2 when writing the archive fails.
#[test]
fn archive_failure_preserves_source_and_exits_2() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("source.org");
    let original = "* DONE Task to archive\nSome body text\n";
    fs::write(&file, original).unwrap();

    let bad_target = format!("{}/cannot_create_dir/archive.org", file.display());
    org()
        .args(["archive", "--target", &bad_target])
        .arg(file.to_str().unwrap())
        .assert()
        .code(2);

    assert_eq!(fs::read_to_string(&file).unwrap(), original);
}

/// Verifies malformed `.org-tools.toml` files cause `org` to report a parse error and exit 2.
#[test]
fn invalid_config_file_exits_2() {
    let dir = TempDir::new().unwrap();
    let config_file = dir.path().join(".org-tools.toml");
    let org_file = dir.path().join("test.org");
    fs::write(&config_file, "invalid toml = [[[").unwrap();
    fs::write(&org_file, "* TODO Test\n").unwrap();

    org()
        .current_dir(dir.path())
        .args(["fmt", "check"])
        .arg(org_file.to_str().unwrap())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("error parsing"));
}
