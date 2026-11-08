// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural and contract tests for Unix manpages (`man/org.1`, `man/org-tools.toml.5`),
//! release packaging, `org schema --list`/`--output`, and JSON Schema Draft 2020-12 parity (`ORGFMT-WP-27`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value};
use tempfile::TempDir;

// region Constants & Workspace Helpers

/// Required top-level `.SH` section headings in `man/org.1`.
const REQUIRED_ORG_1_SECTIONS: &[&str] = &[
    ".SH NAME",
    ".SH SYNOPSIS",
    ".SH DESCRIPTION",
    ".SH SUBCOMMANDS",
    ".SH GLOBAL OPTIONS",
    ".SH EXIT STATUS",
    ".SH ENVIRONMENT",
    ".SH FILES",
    ".SH EXAMPLES",
    ".SH SEE ALSO",
];

/// Required environment variables documented in `man/org.1`.
const REQUIRED_ENV_VARS: &[&str] = &["NO_COLOR", "TERM", "ORG_TOOLS_CONFIG", "VISUAL", "EDITOR"];

/// Required top-level `.SH` section headings in `man/org-tools.toml.5`.
const REQUIRED_TOML_5_SECTIONS: &[&str] = &[
    ".SH NAME",
    ".SH SYNOPSIS",
    ".SH DESCRIPTION",
    ".SH CONFIGURATION HIERARCHY",
    ".SH SECTIONS",
    ".SH FILES",
    ".SH EXAMPLES",
    ".SH SEE ALSO",
];

/// Required TOML table headers documented in `man/org-tools.toml.5`.
const REQUIRED_TOML_TABLES: &[&str] = &[
    "[format]",
    "[lint]",
    "[todo]",
    "[roam]",
    "[capture.templates.<name>]",
];

/// Canonical schema names required in `org schema --list`.
const REQUIRED_SCHEMA_NAMES: &[&str] = &[
    "fmt",
    "query",
    "agenda",
    "columns",
    "show",
    "update",
    "clock-status",
    "clock-report",
    "jscalendar",
    "roam",
    "fleeting",
];

/// Forbidden internal planning-framework lingo tokens.
const FORBIDDEN_LINGO: &[&str] = &[
    "mabuse",
    "stempelamt",
    "uhrenmaschine",
    "herzmaschine",
    "freigabeamt",
    "schaltwerk",
    "moloch",
    "paternoster",
    "befundakte",
    "eichamt",
    "gesetzbuch",
    "babelsberg",
    "fritz",
];

/// Returns the repository workspace root path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/org must sit two levels below workspace root")
        .to_path_buf()
}

/// Reads a UTF-8 file relative to the workspace root.
fn read_repo_file(rel_path: &str) -> String {
    let full = workspace_root().join(rel_path);
    fs::read_to_string(&full)
        .unwrap_or_else(|err| panic!("expected {} to be readable: {err}", full.display()))
}

/// Runs the compiled `org` binary with `args` and returns `(exit_code, stdout, stderr)`.
fn run_org(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_org"))
        .current_dir(workspace_root())
        .args(args)
        .output()
        .expect("execute org binary");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (code, stdout, stderr)
}

/// Asserts that `content` contains zero forbidden framework lingo tokens or hardcoded `/Users/` paths.
fn assert_zero_lingo_and_user_paths(label: &str, content: &str) {
    assert!(
        !content.contains("/Users/"),
        "{label} must not contain hardcoded /Users/ paths"
    );
    let lower = content.to_ascii_lowercase();
    for token in FORBIDDEN_LINGO {
        assert!(
            !lower.contains(token),
            "{label} leaked forbidden planning-framework token `{token}`"
        );
    }
}

// endregion

// region Slice 1: Unix Manpages & Release Packaging

/// Verifies `man/org.1` structure, required `.SH` sections, exit codes `0..3`, env vars, and `SEE ALSO`.
#[test]
fn slice1_man_org_1_contains_required_roff_sections_exit_codes_and_env_vars() {
    let org1 = read_repo_file("man/org.1");
    assert_zero_lingo_and_user_paths("man/org.1", &org1);
    assert!(org1.contains("SPDX-License-Identifier: GPL-3.0-or-later"));
    assert!(org1.lines().any(|line| line.starts_with(".TH ORG 1")));
    for section in REQUIRED_ORG_1_SECTIONS {
        assert!(
            org1.lines().any(|line| line.trim() == *section),
            "man/org.1 missing required section `{section}`"
        );
    }
    for env_var in REQUIRED_ENV_VARS {
        assert!(org1.contains(env_var), "man/org.1 missing `{env_var}`");
    }
    for code in ["0", "1", "2", "3"] {
        assert!(org1.contains(code), "man/org.1 missing exit code `{code}`");
    }
    assert!(org1.contains("org-tools.toml(5)"));
}

/// Verifies `man/org-tools.toml.5` structure, hierarchy, configuration sections, and `SEE ALSO`.
#[test]
fn slice1_man_org_tools_toml_5_contains_required_roff_sections_and_tables() {
    let toml5 = read_repo_file("man/org-tools.toml.5");
    assert_zero_lingo_and_user_paths("man/org-tools.toml.5", &toml5);
    assert!(toml5.contains("SPDX-License-Identifier: GPL-3.0-or-later"));
    assert!(toml5
        .lines()
        .any(|line| line.starts_with(".TH ORG-TOOLS.TOML 5")));
    for section in REQUIRED_TOML_5_SECTIONS {
        assert!(
            toml5.lines().any(|line| line.trim() == *section),
            "man/org-tools.toml.5 missing required section `{section}`"
        );
    }
    for table in REQUIRED_TOML_TABLES {
        assert!(
            toml5.contains(table),
            "man/org-tools.toml.5 missing `{table}`"
        );
    }
    assert!(toml5.contains("org(1)"));
}

/// Verifies `.goreleaser.yaml` and `scripts/verify-release-packages.sh` package and verify both manpages.
#[test]
fn slice1_goreleaser_and_verify_script_package_and_check_manpages() {
    let goreleaser = read_repo_file(".goreleaser.yaml");
    for token in [
        "man/org.1",
        "man/org-tools.toml.5",
        "/usr/share/man/man1/org.1",
        "/usr/share/man/man5/org-tools.toml.5",
        "manpages:",
    ] {
        assert!(
            goreleaser.contains(token),
            ".goreleaser.yaml must include `{token}` for manpage packaging"
        );
    }
    let verify_sh = read_repo_file("scripts/verify-release-packages.sh");
    for token in ["man/org.1", "man/org-tools.toml.5"] {
        assert!(
            verify_sh.contains(token),
            "scripts/verify-release-packages.sh must verify `{token}`"
        );
    }
}

// endregion

// region Slice 2: `org schema --list`, `--format`, and `--output`

/// Verifies `org schema --list` (human and JSON) enumerates all registered JSON Schema Draft 2020-12 artifacts.
#[test]
fn slice2_org_schema_list_human_and_json_enumerate_all_registered_schemas() {
    let (code_h, stdout_h, stderr_h) = run_org(&["schema", "--list"]);
    assert_eq!(code_h, 0, "org schema --list failed: {stderr_h}");
    for name in REQUIRED_SCHEMA_NAMES {
        assert!(stdout_h.contains(name), "missing `{name}` in:\n{stdout_h}");
    }

    let (code_j, stdout_j, stderr_j) = run_org(&["schema", "--list", "--format", "json"]);
    assert_eq!(
        code_j, 0,
        "org schema --list --format json failed: {stderr_j}"
    );
    let list: Value = serde_json::from_str(&stdout_j).expect("valid JSON schema list");
    let arr = list.as_array().expect("schema list JSON must be an array");
    for expected in REQUIRED_SCHEMA_NAMES {
        let entry = arr
            .iter()
            .find(|item| item["name"].as_str() == Some(*expected))
            .unwrap_or_else(|| panic!("missing `{expected}` in JSON schema list: {stdout_j}"));
        assert!(entry["id"]
            .as_str()
            .is_some_and(|s| s.starts_with("https://org-tools.dev/schemas/")));
        assert!(entry["path"]
            .as_str()
            .is_some_and(|s| s.starts_with("schemas/") && s.ends_with(".schema.json")));
        assert!(entry["description"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty()));
    }
}

/// Verifies `org schema <name> --output <path>` (`-o`) writes valid Draft 2020-12 schemas to disk.
#[test]
fn slice2_org_schema_output_flag_writes_schema_file_and_creates_parent_dirs() {
    let temp = TempDir::new().expect("create temp dir");
    let out_long = temp.path().join("columns.schema.json");
    let (code1, stdout1, stderr1) =
        run_org(&["schema", "columns", "--output", out_long.to_str().unwrap()]);
    assert_eq!(code1, 0, "org schema columns --output failed: {stderr1}");
    assert!(stdout1.trim().is_empty());
    let written_cols = fs::read_to_string(&out_long).expect("read written columns schema");
    assert_eq!(
        written_cols.trim(),
        read_repo_file("schemas/org-columns-output.schema.json").trim()
    );

    let out_short = temp.path().join("nested/dir/clock-report.schema.json");
    let (code2, stdout2, stderr2) =
        run_org(&["schema", "clock-report", "-o", out_short.to_str().unwrap()]);
    assert_eq!(code2, 0, "org schema clock-report -o failed: {stderr2}");
    assert!(stdout2.trim().is_empty());
    let written_clock = fs::read_to_string(&out_short).expect("read written clock-report schema");
    assert_eq!(
        written_clock.trim(),
        read_repo_file("schemas/org-clock-report-output.schema.json").trim()
    );

    for name in REQUIRED_SCHEMA_NAMES {
        let (code, stdout, stderr) = run_org(&["schema", name]);
        assert_eq!(code, 0, "org schema {name} failed: {stderr}");
        let parsed: Value = serde_json::from_str(&stdout).expect("schema must be valid JSON");
        assert_eq!(
            parsed["$schema"].as_str(),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
    }
}

// endregion

// region Slice 3: Schema Validation for `columns` and `clock-report` + `cargo doc`

/// Resolves a local `#/$defs/<Key>` reference inside `root`, or returns `node` unchanged.
fn resolve_ref<'a>(node: &'a Value, root: &'a Value) -> &'a Value {
    if let Some(target) = node.get("$ref").and_then(Value::as_str) {
        let key = target.strip_prefix("#/$defs/").expect("local #/$defs/ ref");
        return &root["$defs"][key];
    }
    node
}

/// Returns `true` when `instance` matches a single JSON Schema primitive type name.
fn matches_type_name(instance: &Value, ty_name: &str) -> bool {
    match ty_name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.is_i64() || instance.is_u64(),
        "number" => instance.is_number(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => false,
    }
}

/// Validates `instance` against a JSON Schema Draft 2020-12 `node` with `additionalProperties` support.
fn validate_against_schema(instance: &Value, node: &Value, root: &Value) -> Result<(), String> {
    let schema = resolve_ref(node, root);
    if let Some(ty) = schema.get("type") {
        let matched = match ty {
            Value::String(s) => matches_type_name(instance, s),
            Value::Array(arr) => arr
                .iter()
                .filter_map(Value::as_str)
                .any(|s| matches_type_name(instance, s)),
            _ => false,
        };
        if !matched {
            return Err(format!("type mismatch: expected {ty}, got {instance}"));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(instance) {
            return Err(format!("value {instance} not in enum {allowed:?}"));
        }
    }
    if let Some(arr) = instance.as_array() {
        if let Some(item_schema) = schema.get("items") {
            for item in arr {
                validate_against_schema(item, item_schema, root)?;
            }
        }
    }
    if let Some(obj) = instance.as_object() {
        validate_object_fields(obj, schema, root)?;
    }
    Ok(())
}

/// Validates required keys, known properties, and `additionalProperties` on a JSON object `obj`.
fn validate_object_fields(
    obj: &Map<String, Value>,
    schema: &Value,
    root: &Value,
) -> Result<(), String> {
    if let Some(reqs) = schema.get("required").and_then(Value::as_array) {
        for req in reqs.iter().filter_map(Value::as_str) {
            if !obj.contains_key(req) {
                return Err(format!("missing required property `{req}`"));
            }
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    let add_props = schema.get("additionalProperties");
    for (k, v) in obj {
        if let Some(prop_schema) = props.and_then(|m| m.get(k)) {
            validate_against_schema(v, prop_schema, root)?;
        } else if add_props == Some(&Value::Bool(false)) {
            return Err(format!("unexpected additional property `{k}`"));
        } else if let Some(add_schema) = add_props.filter(|a| a.is_object()) {
            validate_against_schema(v, add_schema, root)?;
        }
    }
    Ok(())
}

/// Verifies `schemas/org-columns-output.schema.json` validates `org query columns --format json` payloads.
#[test]
fn slice3_columns_schema_validates_query_columns_json_and_rejects_extra_fields() {
    let raw_schema = read_repo_file("schemas/org-columns-output.schema.json");
    let schema: Value = serde_json::from_str(&raw_schema).expect("valid columns schema JSON");
    let row_props = &schema["$defs"]["ColumnRow"]["properties"];
    for field in "file line level title todo priority tags properties values".split_whitespace() {
        assert!(row_props.get(field).is_some(), "missing `{field}`");
    }

    let temp = TempDir::new().expect("tempdir");
    let org_file = temp.path().join("columns.org");
    fs::write(
        &org_file,
        "#+COLUMNS: %25ITEM %TODO %EFFORT{:}\n* TODO Parent\n** DONE Child\n:PROPERTIES:\n:EFFORT: 1:30\n:END:\n",
    )
    .expect("write fixture");
    let f = org_file.to_str().unwrap();
    let (code, stdout, stderr) = run_org(&["query", "columns", f, "--format", "json"]);
    assert_eq!(code, 0, "org query columns failed: {stderr}");
    let payload: Value = serde_json::from_str(&stdout).expect("valid columns JSON output");
    validate_against_schema(&payload, &schema, &schema).expect("columns JSON must validate");

    let mut invalid = payload;
    invalid["rows"][0]["unexpected_extra"] = Value::Bool(true);
    assert!(validate_against_schema(&invalid, &schema, &schema).is_err());
}

/// Verifies `schemas/org-clock-report-output.schema.json` defines `effort_minutes` and `variance_minutes` and validates `org clock report --format json`.
#[test]
fn slice3_clock_report_schema_includes_effort_and_variance_minutes_and_validates_payloads() {
    let raw_schema = read_repo_file("schemas/org-clock-report-output.schema.json");
    let schema: Value = serde_json::from_str(&raw_schema).expect("valid clock-report schema JSON");
    let summary_def = &schema["$defs"]["ClockEntrySummary"]["properties"];
    let nullable_int = serde_json::json!(["integer", "null"]);
    assert_eq!(summary_def["effort_minutes"]["type"], nullable_int);
    assert_eq!(summary_def["variance_minutes"]["type"], nullable_int);

    let temp = TempDir::new().expect("tempdir");
    let org_file = temp.path().join("clock.org");
    fs::write(
        &org_file,
        "* DONE Scoped Task\n:PROPERTIES:\n:EFFORT: 1:00\n:END:\n:LOGBOOK:\nCLOCK: [2026-03-01 Sun 09:00]--[2026-03-01 Sun 10:30] =>  1:30\n:END:\n",
    )
    .expect("write clock fixture");
    let f = org_file.to_str().unwrap();
    let (code, stdout, stderr) = run_org(&["clock", "report", f, "--format", "json"]);
    assert_eq!(code, 0, "org clock report failed: {stderr}");
    let payload: Value = serde_json::from_str(&stdout).expect("valid clock report JSON");
    assert_eq!(
        (
            payload[0]["effort_minutes"].as_i64(),
            payload[0]["variance_minutes"].as_i64()
        ),
        (Some(60), Some(30))
    );
    validate_against_schema(&payload, &schema, &schema).expect("clock report JSON must validate");

    let mut invalid = payload;
    invalid[0]["unexpected_field"] = Value::String("bad".to_string());
    assert!(validate_against_schema(&invalid, &schema, &schema).is_err());
}

/// Verifies `cargo doc --workspace --no-deps` completes cleanly with exit code `0`.
#[test]
fn slice3_cargo_doc_workspace_no_deps_succeeds() {
    let status = Command::new("cargo")
        .current_dir(workspace_root())
        .args(["doc", "--workspace", "--no-deps"])
        .status()
        .expect("run cargo doc --workspace --no-deps");
    assert!(
        status.success(),
        "cargo doc --workspace --no-deps must succeed"
    );
}

// endregion
