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
