// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural CLI integration tests for `ORGFMT-WP-14`: `org update set-scheduled`,
//! `org update set-deadline`, and repeater-aware `org update set-state`.

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use org_tools_core::cache::{MabuseCache, DEFAULT_CACHE_REL_PATH};
use org_tools_core::files::join_slash_rel;
use org_tools_core::planning::{day_of_week_abbrev, shift_calendar_date, TimeUnit};
use org_tools_core::state::now_timestamp;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

// region Constants & Test Helpers

/// Expected CLI exit code for operational and validation failures (`EXIT_ERROR`).
const EXIT_ERROR_CODE: i32 = 2;
/// Sample heading `:ID:` UUID used in locator mutation tests.
const HEADING_UUID: &str = "11111111-2222-4333-8444-555555555551";
/// Sample level-0 file `:ID:` UUID used in file-node rejection tests.
const FILE_NODE_UUID: &str = "f0000000-0000-4000-8000-000000000099";
/// Allowed top-level property keys in `schemas/org-mutation-output.schema.json`.
const ALLOWED_TOP_KEYS: &[&str] = &[
    "command",
    "files_modified",
    "entries_affected",
    "dry_run",
    "details",
    "errors",
    "telemetry",
];
/// Allowed item property keys inside `details[]` in `schemas/org-mutation-output.schema.json`.
const ALLOWED_DETAIL_KEYS: &[&str] = &["file", "line", "action", "detail"];

/// Builds an [`assert_cmd::Command`] targeting the compiled `org` binary.
fn org() -> Command {
    Command::from_std(StdCommand::new(env!("CARGO_BIN_EXE_org")))
}

/// Executes `org` with `args` and asserts exit status `0`.
fn run_ok(args: &[&str]) {
    org().args(args).assert().success();
}

/// Executes `org` inside `cwd` with `args` and asserts exit status `EXIT_ERROR_CODE` (`2`).
fn run_err_in(cwd: &Path, args: &[&str]) {
    org()
        .current_dir(cwd)
        .args(args)
        .assert()
        .code(EXIT_ERROR_CODE);
}

/// Writes `content` to `dir/rel_path`, creating parent directories automatically.
fn write_org(dir: &Path, rel_path: &str, content: &str) -> PathBuf {
    let path = join_slash_rel(dir, rel_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directories");
    }
    fs::write(&path, content).expect("write org test fixture");
    path
}

/// Runs `org` with `args`, asserts exit status `0`, and parses stdout as JSON.
fn run_org_json(args: &[&str]) -> Value {
    let out = org()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).expect("stdout must be valid JSON")
}

/// Opens and syncs [`MabuseCache`] at `root` and returns `cache_misses`.
fn sync_misses(root: &Path) -> usize {
    MabuseCache::open_and_sync(root).unwrap().1.cache_misses
}

/// Parses today's local `(year, month, day)` from [`now_timestamp`].
fn local_today() -> (u16, u8, u8) {
    let now = now_timestamp();
    let mut parts = now[..10].split('-');
    let y: u16 = parts.next().unwrap().parse().unwrap();
    let m: u8 = parts.next().unwrap().parse().unwrap();
    let d: u8 = parts.next().unwrap().parse().unwrap();
    (y, m, d)
}

/// Formats `(year, month, day)` as a canonical active Org timestamp `<YYYY-MM-DD Day>`.
fn fmt_active_date(ymd: (u16, u8, u8)) -> String {
    let dow = day_of_week_abbrev(ymd.0, ymd.1, ymd.2);
    format!("<{:04}-{:02}-{:02} {dow}>", ymd.0, ymd.1, ymd.2)
}

/// Asserts that `val` strictly conforms to `schemas/org-mutation-output.schema.json`.
fn assert_mutation_schema(val: &Value, expected_cmd: &str, expected_dry_run: bool) {
    let obj = val.as_object().expect("root must be a JSON object");
    assert!(obj.keys().all(|k| ALLOWED_TOP_KEYS.contains(&k.as_str())));
    assert_eq!(val["command"].as_str(), Some(expected_cmd));
    assert_eq!(val["dry_run"].as_bool(), Some(expected_dry_run));
    assert!(val["files_modified"].is_u64() && val["entries_affected"].is_u64());
    assert!(val["errors"].is_array());

    for item in val["details"].as_array().expect("details must be an array") {
        let d_obj = item.as_object().expect("detail item must be an object");
        assert!(d_obj
            .keys()
            .all(|k| ALLOWED_DETAIL_KEYS.contains(&k.as_str())));
        assert!(item["file"].is_string() && item["action"].is_string());
        assert!(item["line"].as_u64().unwrap_or(0) >= 1);
        assert!(item["detail"].is_string() || item["detail"].is_null());
    }
    let tel = val["telemetry"]
        .as_object()
        .expect("telemetry must be an object");
    for req in ["duration_us", "files_scanned", "cache_hits", "cache_misses"] {
        assert!(tel[req].is_u64(), "telemetry.{req} must be u64");
    }
}

// endregion

// region Slice 1: CLI set-scheduled & set-deadline Mutations

/// Verifies `set-scheduled` and `set-deadline` across line, heading, outline, and `id:` locators.
#[test]
fn slice1_set_scheduled_and_deadline_across_locators_and_specs() {
    let dir = TempDir::new().unwrap();
    let fixture = concat!(
        "* TODO First Entry\n",
        "* TODO Second Entry\n:PROPERTIES:\n:ID: 11111111-2222-4333-8444-555555555551\n:END:\n",
        "* TODO Third Entry\n",
    );
    let file = write_org(dir.path(), "tasks.org", fixture);
    let line_loc = format!("{}::1", file.display());
    let heading_loc = format!("{}::Third Entry", file.display());
    let id_loc = format!("id:{HEADING_UUID}");

    run_ok(&["update", "set-scheduled", "2026-04-20", &line_loc]);
    let full_ts = "<2026-04-20 Mon +1w -2d>";
    run_ok(&["update", "set-deadline", full_ts, &line_loc]);
    org()
        .current_dir(dir.path())
        .args(["update", "set-scheduled", "2026-04-20 09:30 +1w", &id_loc])
        .assert()
        .success();
    run_ok(&["update", "set-deadline", "2026-05-01", &heading_loc]);

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("SCHEDULED: <2026-04-20 Mon> DEADLINE: <2026-04-20 Mon +1w -2d>"));
    assert!(updated.contains("SCHEDULED: <2026-04-20 Mon 09:30 +1w>"));
    assert!(updated.contains("* TODO Third Entry\nDEADLINE: <2026-05-01 Fri>"));
    run_ok(&["fmt", "check", file.to_str().unwrap()]);
}

/// Verifies relative shifts (`+3d`, `+1w`, `+1m`, `+1y`, `-1d`), `today`, `.`, and `--clear`.
#[test]
fn slice1_relative_shifts_today_dot_negative_offset_and_clear() {
    let dir = TempDir::new().unwrap();
    let file = write_org(dir.path(), "shifts.org", "* TODO Shift Target\n");
    let loc = format!("{}::1", file.display());
    let (y, m, d) = local_today();

    let cases = [
        ("today", (y, m, d)),
        (".", (y, m, d)),
        ("+3d", shift_calendar_date(y, m, d, 3, TimeUnit::Day)),
        ("+1w", shift_calendar_date(y, m, d, 1, TimeUnit::Week)),
        ("+1m", shift_calendar_date(y, m, d, 1, TimeUnit::Month)),
        ("+1y", shift_calendar_date(y, m, d, 1, TimeUnit::Year)),
    ];
    for (spec, expected_ymd) in cases {
        run_ok(&["update", "set-scheduled", spec, &loc]);
        let expected = format!("SCHEDULED: {}", fmt_active_date(expected_ymd));
        assert!(fs::read_to_string(&file).unwrap().contains(&expected));
    }

    let minus_one = fmt_active_date(shift_calendar_date(y, m, d, -1, TimeUnit::Day));
    let dry_args = [
        "update",
        "set-deadline",
        "-1d",
        &loc,
        "--dry-run",
        "--format",
        "json",
    ];
    assert_mutation_schema(&run_org_json(&dry_args), "set-deadline", true);
    assert!(!fs::read_to_string(&file).unwrap().contains("DEADLINE:"));

    let neg = run_org_json(&["update", "set-scheduled", "-1d", &loc, "--format", "json"]);
    assert_mutation_schema(&neg, "set-scheduled", false);
    assert!(fs::read_to_string(&file)
        .unwrap()
        .contains(&format!("SCHEDULED: {minus_one}")));

    run_ok(&["update", "set-scheduled", "--clear", &loc]);
    assert!(!fs::read_to_string(&file).unwrap().contains("SCHEDULED:"));
    run_ok(&["update", "set-deadline", "2026-04-20", &loc]);
    run_ok(&["update", "set-deadline", &loc, "--clear"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), "* TODO Shift Target\n");
}

/// Verifies multi-entry file targets, `--stdin` locator targets, and `--dry-run` preservation.
#[test]
fn slice1_batch_file_stdin_targets_and_dry_run_preservation() {
    let dir = TempDir::new().unwrap();
    let initial = "* TODO Alpha\n* TODO Beta\n* TODO Gamma\n";
    let file = write_org(dir.path(), "batch.org", initial);
    let p = file.to_str().unwrap();

    run_ok(&["update", "set-scheduled", "2026-04-20", p, "--dry-run"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), initial);

    run_ok(&["update", "set-scheduled", "2026-04-20", p]);
    let after_file = fs::read_to_string(&file).unwrap();
    assert_eq!(after_file.matches("SCHEDULED: <2026-04-20 Mon>").count(), 3);

    let stdin_payload = format!("{}::*/Alpha\n{}::*/Gamma\n", file.display(), file.display());
    org()
        .args(["update", "set-deadline", "2026-04-25", "--stdin"])
        .write_stdin(stdin_payload)
        .assert()
        .success();
    let after_stdin = fs::read_to_string(&file).unwrap();
    assert_eq!(after_stdin.matches("DEADLINE: <2026-04-25 Sat>").count(), 2);
    assert!(!after_stdin.contains("* TODO Beta\nSCHEDULED: <2026-04-20 Mon> DEADLINE:"));
}

// endregion

// region Slice 2: Schema JSON, Warm Cache Invalidation & Error Exit Codes

/// Verifies `set-scheduled` and `set-deadline` emit schema-valid `MutationResult` JSON.
#[test]
fn slice2_json_envelope_matches_mutation_schema() {
    let dir = TempDir::new().unwrap();
    let file = write_org(dir.path(), "json.org", "* TODO One\n* TODO Two\n");
    let p = file.to_str().unwrap();

    let sched = run_org_json(&[
        "update",
        "set-scheduled",
        "2026-04-20",
        p,
        "--format",
        "json",
    ]);
    assert_mutation_schema(&sched, "set-scheduled", false);
    assert_eq!(sched["files_modified"], 1);
    assert_eq!(sched["entries_affected"], 2);
    assert_eq!(sched["details"][0]["line"], 1);
    assert_eq!(sched["details"][1]["line"], 3);

    let d_args = [
        "update",
        "set-deadline",
        "2026-04-30",
        p,
        "--dry-run",
        "--format",
        "json",
    ];
    let dead = run_org_json(&d_args);
    assert_mutation_schema(&dead, "set-deadline", true);
    assert_eq!(dead["files_modified"], 1);
    assert_eq!(dead["entries_affected"], 2);
}

/// Verifies warm `.cache/mabuse.db` invalidation (even under frozen `mtime`) without creating cold caches.
#[test]
fn slice2_invalidates_warm_mabuse_cache_without_autocreating_cold_cache() {
    let dir = TempDir::new().unwrap();
    let raw = "#+TODO: TODO NEXT | DONE\n* TODO Task\nSCHEDULED: <2026-04-10 Fri> DEADLINE: <2026-04-15 Wed>\n";
    let file = write_org(dir.path(), "sub/tasks.org", raw);
    let p = file.to_str().unwrap();
    let db_path = join_slash_rel(dir.path(), DEFAULT_CACHE_REL_PATH);

    run_ok(&["update", "set-scheduled", "2026-04-11", p]);
    run_ok(&["update", "set-deadline", "2026-04-16", p]);
    run_ok(&["update", "set-state", "NEXT", p]);
    assert!(!db_path.exists());

    MabuseCache::rebuild(dir.path()).expect("rebuild warm cache");
    assert_eq!(sync_misses(dir.path()), 0);

    let old_mtime = fs::metadata(&file).unwrap().modified().unwrap();
    run_ok(&["update", "set-scheduled", "2026-04-20", p]);
    let fh = File::options().write(true).open(&file).unwrap();
    fh.set_times(FileTimes::new().set_modified(old_mtime))
        .unwrap();
    drop(fh);
    assert_eq!(sync_misses(dir.path()), 1);

    run_ok(&["update", "set-deadline", "2026-04-25", p]);
    assert_eq!(sync_misses(dir.path()), 1);

    run_ok(&["update", "set-state", "TODO", p]);
    assert_eq!(sync_misses(dir.path()), 1);
}

/// Verifies exit code `2` on invalid dates, missing locators, Org `--stdin`, empty targets, and file nodes.
#[test]
fn slice2_exits_two_on_invalid_specs_missing_locators_org_stdin_and_file_nodes() {
    let dir = TempDir::new().unwrap();
    let raw =
        format!(":PROPERTIES:\n:ID: {FILE_NODE_UUID}\n:END:\n#+TITLE: Note\n* TODO Heading\n");
    let file = write_org(dir.path(), "roam.org", &raw);
    let valid_loc = format!("{}::5", file.display());

    for bad_spec in ["2025-02-29", "not-a-date", "[2026-04-20 Mon]", "+0h"] {
        run_err_in(
            dir.path(),
            &["update", "set-scheduled", bad_spec, &valid_loc],
        );
    }

    let bad_h = format!("{}::NoSuchHeading", file.display());
    let pre_h = format!("{}::1", file.display());
    let bad_cid = format!("{}::#missing-custom", file.display());
    for bad in [
        bad_h.as_str(),
        pre_h.as_str(),
        bad_cid.as_str(),
        "missing.org::1",
        "id:deadbeef-0",
    ] {
        run_err_in(dir.path(), &["update", "set-deadline", "2026-04-20", bad]);
    }

    org()
        .args(["update", "set-scheduled", "2026-04-20", "--stdin"])
        .write_stdin("* TODO Raw Org Content\n")
        .assert()
        .code(EXIT_ERROR_CODE)
        .stderr(predicate::str::contains(
            "stdin looks like org-mode content",
        ));

    run_err_in(dir.path(), &["update", "set-scheduled", "2026-04-20"]);
    org()
        .args(["update", "set-deadline", "--clear", "--stdin"])
        .write_stdin("\n")
        .assert()
        .code(EXIT_ERROR_CODE);

    let file_id_loc = format!("id:{FILE_NODE_UUID}");
    run_err_in(
        dir.path(),
        &["update", "set-scheduled", "2026-04-20", &file_id_loc],
    );
    run_err_in(
        dir.path(),
        &["update", "set-deadline", "2026-04-20", &file_id_loc],
    );
}

// endregion

// region Slice 3: Repeater Auto-Advance & --no-repeat in set-state

/// Verifies `set-state DONE` advances `+1w`, `++2d`, `.+1m` repeaters and emits `action: "repeated"`.
#[test]
fn slice3_set_state_done_advances_repeaters_and_reports_repeated_action() {
    let dir = TempDir::new().unwrap();
    let fixture = concat!(
        "#+TODO: TODO WAIT | DONE\n",
        "* TODO Weekly Review\nSCHEDULED: <2026-02-01 Sun +1w>\n",
        "* TODO Catchup Backup\nDEADLINE: <2026-02-01 Sun ++2d>\n",
        "* WAIT Monthly Audit\nActive body <2026-01-15 Thu .+1m>\n",
        "* TODO One-Off Task\n",
    );
    let file = write_org(dir.path(), "habits.org", fixture);
    let p = file.to_str().unwrap();

    let json = run_org_json(&["update", "set-state", "DONE", p, "--format", "json"]);
    assert_mutation_schema(&json, "set-state", false);
    assert_eq!(json["entries_affected"], 4);

    let actions: Vec<&str> = json["details"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["action"].as_str())
        .collect();
    assert_eq!(
        actions,
        vec!["repeated", "repeated", "repeated", "state-changed"]
    );

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("* TODO Weekly Review\nSCHEDULED: <2026-02-08 Sun +1w>"));
    assert!(updated.contains("* TODO Catchup Backup\nDEADLINE: <20"));
    assert!(!updated.contains("DEADLINE: <2026-02-01 Sun ++2d>"));
    assert!(updated.contains("* TODO Monthly Audit\n:PROPERTIES:\n:LAST_REPEAT: [20"));
    assert!(updated.contains("Active body <20") && !updated.contains("<2026-01-15 Thu .+1m>"));
    assert!(updated.contains("* DONE One-Off Task\nCLOSED: ["));
    assert_eq!(updated.matches("CLOSED:").count(), 1);
}

/// Verifies `set-state DONE --no-repeat` closes repeating tasks permanently and honors `--no-closed`.
#[test]
fn slice3_set_state_no_repeat_closes_permanently_and_honors_no_closed() {
    let dir = TempDir::new().unwrap();
    let fixture = concat!(
        "* TODO Retire With Closed\nSCHEDULED: <2026-02-01 Sun +1w>\n",
        "* TODO Retire Without Closed\nSCHEDULED: <2026-02-01 Sun +1w>\n",
    );
    let file = write_org(dir.path(), "retire.org", fixture);
    let l1 = format!("{}::*/Retire With Closed", file.display());
    let l2 = format!("{}::*/Retire Without Closed", file.display());

    let mut a1 = vec!["update", "set-state", "DONE", &l1, "--no-repeat"];
    a1.extend_from_slice(&["--format", "json"]);
    let j1 = run_org_json(&a1);
    assert_mutation_schema(&j1, "set-state", false);
    assert_eq!(j1["details"][0]["action"], "state-changed");

    let mut a2 = vec!["update", "set-state", "DONE", &l2, "--no-repeat"];
    a2.extend_from_slice(&["--no-closed", "--format", "json"]);
    let j2 = run_org_json(&a2);
    assert_mutation_schema(&j2, "set-state", false);
    assert_eq!(j2["details"][0]["action"], "state-changed");

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("* DONE Retire With Closed\nCLOSED: ["));
    assert!(updated.contains("* DONE Retire Without Closed\nSCHEDULED: <2026-02-01 Sun +1w>"));
    assert_eq!(updated.matches("CLOSED:").count(), 1);
}

// endregion
