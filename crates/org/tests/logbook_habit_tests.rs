// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural Red-to-Green tests for `ORGFMT-WP-16`: `#+STARTUP:` / `#+TODO:` / `:LOGGING:`
//! configuration, `:LOGBOOK:` state transition logging, `:LAST_REPEAT:` updates, `ORG_TOOLS_NOW`,
//! and `org-habit` dual repeaters with 21-day consistency graphs in `org query agenda`.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use org_tools_core::document::OrgDocument;
use org_tools_core::logbook::{
    build_habit_summary, format_state_log_line, resolve_entry_logging, should_log_state_transition,
    HabitSummary, ENV_ORG_TOOLS_NOW,
};
use org_tools_core::planning::{
    advance_timestamp_repeater, parse_repeater, RepeaterKind, RepeaterSpec, TimeUnit,
};
use org_tools_core::rules::timestamp::{is_valid_repeater, parse_timestamp};
use org_tools_core::source::SourceFile;
use org_tools_core::state::{set_state_with_options, SetStateOptions};
use serde_json::{Map, Value};
use tempfile::TempDir;

// region Test Helpers

/// Parses raw Org markup into a `(SourceFile, OrgDocument)` test fixture.
fn parse_org(raw: &str) -> (SourceFile, OrgDocument) {
    let src = SourceFile::new("fixture.org", raw.to_string());
    let doc = OrgDocument::from_source(&src);
    (src, doc)
}

/// Builds an [`assert_cmd::Command`] targeting the compiled `org` binary.
fn org() -> Command {
    Command::from_std(StdCommand::new(env!("CARGO_BIN_EXE_org")))
}

/// Runs `org <args>`, asserts exit code 0, and returns stdout as a UTF-8 [`String`].
fn run_stdout(args: &[&str]) -> String {
    let assert = org().args(args).assert().success();
    String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout")
}

/// Runs `org query agenda <file> --from 2026-03-21 --days 3 <extra>` and returns stdout.
fn run_agenda(file: &str, extra: &[&str]) -> String {
    let mut args = vec![
        "query",
        "agenda",
        file,
        "--from",
        "2026-03-21",
        "--days",
        "3",
    ];
    args.extend_from_slice(extra);
    run_stdout(&args)
}

/// Writes `content` to `dir/filename` and returns its [`std::path::PathBuf`].
fn write_file_in(dir: &Path, filename: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(filename);
    fs::write(&path, content).expect("write test fixture");
    path
}

/// Asserts that `org fmt check <file>` succeeds with zero diagnostics.
fn assert_fmt_clean(file: &Path) {
    org()
        .args(["fmt", "check", file.to_str().unwrap()])
        .assert()
        .success()
        .stdout("");
}

/// Checks whether `instance` matches a JSON Schema primitive type keyword.
fn matches_type(instance: &Value, ty: &str) -> bool {
    match ty {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => false,
    }
}

/// Validates scalar `type`, `const`, `enum`, and `minimum` constraints on `val`.
fn check_scalar(val: &Value, schema: &Value) {
    if let Some(ty) = schema.get("type") {
        let ok = match ty {
            Value::String(s) => matches_type(val, s),
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .any(|s| matches_type(val, s)),
            _ => false,
        };
        assert!(ok, "type mismatch: expected {ty}, got {val}");
    }
    if let Some(c) = schema.get("const") {
        assert_eq!(val, c);
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        assert!(allowed.contains(val), "{val} not in {allowed:?}");
    }
    if let (Some(min), Some(n)) = (schema.get("minimum").and_then(Value::as_i64), val.as_i64()) {
        assert!(n >= min, "{n} < {min}");
    }
}

/// Validates object `required`, `properties`, and `additionalProperties` against `schema`.
fn check_object(obj: &Map<String, Value>, schema: &Value, root: &Value) {
    if let Some(reqs) = schema.get("required").and_then(Value::as_array) {
        for req in reqs.iter().filter_map(Value::as_str) {
            assert!(obj.contains_key(req), "missing required key '{req}'");
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    for (k, v) in obj {
        if let Some(p_schema) = props.and_then(|m| m.get(k)) {
            assert_valid_schema(v, p_schema, root);
        } else if let Some(add) = schema.get("additionalProperties") {
            assert_ne!(add, &Value::Bool(false), "unexpected property '{k}'");
            if add.is_object() {
                assert_valid_schema(v, add, root);
            }
        }
    }
}

/// Recursively validates `val` against a JSON Schema node within `root`.
fn assert_valid_schema(val: &Value, schema: &Value, root: &Value) {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let key = r.strip_prefix("#/$defs/").expect("local $defs ref");
        return assert_valid_schema(val, &root["$defs"][key], root);
    }
    check_scalar(val, schema);
    if let (Some(arr), Some(items)) = (val.as_array(), schema.get("items")) {
        for item in arr {
            assert_valid_schema(item, items, root);
        }
    }
    if let Some(obj) = val.as_object() {
        check_object(obj, schema, root);
    }
}

// endregion

// region Slice 1: #+STARTUP:, #+TODO:, & :LOGGING: Configuration

/// Verifies `#+STARTUP:`, `#+TODO:` per-keyword flags (`!`, `@`, `/!`), and `:LOGGING:` overrides.
#[test]
fn slice1_startup_todo_flags_and_logging_property_overrides() {
    let raw = concat!(
        "#+STARTUP: overview logdone\n",
        "#+STARTUP: nologrepeat\n",
        "#+TODO: TODO(t) WAIT(w!) HOLD(!) | DONE(d!) CANCELED(c@/!)\n",
        "* TODO Default Entry\n",
        "* TODO Nil Entry\n:PROPERTIES:\n:LOGGING: nil\n:END:\n",
        "* TODO Custom Entry\n:PROPERTIES:\n:LOGGING: nologdone logrepeat TODO(!)\n:END:\n",
    );
    let (_src, doc) = parse_org(raw);

    assert!(doc.todo_keywords.logs_on_enter("WAIT"));
    assert!(doc.todo_keywords.logs_on_enter("HOLD"));
    assert!(doc.todo_keywords.logs_on_enter("DONE"));
    assert!(doc.todo_keywords.logs_on_enter("CANCELED"));
    assert!(!doc.todo_keywords.logs_on_enter("TODO"));
    assert!(doc.todo_keywords.logs_on_leave("CANCELED"));
    assert!(!doc.todo_keywords.logs_on_leave("WAIT"));

    let c0 = resolve_entry_logging(&doc, 0);
    assert!(!c0.disabled && c0.log_done && !c0.log_repeat);
    assert!(should_log_state_transition(
        &doc,
        0,
        Some("TODO"),
        "WAIT",
        false
    ));
    assert!(should_log_state_transition(
        &doc,
        0,
        Some("CANCELED"),
        "TODO",
        false
    ));
    assert!(!should_log_state_transition(
        &doc,
        0,
        Some("TODO"),
        "DONE",
        true
    ));

    let c1 = resolve_entry_logging(&doc, 1);
    assert!(c1.disabled && !c1.log_done && !c1.log_repeat);
    assert!(!should_log_state_transition(
        &doc,
        1,
        Some("TODO"),
        "WAIT",
        false
    ));
    assert!(!should_log_state_transition(
        &doc,
        1,
        Some("TODO"),
        "DONE",
        false
    ));
    assert!(!should_log_state_transition(
        &doc,
        1,
        Some("TODO"),
        "DONE",
        true
    ));

    let c2 = resolve_entry_logging(&doc, 2);
    assert!(!c2.disabled && !c2.log_done && c2.log_repeat);
    assert!(should_log_state_transition(
        &doc,
        2,
        Some("WAIT"),
        "TODO",
        false
    ));
    assert!(!should_log_state_transition(
        &doc,
        2,
        Some("TODO"),
        "DONE",
        false
    ));
    assert!(should_log_state_transition(
        &doc,
        2,
        Some("TODO"),
        "DONE",
        true
    ));
}

// endregion

// region Slice 2: :LOGBOOK: State Entry, :LAST_REPEAT:, & ORG_TOOLS_NOW

/// Verifies repeater auto-advance writes `- State` at top of `:LOGBOOK:` and `F005`-aligned `:LAST_REPEAT:`.
#[test]
fn slice2_repeater_auto_advance_logs_state_and_updates_aligned_last_repeat() {
    assert_eq!(
        format_state_log_line("DONE", Some("TODO"), "2026-03-05 Thu 14:30"),
        "- State \"DONE\"       from \"TODO\"       [2026-03-05 Thu 14:30]"
    );

    let dir = TempDir::new().unwrap();
    let fixture = concat!(
        "* TODO Morning Run\n",
        "SCHEDULED: <2026-03-01 Sun .+1d>\n",
        ":PROPERTIES:\n",
        ":STYLE: habit\n",
        ":END:\n",
        ":LOGBOOK:\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-01 Sun 08:00]\n",
        ":END:\n",
        "\n",
        "* TODO Quiet Repeater\n",
        "SCHEDULED: <2026-03-01 Sun +1w>\n",
        ":PROPERTIES:\n",
        ":LOGGING: nil\n",
        ":END:\n",
    );
    let file = write_file_in(dir.path(), "repeat.org", fixture);

    org()
        .env(ENV_ORG_TOOLS_NOW, "2026-03-05T14:30")
        .args(["update", "set-state", "DONE", file.to_str().unwrap()])
        .assert()
        .success();

    let updated = fs::read_to_string(&file).unwrap();
    let expected_first = concat!(
        "* TODO Morning Run\n",
        "SCHEDULED: <2026-03-06 Fri .+1d>\n",
        ":PROPERTIES:\n",
        ":STYLE:       habit\n",
        ":LAST_REPEAT: [2026-03-05 Thu 14:30]\n",
        ":END:\n",
        ":LOGBOOK:\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-05 Thu 14:30]\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-01 Sun 08:00]\n",
        ":END:\n",
    );
    assert!(updated.contains(expected_first), "got:\n{updated}");
    assert_eq!(updated.matches(":LAST_REPEAT:").count(), 1);
    assert_eq!(updated.matches("2026-03-05 Thu 14:30").count(), 2);
    assert_fmt_clean(&file);
}

/// Verifies non-repeating transitions log to `:LOGBOOK:` on `logdone`, enter (`!`/`@`), and leave (`/!`).
#[test]
fn slice2_non_repeating_transitions_log_enter_leave_and_from_empty() {
    let dir = TempDir::new().unwrap();
    let fixture = concat!(
        "#+STARTUP: logdone\n",
        "#+TODO: TODO WAIT(w@/!) | DONE CANCELED(c@)\n",
        "\n",
        "* TODO Enter Wait\n",
        "\n",
        "* WAIT Leave Wait\n",
        "\n",
        "* Bare Heading\n",
    );
    let file = write_file_in(dir.path(), "transitions.org", fixture);
    let l_wait = format!("{}::*/Enter Wait", file.display());
    let l_leave = format!("{}::*/Leave Wait", file.display());
    let l_bare = format!("{}::*/Bare Heading", file.display());

    org()
        .env(ENV_ORG_TOOLS_NOW, "2026-03-05T09:15")
        .args(["update", "set-state", "WAIT", &l_wait])
        .assert()
        .success();
    org()
        .env(ENV_ORG_TOOLS_NOW, "2026-03-05T10:00")
        .args(["update", "set-state", "TODO", &l_leave])
        .assert()
        .success();
    org()
        .env(ENV_ORG_TOOLS_NOW, "2026-03-05T11:45")
        .args(["update", "set-state", "DONE", &l_bare])
        .assert()
        .success();

    let updated = fs::read_to_string(&file).unwrap();
    let e1 = "* WAIT Enter Wait\n:LOGBOOK:\n- State \"WAIT\"       from \"TODO\"       [2026-03-05 Thu 09:15]\n:END:";
    let e2 = "* TODO Leave Wait\n:LOGBOOK:\n- State \"TODO\"       from \"WAIT\"       [2026-03-05 Thu 10:00]\n:END:";
    let e3 = "* DONE Bare Heading\nCLOSED: [2026-03-05 Thu 11:45]\n:LOGBOOK:\n- State \"DONE\"       from \"\"           [2026-03-05 Thu 11:45]\n:END:";
    assert!(updated.contains(e1) && updated.contains(e2) && updated.contains(e3));
    assert_fmt_clean(&file);
}

// endregion

// region Slice 3: org-habit Dual Repeaters & Consistency Graph

/// Verifies dual min/max habit repeaters (`.+2d/4d`, `+1w/2w`, `++3d/5d`) parse, advance, and lint cleanly.
#[test]
fn slice3_dual_min_max_repeaters_parse_advance_and_pass_lint() {
    let spec = parse_repeater(".+2d/4d").expect("valid dual repeater");
    assert_eq!(
        spec,
        RepeaterSpec {
            kind: RepeaterKind::Restart,
            value: 2,
            unit: TimeUnit::Day,
            max_value: Some(4),
            max_unit: Some(TimeUnit::Day),
        }
    );
    let wk = parse_repeater("+1w/2w").expect("valid weekly dual repeater");
    assert_eq!((wk.max_value, wk.max_unit), (Some(2), Some(TimeUnit::Week)));
    assert!(is_valid_repeater("++3d/5d") && is_valid_repeater(".+1m/2m"));
    assert!(!is_valid_repeater(".+2d/0d") && !is_valid_repeater(".+2d/4x"));

    let (ts, _) = parse_timestamp("<2026-03-01 Sun .+2d/4d>", 0).unwrap();
    let adv = advance_timestamp_repeater(&ts, (2026, 3, 5)).unwrap();
    assert_eq!(adv.to_string(), "<2026-03-07 Sat .+2d/4d>");

    let raw = "* TODO Workout\nSCHEDULED: <2026-03-01 Sun .+2d/4d>\n";
    let (src, doc) = parse_org(raw);
    let opts = SetStateOptions {
        today: Some((2026, 3, 5)),
        ..Default::default()
    };
    let res = set_state_with_options(&src, &doc, None, "DONE", opts).unwrap();
    assert!(res.content.contains("SCHEDULED: <2026-03-07 Sat .+2d/4d>"));
}

/// Verifies `org query agenda` attaches `HabitSummary` and renders 21-day consistency graphs across formats.
#[test]
fn slice3_agenda_habit_summary_consistency_graph_and_schema_validation() {
    let dir = TempDir::new().unwrap();
    let content = concat!(
        "* TODO Hydrate & Stretch\n",
        "SCHEDULED: <2026-03-21 Sat .+2d/4d>\n",
        ":PROPERTIES:\n",
        ":STYLE:       habit\n",
        ":LAST_REPEAT: [2026-03-19 Thu 08:00]\n",
        ":END:\n",
        ":LOGBOOK:\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-19 Thu 08:00]\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-15 Sun 08:00]\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-10 Tue 08:00]\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-05 Thu 08:00]\n",
        "- State \"DONE\"       from \"TODO\"       [2026-03-01 Sun 08:00]\n",
        ":END:\n",
        "\n",
        "* TODO Regular Meeting\n",
        "SCHEDULED: <2026-03-21 Sat 10:00>\n",
    );
    let file = write_file_in(dir.path(), "habits.org", content);
    let (src, doc) = parse_org(content);
    let expected_graph = "*...*....*....*...*.!";
    assert_eq!(expected_graph.len(), 21);

    let summary = build_habit_summary(Some(&src), &doc, 0, (2026, 3, 21)).unwrap();
    assert_eq!(
        summary,
        HabitSummary {
            min_interval_days: 2,
            max_interval_days: Some(4),
            last_repeat: Some("[2026-03-19 Thu 08:00]".to_string()),
            consistency_graph: expected_graph.to_string(),
        }
    );
    assert!(build_habit_summary(Some(&src), &doc, 1, (2026, 3, 21)).is_none());

    let p = file.to_str().unwrap();
    let human_txt = run_agenda(p, &[]);
    assert!(human_txt.contains(expected_graph), "human:\n{human_txt}");
    assert!(run_agenda(p, &["--format", "md"]).contains(expected_graph));

    let org_report = dir.path().join("agenda_out.org");
    let _ = run_agenda(p, &["--format", "org", "-o", org_report.to_str().unwrap()]);
    assert!(fs::read_to_string(&org_report)
        .unwrap()
        .contains(expected_graph));
    assert_fmt_clean(&org_report);

    let json_val: Value = serde_json::from_str(&run_agenda(p, &["--format", "json"])).unwrap();
    let schema_val: Value = serde_json::from_str(&run_stdout(&["schema", "agenda"])).unwrap();
    assert_valid_schema(&json_val, &schema_val, &schema_val);

    let items = json_val["days_timeline"][0]["items"].as_array().unwrap();
    assert_eq!(items[0]["habit"]["min_interval_days"], 2);
    assert_eq!(items[0]["habit"]["max_interval_days"], 4);
    assert_eq!(items[0]["habit"]["last_repeat"], "[2026-03-19 Thu 08:00]");
    assert_eq!(items[0]["habit"]["consistency_graph"], expected_graph);
    assert!(items[1].get("habit").is_none());
}

// endregion
