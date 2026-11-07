// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural CLI and JSON Schema contract tests for `org query agenda` (`ORGFMT-WP-13`).

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Map, Value};
use tempfile::TempDir;

// region Constants & Test Harness Helpers

/// Expected CLI exit code when matching agenda items are found.
const EXIT_OK: i32 = 0;

/// Expected CLI exit code when no agenda or composite GTD items match.
const EXIT_ISSUES: i32 = 1;

/// Expected CLI exit code on operational or validation errors.
const EXIT_ERROR: i32 = 2;

/// Runs the compiled `org` binary with `args` and returns raw process [`Output`].
fn run_org(args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_org");
    Command::new(bin).args(args).output().expect("org executes")
}

/// Asserts `org` exits with `code` and returns `(stdout, stderr)`.
fn assert_org_code(args: &[&str], code: i32) -> (String, String) {
    let out = run_org(args);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let msg = format!("args={args:?}\nout={stdout}\nerr={stderr}");
    assert_eq!(out.status.code(), Some(code), "{msg}");
    (stdout, stderr)
}

/// Asserts `text` contains every `|`-delimited substring in `needles`.
fn assert_has_all(text: &str, needles: &str) {
    for n in needles.split('|') {
        assert!(text.contains(n), "missing {n:?} in:\n{text}");
    }
}

/// Runs `org query agenda <target>` with whitespace-delimited `flags` and optional `-o` path.
fn run_agenda_ws(target: &Path, flags: &str, out: Option<&Path>, code: i32) -> (String, String) {
    let target_str = target.to_str().expect("utf-8 path");
    let mut args: Vec<&str> = vec!["query", "agenda", target_str];
    args.extend(flags.split_whitespace());
    if let Some(out_p) = out {
        args.extend(["-o", out_p.to_str().expect("utf-8 output path")]);
    }
    assert_org_code(&args, code)
}

/// Runs `org query agenda <target> <flag_spec>` expecting `EXIT_OK` and returns stdout.
fn agenda_ok(target: &Path, flag_spec: &str) -> String {
    run_agenda_ws(target, flag_spec, None, EXIT_OK).0
}

/// Runs `org query agenda <target> <flag_spec> -o <out>` expecting `EXIT_OK`.
fn agenda_out(target: &Path, flag_spec: &str, out: &Path) {
    run_agenda_ws(target, flag_spec, Some(out), EXIT_OK);
}

/// Asserts `org fmt check <org_file>` exits with `EXIT_OK` and zero diagnostic lines.
fn assert_org_fmt_clean(org_file: &Path) {
    let (out, err) = assert_org_code(&["fmt", "check", org_file.to_str().unwrap()], EXIT_OK);
    assert!(out.trim().is_empty(), "diagnostics:\n{out}\n{err}");
}

/// Checks whether `instance` matches a single JSON Schema primitive type string.
fn matches_primitive_type(instance: &Value, ty_name: &str) -> bool {
    match ty_name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => false,
    }
}

/// Validates `const` and `enum` scalar constraints on `instance`.
fn validate_const_and_enum(instance: &Value, schema: &Value) -> Result<(), String> {
    if let Some(expected) = schema.get("const") {
        if instance != expected {
            return Err(format!("expected const {expected}, got {instance}"));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(instance) {
            return Err(format!("value {instance} not in enum {allowed:?}"));
        }
    }
    Ok(())
}

/// Validates JSON Schema `type` and `minimum` constraints on `instance`.
fn validate_type_and_minimum(instance: &Value, schema: &Value) -> Result<(), String> {
    if let Some(ty) = schema.get("type") {
        let ok = match ty {
            Value::String(s) => matches_primitive_type(instance, s),
            Value::Array(arr) => arr
                .iter()
                .filter_map(Value::as_str)
                .any(|s| matches_primitive_type(instance, s)),
            _ => false,
        };
        if !ok {
            return Err(format!("type mismatch: expected {ty}, got {instance}"));
        }
    }
    let min_opt = schema.get("minimum").and_then(Value::as_i64);
    if let (Some(min), Some(n)) = (min_opt, instance.as_i64()) {
        if n < min {
            return Err(format!("integer {n} below minimum {min}"));
        }
    }
    Ok(())
}

/// Validates `required` and `additionalProperties: false` key bounds on a JSON object.
fn validate_object_keys(obj: &Map<String, Value>, schema: &Value) -> Result<(), String> {
    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for key in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(format!("missing required property '{key}'"));
            }
        }
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for key in obj.keys() {
                if !props.contains_key(key) {
                    return Err(format!("unexpected additional property '{key}'"));
                }
            }
        }
    }
    Ok(())
}

/// Validates nested property values inside a JSON object against `s`.
fn validate_object_children(obj: &Map<String, Value>, s: &Value, r: &Value) -> Result<(), String> {
    validate_object_keys(obj, s)?;
    if let Some(props) = s.get("properties").and_then(Value::as_object) {
        for (k, v) in obj {
            if let Some(prop_s) = props.get(k) {
                validate_schema_node(v, prop_s, r)?;
            }
        }
        return Ok(());
    }
    if let Some(add_s) = s.get("additionalProperties").filter(|v| v.is_object()) {
        for v in obj.values() {
            validate_schema_node(v, add_s, r)?;
        }
    }
    Ok(())
}

/// Validates every element of `arr` against `schema["items"]` when present.
fn validate_array_items(arr: &[Value], schema: &Value, root: &Value) -> Result<(), String> {
    if let Some(item_schema) = schema.get("items") {
        for item in arr {
            validate_schema_node(item, item_schema, root)?;
        }
    }
    Ok(())
}

/// Resolves a local `#/$defs/<Name>` `$ref` target or returns `schema` unchanged.
fn resolve_schema_ref<'a>(schema: &'a Value, root: &'a Value) -> Result<&'a Value, String> {
    let Some(ref_path) = schema.get("$ref").and_then(Value::as_str) else {
        return Ok(schema);
    };
    let name = ref_path.strip_prefix("#/$defs/").ok_or("bad $ref")?;
    Ok(&root["$defs"][name])
}

/// Recursively validates `instance` against a JSON Schema node within `root`.
fn validate_schema_node(instance: &Value, raw: &Value, root: &Value) -> Result<(), String> {
    let schema = resolve_schema_ref(raw, root)?;
    validate_const_and_enum(instance, schema)?;
    validate_type_and_minimum(instance, schema)?;
    if let Some(obj) = instance.as_object() {
        validate_object_children(obj, schema, root)?;
    } else if let Some(arr) = instance.as_array() {
        validate_array_items(arr, schema, root)?;
    }
    Ok(())
}

/// Writes a composite GTD + timeline fixture workspace inside `dir`.
fn write_composite_workspace(dir: &Path) {
    let tasks = concat!(
        "* NEXT [#A] Implement agenda engine :@code:\n",
        "SCHEDULED: <2026-06-10 Wed 09:00 +1w>\n",
        ":PROPERTIES:\n:ID: 11111111-1111-4111-8111-111111111111\n:END:\n",
        "Check [[id:11111111-1111-4111-8111-111111111111][Engine Spec]].\n",
        "* TODO Publish release notes :@docs:\n",
        "DEADLINE: <2026-06-14 Sun -5d>\n",
        ":PROPERTIES:\n:BLOCKER: ids(11111111-1111-4111-8111-111111111111)\n:END:\n",
        "* TODO Untriaged capture thought\n",
    );
    fs::write(dir.join("tasks.org"), tasks).unwrap();
}

// endregion

// region Slice 1: Repeaters, Overdue, Deadline Warnings & Body Timestamps

/// Verifies `+1w`, `++2d`, `.+1m` repeaters and Day-0 overdue exclusion of `DONE` items.
#[test]
fn slice1_projects_repeaters_and_surfaces_uncompleted_overdue_items() {
    let dir = TempDir::new().unwrap();
    let content = concat!(
        "* TODO Weekly sync\nSCHEDULED: <2026-06-10 Wed 09:30 +1w>\n",
        "* TODO Catchup review\nSCHEDULED: <2026-06-07 Sun ++2d>\n",
        "* TODO Monthly billing\nDEADLINE: <2026-05-15 Fri .+1m>\n",
        "* DONE Finished past task\nSCHEDULED: <2026-06-08 Mon>\nDEADLINE: <2026-06-05 Fri>\n",
    );
    let file = dir.path().join("repeaters.org");
    fs::write(&file, content).unwrap();

    let stdout = agenda_ok(&file, "--from 2026-06-10 --days 14 --warning-days 0");
    assert_has_all(
        &stdout,
        "2026-06-10 Wed|2026-06-17 Wed|2026-06-11 Thu|2026-06-15 Mon",
    );
    assert_has_all(&stdout, "Sched. 3x: Scheduled|In -26 d.: DEADLINE");
    assert!(!stdout.contains("Finished past task"));
}

/// Verifies Day-0 deadline warnings honor CLI, `.org-tools.toml`, `-3d`, `--5d`, and `0` suppression.
#[test]
fn slice1_deadline_warnings_honor_cli_config_per_timestamp_and_zero_suppression() {
    let dir = TempDir::new().unwrap();
    let toml_path = dir.path().join(".org-tools.toml");
    fs::write(toml_path, "[agenda]\ndeadline_warning_days = 6\n").unwrap();
    let content = concat!(
        "* TODO Config window hit\nDEADLINE: <2026-06-15 Mon>\n",
        "* TODO Config window miss\nDEADLINE: <2026-06-20 Sat>\n",
        "* TODO Explicit narrow hit\nDEADLINE: <2026-06-13 Sat -3d>\n",
        "* TODO First only base hit\nDEADLINE: <2026-06-14 Sun +1m --5d>\n",
        "* TODO First only repeater suppressed\nDEADLINE: <2026-05-14 Thu +1m --5d>\n",
        "* TODO All occurrences repeater warned\nDEADLINE: <2026-05-14 Thu +1m -5d>\n",
    );
    fs::write(dir.path().join("deadlines.org"), content).unwrap();

    let cfg_out = agenda_ok(dir.path(), "--from 2026-06-10 --days 1");
    assert!(cfg_out.contains("Config window hit") && cfg_out.contains("In 5 d.: DEADLINE"));
    assert!(!cfg_out.contains("Config window miss"));
    assert!(cfg_out.contains("Explicit narrow hit") && cfg_out.contains("In 3 d.: DEADLINE"));
    assert!(cfg_out.contains("First only base hit") && cfg_out.contains("In 4 d.: DEADLINE"));
    assert_eq!(cfg_out.matches("First only repeater suppressed").count(), 1);
    let all_warn_cnt = cfg_out.matches("All occurrences repeater warned").count();
    assert_eq!(all_warn_cnt, 2);

    let zero_out = agenda_ok(dir.path(), "--from 2026-06-10 --days 1 --warning-days 0");
    assert!(!zero_out.contains("Config window hit"));
}

/// Verifies unprotected active body timestamps (including repeaters) appear while excluded zones do not.
#[test]
fn slice1_surfaces_active_body_timestamps_and_ignores_excluded_regions() {
    let dir = TempDir::new().unwrap();
    let content = concat!(
        "* Team Offsite\n:PROPERTIES:\n:CREATED: <2026-06-11 Thu>\n:END:\n",
        ":LOGBOOK:\n- Logged <2026-06-12 Fri>\n:END:\n",
        "Active appointment <2026-06-13 Sat 14:00> and repeating <2026-06-08 Mon 10:00 +1w>.\n",
        "Inactive [2026-06-14 Sun] and inline =<2026-06-16 Tue>=.\n",
        "#+BEGIN_SRC org\n<2026-06-17 Wed>\n#+END_SRC\n",
        "#+BEGIN_EXAMPLE\n<2026-06-18 Thu>\n#+END_EXAMPLE\n",
    );
    let file = dir.path().join("events.org");
    fs::write(&file, content).unwrap();

    let human = agenda_ok(&file, "--from 2026-06-10 --days 14");
    assert!(human.contains("2026-06-13 Sat") && human.contains("14:00 Timestamp"));
    assert!(human.contains("2026-06-15 Mon") && human.contains("10:00 Timestamp"));
    for day_num in [11, 12, 14, 16, 17, 18] {
        assert!(!human.contains(&format!("2026-06-{day_num:02}")));
    }

    let json_raw = agenda_ok(&file, "--from 2026-06-10 --days 14 --format json");
    let parsed: Value = serde_json::from_str(&json_raw).expect("valid JSON");
    assert_eq!(parsed["days_timeline"][0]["items"][0]["kind"], "timestamp");
}

// endregion

// region Slice 2: Composite GTD Sections, Deduplicated Locators & Error Codes

/// Verifies `--composite`, deduplicated `file::line` locators, and empty vs non-empty exit codes.
#[test]
fn slice2_composite_gtd_sections_deduplicated_locators_and_exit_codes() {
    let dir = TempDir::new().unwrap();
    write_composite_workspace(dir.path());

    let comp = agenda_ok(dir.path(), "--from 2026-06-10 --days 14 --composite");
    assert_has_all(&comp, "Next Actions|Blocked|Inbox");
    assert_has_all(
        &comp,
        "Implement agenda engine|Publish release notes|Untriaged capture thought",
    );

    let loc_flags = "--from 2026-06-10 --days 14 --composite --format locator";
    let loc_out = agenda_ok(dir.path(), loc_flags);
    let loc_lines: Vec<&str> = loc_out.lines().collect();
    let unique: HashSet<&str> = loc_lines.iter().copied().collect();
    assert_eq!(loc_lines.len(), 3);
    assert_eq!(loc_lines.len(), unique.len());
    assert!(loc_lines.iter().all(|l| l.contains(".org::")));

    let empty_dir = TempDir::new().unwrap();
    fs::write(empty_dir.path().join("plain.org"), "* Notes\nJust prose.\n").unwrap();
    run_agenda_ws(empty_dir.path(), "--composite", None, EXIT_ISSUES);
}

/// Verifies invalid `--from` dates, missing `.org` files, and malformed `.org-tools.toml` exit `2`.
#[test]
fn slice2_invalid_dates_missing_files_and_invalid_config_exit_error() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ok.org");
    fs::write(&file, "* TODO Valid\nSCHEDULED: <2026-06-10 Wed>\n").unwrap();

    run_agenda_ws(&file, "--from 2026-02-30", None, EXIT_ERROR);
    run_agenda_ws(&file, "--from bad-date", None, EXIT_ERROR);
    run_agenda_ws(&dir.path().join("missing.org"), "", None, EXIT_ERROR);

    let empty_dir = TempDir::new().unwrap();
    run_agenda_ws(empty_dir.path(), "", None, EXIT_ERROR);

    let bad_cfg = TempDir::new().unwrap();
    let bad_toml = bad_cfg.path().join(".org-tools.toml");
    fs::write(bad_toml, "invalid_toml = [[[\n").unwrap();
    fs::write(bad_cfg.path().join("t.org"), "* TODO T\n").unwrap();
    run_agenda_ws(bad_cfg.path(), "", None, EXIT_ERROR);
}

// endregion

// region Slice 3: Atomic Org & Zensical Markdown Report Materialization

/// Verifies `--composite --format org -o agenda.org` passes `org fmt check` with zero diagnostics.
#[test]
fn slice3_atomic_org_report_passes_fmt_check_and_md_renders_zensical_gfm() {
    let (ws, out_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    write_composite_workspace(ws.path());
    let org_out = out_dir.path().join("agenda.org");
    let md_out = out_dir.path().join("agenda.md");

    let f_org = "--from 2026-06-10 --days 14 --composite --format org";
    agenda_out(ws.path(), f_org, &org_out);
    let org_txt = fs::read_to_string(&org_out).expect("agenda.org written");
    assert!(org_txt.starts_with("#+TITLE: Agenda"));
    assert_has_all(&org_txt, "* Next Actions|* Blocked|* Inbox");
    assert_org_fmt_clean(&org_out);

    let f_md = "--from 2026-06-10 --days 14 --composite --format md";
    agenda_out(ws.path(), f_md, &md_out);
    let md_txt = fs::read_to_string(&md_out).expect("agenda.md written");
    assert!(md_txt.starts_with("# Agenda") && md_txt.contains("2026-06-10 Wed"));
    assert_has_all(&md_txt, "## Next Actions|## Blocked|## Inbox");
}

/// Verifies tricky footnotes, anchors, broken links, invalid dates, and `=` URLs sanitize cleanly.
#[test]
fn slice3_tricky_titles_pass_org_fmt_check_and_render_clean_markdown() {
    let (ws, out_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let tricky = concat!(
        "* NEXT Spec [fn:1] <<anchor-1>> [[https://example.com?a=1]] [[broken SCHEDULED: \n",
        "SCHEDULED: <2026-06-10 Wed +1d>\n",
        ":PROPERTIES:\n:ID: 22222222-2222-4222-8222-222222222222\n:END:\n",
        "* TODO Followup <2026-02-30 Mon> [[https://example.com/docs][Docs]]\n",
        ":PROPERTIES:\n:BLOCKER: ids(22222222-2222-4222-8222-222222222222)\n:END:\n",
    );
    fs::write(ws.path().join("tricky.org"), tricky).unwrap();
    let org_out = out_dir.path().join("agenda.org");
    let md_out = out_dir.path().join("agenda.md");

    let f_org = "--from 2026-06-10 --days 3 --composite --format org";
    agenda_out(ws.path(), f_org, &org_out);
    assert_org_fmt_clean(&org_out);
    let org_txt = fs::read_to_string(&org_out).unwrap();
    assert!(!org_txt.contains("=https://example.com?a=1="));

    let f_md = "--from 2026-06-10 --days 3 --composite --format md";
    agenda_out(ws.path(), f_md, &md_out);
    let md_txt = fs::read_to_string(&md_out).unwrap();
    assert!(!md_txt.contains("=https://example.com?a=1="));
    assert_has_all(
        &md_txt,
        "`https://example.com?a=1`|[Docs](https://example.com/docs)",
    );
}

// endregion

// region Slice 4: JSON Schema Contract & Warm Cache Telemetry

/// Verifies `org schema agenda`, JSON validation (plain & composite), and warm `.cache/mabuse.db` sync.
#[test]
fn slice4_json_conforms_to_schema_and_syncs_only_existing_mabuse_cache() {
    let (schema_raw, _) = assert_org_code(&["schema", "agenda"], EXIT_OK);
    let (alias_raw, _) = assert_org_code(&["schema", "org-agenda-output"], EXIT_OK);
    assert_eq!(schema_raw, alias_raw);
    let schema: Value = serde_json::from_str(&schema_raw).expect("valid agenda JSON schema");

    let ws_dir = TempDir::new().unwrap();
    write_composite_workspace(ws_dir.path());
    let ws = ws_dir.path();

    let plain_raw = agenda_ok(ws, "--from 2026-06-10 --days 14 --format json");
    assert!(!ws.join(".cache/mabuse.db").exists());
    let plain_val: Value = serde_json::from_str(&plain_raw).expect("valid plain JSON");
    validate_schema_node(&plain_val, &schema, &schema).expect("plain JSON matches schema");
    assert!(plain_val.get("composite").is_none());
    assert_eq!(plain_val["telemetry"]["files_scanned"], 1);
    assert_eq!(plain_val["telemetry"]["cache_hits"], 0);
    assert_eq!(plain_val["telemetry"]["cache_misses"], 1);

    assert_org_code(&["cache", "rebuild", ws.to_str().unwrap()], EXIT_OK);
    assert!(ws.join(".cache/mabuse.db").is_file());

    let comp_raw = agenda_ok(ws, "--from 2026-06-10 --days 14 --composite --format json");
    let comp_val: Value = serde_json::from_str(&comp_raw).expect("valid composite JSON");
    validate_schema_node(&comp_val, &schema, &schema).expect("composite JSON matches schema");
    let comp = &comp_val["composite"];
    assert_eq!(comp["next_actions"].as_array().unwrap().len(), 2);
    assert_eq!(comp["blocked"].as_array().unwrap().len(), 1);
    assert_eq!(comp["inbox"].as_array().unwrap().len(), 1);
    assert_eq!(comp_val["telemetry"]["files_scanned"], 1);
    assert_eq!(comp_val["telemetry"]["cache_hits"], 1);
    assert_eq!(comp_val["telemetry"]["cache_misses"], 0);
}

// endregion
