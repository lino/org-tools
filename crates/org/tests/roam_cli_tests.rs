// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural CLI, JSON Schema, and lint integration tests for `org roam` (`ORGFMT-WP-04`).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

// region Constants & Path Helpers

/// Deterministic Stempelamt UUIDv5 for `urn:babelsberg:fritz:wp:fritz-wp-01`.
const GOLDEN_WP_ID: &str = "cbba66ec-baff-5eb9-b0d3-4e38f56ebcd9";

/// Canonical `:ID_SEED:` URN producing [`GOLDEN_WP_ID`].
const GOLDEN_WP_SEED: &str = "urn:babelsberg:fritz:wp:fritz-wp-01";

/// Sample file-level UUID for Babelsberg spec fixture.
const SPEC_FILE_ID: &str = "f0000000-0000-4000-8000-000000000012";

/// Sample heading UUID for `SPEC-ORGFMT-01` fixture.
const SPEC_HEADING_ID: &str = "20000000-0000-4000-8000-000000000010";

/// Sample heading UUID for `ADR-0005` fixture.
const ADR_HEADING_ID: &str = "20000000-0000-4000-8000-000000000021";

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
fn run_ok_stdout(args: &[&str]) -> String {
    let out = org()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).expect("valid UTF-8 stdout")
}

/// Seeds a Babelsberg-shaped multi-file `org-roam` v2 workspace inside `dir`.
fn seed_babelsberg_workspace(dir: &Path) {
    fs::write(dir.join("kb.toml"), "[workspace]\nname = \"test\"\n").unwrap();
    write_org(
        &dir.join("specs/agent.org"),
        &format!(
            concat!(
                ":PROPERTIES:\n",
                ":ID:           {spec_file}\n",
                ":ROAM_ALIASES: \"SPEC-ORGFMT-01\" \"Agent Spec\"\n",
                ":ROAM_REFS:    https://orgmode.org\n",
                ":END:\n",
                "#+TITLE: Agent Engine Spec\n",
                "#+FILETAGS: :orgfmt:spec:\n\n",
                "* SPEC-ORGFMT-01: Agent Engine\n",
                ":PROPERTIES:\n",
                ":ID:        {spec_head}\n",
                ":CUSTOM_ID: SPEC-ORGFMT-01\n",
                ":END:\n",
                "Implements =[[id:<uuid>]]= parsing per [[id:{adr_head}][ADR-0005]].\n",
                "#+begin_src org\n[[id:fake-in-src]]\n#+end_src\n",
            ),
            spec_file = SPEC_FILE_ID,
            spec_head = SPEC_HEADING_ID,
            adr_head = ADR_HEADING_ID,
        ),
    );
    write_org(
        &dir.join("adr/adr-0005.org"),
        &format!(
            concat!(
                ":PROPERTIES:\n",
                ":ID:           {wp_id}\n",
                ":ID_SEED:      {wp_seed}\n",
                ":ROAM_EXCLUDE: t\n",
                ":END:\n",
                "#+TITLE: Excluded Wrapper\n\n",
                "* ADR-0005: Pure-Rust Roam <Graph>\n",
                ":PROPERTIES:\n",
                ":ID:           {adr_head}\n",
                ":ROAM_ALIASES: \"ADR-ORGFMT-0005\"\n",
                ":END:\n",
                "** Consequences\n",
                "Satisfies [[id:{spec_file}][Spec File]] and [[id:00000000-0000-4000-8000-deadbeef0000][Missing]].\n",
            ),
            wp_id = GOLDEN_WP_ID,
            wp_seed = GOLDEN_WP_SEED,
            adr_head = ADR_HEADING_ID,
            spec_file = SPEC_FILE_ID,
        ),
    );
}

// endregion

// region JSON Schema Contract Validator

/// Validates `instance` against a JSON Schema node inside `root_schema`.
fn validate_schema_node(
    instance: &Value,
    schema: &Value,
    root_schema: &Value,
) -> Result<(), String> {
    if let Some(ref_path) = schema.get("$ref").and_then(Value::as_str) {
        let def_name = ref_path
            .strip_prefix("#/$defs/")
            .ok_or("unsupported $ref")?;
        let target = &root_schema["$defs"][def_name];
        return validate_schema_node(instance, target, root_schema);
    }
    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = variants
            .iter()
            .filter(|v| validate_schema_node(instance, v, root_schema).is_ok())
            .count();
        return (matches == 1)
            .then_some(())
            .ok_or_else(|| format!("expected exactly 1 oneOf match, got {matches}"));
    }
    validate_type_and_const(instance, schema)?;
    validate_object_or_array(instance, schema, root_schema)
}

/// Checks `type` and `const` constraints on `instance`.
fn validate_type_and_const(instance: &Value, schema: &Value) -> Result<(), String> {
    if let Some(expected_const) = schema.get("const") {
        if instance != expected_const {
            return Err(format!("const mismatch: {instance} != {expected_const}"));
        }
    }
    if let Some(ty) = schema.get("type") {
        let allowed: Vec<&str> = match ty {
            Value::String(s) => vec![s.as_str()],
            Value::Array(arr) => arr.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !allowed.iter().any(|t| value_matches_type(instance, t)) {
            return Err(format!(
                "type mismatch for {instance}: expected {allowed:?}"
            ));
        }
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_i64) {
        if instance.as_i64().is_some_and(|n| n < min) {
            return Err(format!("{instance} < minimum {min}"));
        }
    }
    Ok(())
}

/// Returns `true` when `val` matches the JSON Schema primitive `type_name`.
fn value_matches_type(val: &Value, type_name: &str) -> bool {
    match type_name {
        "null" => val.is_null(),
        "boolean" => val.is_boolean(),
        "integer" => val.is_i64() || val.is_u64(),
        "string" => val.is_string(),
        "array" => val.is_array(),
        "object" => val.is_object(),
        _ => false,
    }
}

/// Checks `required` and `additionalProperties: false` constraints on a JSON object.
fn check_object_key_bounds(
    obj: &serde_json::Map<String, Value>,
    schema: &Value,
) -> Result<(), String> {
    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for key in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(format!("missing required key '{key}'"));
            }
        }
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for key in obj.keys() {
                if !props.contains_key(key) {
                    return Err(format!("unexpected property '{key}'"));
                }
            }
        }
    }
    Ok(())
}

/// Validates object `properties` and array `items` recursively.
fn validate_object_or_array(
    instance: &Value,
    schema: &Value,
    root_schema: &Value,
) -> Result<(), String> {
    if let Some(obj) = instance.as_object() {
        check_object_key_bounds(obj, schema)?;
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (k, v) in obj {
                if let Some(prop_schema) = props.get(k) {
                    validate_schema_node(v, prop_schema, root_schema)?;
                }
            }
        }
    }
    if let (Some(arr), Some(item_schema)) = (instance.as_array(), schema.get("items")) {
        for item in arr {
            validate_schema_node(item, item_schema, root_schema)?;
        }
    }
    Ok(())
}

/// Loads `org schema roam` and validates `json_str` against `org-roam-output.schema.json`.
fn parse_and_validate_roam_json(json_str: &str) -> Value {
    let schema_raw = run_ok_stdout(&["schema", "roam"]);
    let schema: Value = serde_json::from_str(&schema_raw).expect("valid schema JSON");
    let value: Value = serde_json::from_str(json_str).expect("valid command JSON");
    validate_schema_node(&value, &schema, &schema).expect("must conform to org-roam-output schema");
    let files_scanned = value["telemetry"]["files_scanned"].as_u64().unwrap();
    let cache_hits = value["telemetry"]["cache_hits"].as_u64().unwrap();
    let cache_misses = value["telemetry"]["cache_misses"].as_u64().unwrap();
    assert!(files_scanned >= 1);
    assert_eq!(cache_hits + cache_misses, files_scanned);
    value
}

/// Executes `org roam <sub> <arg> --workspace <ws> --format json` and validates against schema.
fn run_roam_json(sub: &str, arg: &str, ws: &str) -> Value {
    let out = run_ok_stdout(&["roam", sub, arg, "--workspace", ws, "--format", "json"]);
    parse_and_validate_roam_json(&out)
}

// endregion

// region Slice 2: org roam Subcommands, Schema & Completions

/// Verifies `org roam node` resolves ID, `id:` prefix, title, and multi-word aliases, exiting 1 on miss.
#[test]
fn roam_node_resolves_id_title_alias_and_exits_one_when_missing() {
    let dir = TempDir::new().unwrap();
    seed_babelsberg_workspace(dir.path());
    let ws = dir.path().to_str().unwrap();

    for query in [
        "SPEC-ORGFMT-01",
        "agent spec",
        "Agent Engine Spec",
        SPEC_FILE_ID,
        &format!("id:{SPEC_FILE_ID}"),
    ] {
        let val = run_roam_json("node", query, ws);
        assert_eq!(val["kind"], "node");
        assert_eq!(val["nodes"][0]["id"], SPEC_FILE_ID);
    }

    let human = run_ok_stdout(&["roam", "node", "ADR-ORGFMT-0005", ws]);
    assert!(human.contains(ADR_HEADING_ID));

    org()
        .args(["roam", "node", "DOES-NOT-EXIST", "--workspace", ws])
        .assert()
        .code(1);
}

/// Verifies `org roam backlinks` and `org roam links` resolve aliases/locators and validate against schema.
#[test]
fn roam_backlinks_and_links_resolve_targets_locators_and_empty_sets() {
    let dir = TempDir::new().unwrap();
    seed_babelsberg_workspace(dir.path());
    let ws = dir.path().to_str().unwrap();

    let bl_json = run_roam_json("backlinks", "ADR-ORGFMT-0005", ws);
    assert_eq!(bl_json["kind"], "backlinks");
    assert_eq!(bl_json["backlinks"].as_array().unwrap().len(), 1);
    assert_eq!(bl_json["backlinks"][0]["source_id"], SPEC_HEADING_ID);
    assert_eq!(bl_json["backlinks"][0]["description"], "ADR-0005");

    let empty_json = run_roam_json("backlinks", "specs/agent.org::#SPEC-ORGFMT-01", ws);
    assert!(empty_json["backlinks"].as_array().unwrap().is_empty());

    let links_json = run_roam_json("links", &format!("id:{ADR_HEADING_ID}"), ws);
    assert_eq!(links_json["kind"], "links");
    let links = links_json["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0]["target_id"], SPEC_FILE_ID);
    assert_eq!(links[0]["resolved"], true);
    assert_eq!(links[1]["resolved"], false);

    org()
        .args(["roam", "backlinks", "id:unknown", "--workspace", ws])
        .assert()
        .code(1);
    org()
        .args(["roam", "links", "id:unknown", "--workspace", ws])
        .assert()
        .code(1);
}

/// Verifies `org roam graph` exports JSON, Mermaid, and DOT, discovers `kb.toml`, and registers schema/completions.
#[test]
fn roam_graph_exports_json_mermaid_dot_and_supports_discovery_and_completions() {
    let dir = TempDir::new().unwrap();
    seed_babelsberg_workspace(dir.path());
    let ws = dir.path().to_str().unwrap();

    let graph_raw = String::from_utf8(
        org()
            .current_dir(dir.path().join("specs"))
            .args(["roam", "graph", "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    let graph_val = parse_and_validate_roam_json(&graph_raw);
    assert_eq!(graph_val["kind"], "graph");
    assert_eq!(graph_val["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(graph_val["edges"].as_array().unwrap().len(), 3);

    let mermaid = run_ok_stdout(&["roam", "graph", ws, "--format", "mermaid"]);
    assert!(mermaid.starts_with("flowchart LR\n"));
    assert!(mermaid.contains("N0[\""));
    assert!(!mermaid.contains("<Graph>"));
    assert!(mermaid.contains("-->"));

    let dot = run_ok_stdout(&["roam", "graph", ws, "--format", "dot"]);
    assert!(dot.starts_with("digraph org_roam {\n"));
    assert!(dot.contains("->"));

    assert!(run_ok_stdout(&["schema", "org-roam-output"]).contains("org-roam-output.schema.json"));
    org()
        .args(["--completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("roam"));
}

/// Verifies `--format mermaid` and `--format dot` cleanly render quotes, parens, brackets, and trailing `\`.
#[test]
fn roam_graph_mermaid_and_dot_escape_special_chars_and_trailing_backslash() {
    let dir = TempDir::new().unwrap();
    write_org(
        &dir.path().join("special.org"),
        concat!(
            ":PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000001\n:END:\n",
            "#+TITLE: Spec \"v2\" (Core) [Draft] C:\\\n",
        ),
    );
    let ws = dir.path().to_str().unwrap();

    let mermaid = run_ok_stdout(&["roam", "graph", ws, "--format", "mermaid"]);
    assert!(
        mermaid.contains("N0[\"Spec #quot;v2#quot; (Core) [Draft] C:#92;\"]"),
        "unexpected mermaid output: {mermaid}"
    );
    assert!(!mermaid.contains("\\\"]"));

    let dot = run_ok_stdout(&["roam", "graph", ws, "--format", "dot"]);
    assert!(
        dot.contains("N0 [label=\"Spec \\\"v2\\\" (Core) [Draft] C:\\\\\"];"),
        "unexpected dot output: {dot}"
    );
}

// endregion

// region Slice 3: Cross-File ID & Link Lints in org fmt check

/// Writes two colliding/mismatched Org fixtures into `dir` for cross-file lint verification.
fn seed_lint_fixture_dir(dir: &Path) {
    let one = format!(
        ":PROPERTIES:\n:ID:      {GOLDEN_WP_ID}\n:ID_SEED: {GOLDEN_WP_SEED}\n:END:\n\n#+TITLE: One\n\n* Task\n:PROPERTIES:\n:ID:      00000000-0000-5000-8000-000000000099\n:ID_SEED: urn:babelsberg:orgfmt:jira:orgfmt-101\n:END:\n\nSee =[[id:not-a-link]]= and [[id:missing-uuid][Broken]].\n"
    );
    write_org(&dir.join("one.org"), &one);
    write_org(
        &dir.join("two.org"),
        &format!("* Copy\n:PROPERTIES:\n:ID: {GOLDEN_WP_ID}\n:END:\n\nBody.\n"),
    );
}

/// Verifies `org fmt check` reports `E009` (`duplicate-id`), `E010` (`invalid-id-seed`), and `W037` (`broken-id-link`).
#[test]
fn fmt_check_reports_duplicate_id_invalid_seed_and_broken_id_link() {
    let dir = TempDir::new().unwrap();
    seed_lint_fixture_dir(dir.path());
    let out = org()
        .args([
            "fmt",
            "check",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let diags: Vec<Value> = serde_json::from_slice(&out).expect("valid diagnostic JSON");

    let e009: Vec<&Value> = diags.iter().filter(|d| d["rule_id"] == "E009").collect();
    let e010: Vec<&Value> = diags.iter().filter(|d| d["rule_id"] == "E010").collect();
    let w037: Vec<&Value> = diags.iter().filter(|d| d["rule_id"] == "W037").collect();

    assert_eq!(e009.len(), 2);
    assert_eq!(e010.len(), 1);
    assert_eq!(w037.len(), 1);
    assert_eq!(e009[0]["severity"], "error");
    assert_eq!(e010[0]["severity"], "error");
    assert_eq!(w037[0]["severity"], "warning");
}

// endregion
