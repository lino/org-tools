// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSON Schema Draft 2020-12 parity tests for CLI output contracts.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use tempfile::TempDir;

use super::run_org;

/// Resolves a local `#/$defs/<Key>` schema reference inside `root`.
fn resolve_ref<'a>(node: &'a Value, root: &'a Value) -> &'a Value {
    if let Some(target) = node.get("$ref").and_then(Value::as_str) {
        let key = target.strip_prefix("#/$defs/").expect("local #/$defs/ ref");
        return &root["$defs"][key];
    }
    node
}

/// Returns true when `instance` matches a primitive JSON Schema type name.
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

/// Validates object keys, required properties, and enforces `additionalProperties: false`.
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

/// Validates `instance` against a JSON Schema Draft 2020-12 definition.
fn validate_against_schema(instance: &Value, node: &Value, root: &Value) -> Result<(), String> {
    let schema = resolve_ref(node, root);
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        let matched = one_of
            .iter()
            .any(|sub| validate_against_schema(instance, sub, root).is_ok());
        if !matched {
            return Err(format!(
                "instance did not match any oneOf variant: {instance}"
            ));
        }
        return Ok(());
    }
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

/// Asserts `instance` conforms to `schema` and adding `key` fails validation.
pub(crate) fn assert_schema_valid_and_rejects_extra(instance: &Value, schema: &Value, key: &str) {
    validate_against_schema(instance, schema, schema).expect("valid payload conforms to schema");
    let mut bad = instance.clone();
    if let Some(arr) = bad.as_array_mut() {
        if let Some(first) = arr.first_mut().and_then(Value::as_object_mut) {
            first.insert(key.to_string(), Value::Bool(true));
        }
    } else if let Some(obj) = bad.as_object_mut() {
        obj.insert(key.to_string(), Value::Bool(true));
    }
    assert!(
        validate_against_schema(&bad, schema, schema).is_err(),
        "schema must reject undeclared property `{key}`"
    );
}

/// Creates an Org mode fixture file for schema parity testing in `dir`.
fn create_schema_fixture(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir.join(".git")).expect("create .git marker");
    let org_path = dir.join("schema_test.org");
    let content = concat!(
        ":PROPERTIES:\n:ID:       a1b2c3d4-e5f6-7890-abcd-ef1234567890\n:SCOPE:    global\n:END:\n#+TITLE: Schema Fixture\n\n",
        "* TODO Task Alpha :work:\n:PROPERTIES:\n:ID:       b2c3d4e5-f6a7-8901-bcde-f12345678901\n:SCOPE:    project:alpha\n:APPROVAL_DIGEST: sha256:1111111111111111\n:END:\n",
        "Body of task alpha with [[id:a1b2c3d4-e5f6-7890-abcd-ef1234567890][File Link]].\n- [ ] Checklist item 1\n"
    );
    fs::write(&org_path, content).expect("write schema fixture");
    org_path
}

/// Verifies JSON Schema Draft 2020-12 parity for `fmt` and `query` output commands.
#[test]
fn test_fmt_and_query_schemas() {
    let temp = TempDir::new().expect("create temp dir");
    let org_path = create_schema_fixture(temp.path());
    let f = org_path.to_str().unwrap();

    let (_, fmt_schema_raw, _) = run_org(&["schema", "fmt"], None);
    let fmt_schema: Value = serde_json::from_str(&fmt_schema_raw).expect("fmt schema");
    let (_, fmt_out, _) = run_org(
        &["fmt", "check", "--id-scope", "vault", "--format", "json", f],
        None,
    );
    let fmt_val: Value = serde_json::from_str(&fmt_out).expect("fmt json output");
    assert_schema_valid_and_rejects_extra(&fmt_val, &fmt_schema, "undeclared_field");

    let (_, query_schema_raw, _) = run_org(&["schema", "query"], None);
    let query_schema: Value = serde_json::from_str(&query_schema_raw).expect("query schema");
    let (_, query_out, _) = run_org(
        &[
            "query",
            "search",
            "--fts",
            "alpha",
            f,
            "--include",
            "body,vulpea_meta,checkboxes,links,digest",
            "--format",
            "json",
        ],
        None,
    );
    let query_val: Value = serde_json::from_str(&query_out).expect("query json output");
    assert_schema_valid_and_rejects_extra(&query_val, &query_schema, "extra_property");
}

/// Verifies JSON Schema Draft 2020-12 parity for `show` and `update` commands.
#[test]
fn test_show_and_update_schemas() {
    let temp = TempDir::new().expect("create temp dir");
    let org_path = create_schema_fixture(temp.path());
    let f = org_path.to_str().unwrap();
    let locator = format!("{f}::1");

    let (_, show_schema_raw, _) = run_org(&["schema", "show"], None);
    let show_schema: Value = serde_json::from_str(&show_schema_raw).expect("show schema");
    let (_, show_out, _) = run_org(
        &[
            "show",
            &locator,
            "--format",
            "json",
            "--exclude-properties",
            "APPROVAL_DIGEST",
        ],
        None,
    );
    let show_val: Value = serde_json::from_str(&show_out).expect("show json output");
    assert_schema_valid_and_rejects_extra(&show_val, &show_schema, "bogus_metric");

    let (_, update_schema_raw, _) = run_org(&["schema", "update"], None);
    let update_schema: Value = serde_json::from_str(&update_schema_raw).expect("update schema");
    let target_uuid = "c3d4e5f6-a7b8-9012-cdef-123456789012";
    let (_, link_out, _) = run_org(
        &[
            "update",
            "link",
            &locator,
            "--to",
            target_uuid,
            "--format",
            "json",
        ],
        None,
    );
    let link_val: Value = serde_json::from_str(&link_out).expect("link json output");
    assert_schema_valid_and_rejects_extra(&link_val, &update_schema, "bad_link_prop");

    let unassigned_path = temp.path().join("unassigned.org");
    fs::write(
        &unassigned_path,
        "#+TITLE: Plain Note\n\n* Plain Heading\nBody\n",
    )
    .expect("write unassigned");
    let u = unassigned_path.to_str().unwrap();
    let (_, add_id_out, _) = run_org(&["update", "add-id", "--file", "--format", "json", u], None);
    let add_id_val: Value = serde_json::from_str(&add_id_out).expect("add-id json output");
    assert_schema_valid_and_rejects_extra(&add_id_val, &update_schema, "bad_add_id_prop");
}

/// Verifies JSON Schema Draft 2020-12 parity for `roam` output commands.
#[test]
fn test_roam_schema() {
    let temp = TempDir::new().expect("create temp dir");
    let org_path = create_schema_fixture(temp.path());
    let f = org_path.to_str().unwrap();

    let (_, roam_schema_raw, _) = run_org(&["schema", "roam"], None);
    let roam_schema: Value = serde_json::from_str(&roam_schema_raw).expect("roam schema");
    let (_, roam_out, _) = run_org(&["roam", "node", "list", f, "--format", "json"], None);
    let roam_val: Value = serde_json::from_str(&roam_out).expect("roam json output");
    assert_schema_valid_and_rejects_extra(&roam_val, &roam_schema, "rogue_key");
}

/// Verifies `org roam node` scope filtering with `--for-project`, `--for-landscape`, and `--strict-scope`.
#[test]
fn test_roam_node_scope_filtering() {
    let temp = TempDir::new().expect("create temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join(".git")).expect("create .git marker");
    fs::write(
        root.join("kb.toml"),
        "[landscapes.billing-platform]\nprojects = [\"payments-api\"]\n",
    )
    .expect("write kb.toml");

    let org_file = root.join("roam_notes.org");
    let content = concat!(
        ":PROPERTIES:\n:ID:       11111111-2222-3333-4444-555555555555\n:SCOPE:    global\n:END:\n#+TITLE: Common Note\n\n",
        "* Common Note\n:PROPERTIES:\n:ID:       66666666-7777-8888-9999-000000000000\n:SCOPE:    landscape:billing-platform\n:END:\nCore billing logic.\n\n",
        "* Common Note\n:PROPERTIES:\n:ID:       aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee\n:SCOPE:    project:other-cli\n:END:\nCLI commands.\n"
    );
    fs::write(&org_file, content).expect("write roam notes");

    let (_, roam_schema_raw, _) = run_org(&["schema", "roam"], None);
    let roam_schema: Value = serde_json::from_str(&roam_schema_raw).expect("roam schema");
    let f = org_file.to_str().unwrap();
    let r = root.to_str().unwrap();

    let (c1, out1, _) = run_org(
        &[
            "roam",
            "node",
            "Common Note",
            f,
            "--workspace",
            r,
            "--for-project",
            "payments-api",
            "--format",
            "json",
        ],
        Some(root),
    );
    assert_eq!(c1, 0, "expected exit 0, got {c1}: {out1}");
    let val1: Value = serde_json::from_str(&out1).expect("valid roam json");
    assert_schema_valid_and_rejects_extra(&val1, &roam_schema, "extra_field");
    let nodes1 = val1["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes1.len(), 2, "expected 2 nodes, got {out1}");
    assert!(nodes1
        .iter()
        .any(|n| n["id"] == "11111111-2222-3333-4444-555555555555"));
    assert!(nodes1
        .iter()
        .any(|n| n["id"] == "66666666-7777-8888-9999-000000000000"));

    let (c2, out2, _) = run_org(
        &[
            "roam",
            "node",
            "Common Note",
            f,
            "--workspace",
            r,
            "--for-project",
            "payments-api",
            "--strict-scope",
            "--format",
            "json",
        ],
        Some(root),
    );
    assert_eq!(c2, 0, "expected exit 0, got {c2}: {out2}");
    let val2: Value = serde_json::from_str(&out2).expect("valid roam json");
    assert_schema_valid_and_rejects_extra(&val2, &roam_schema, "extra_field");
    let nodes2 = val2["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes2.len(), 1, "expected 1 node, got {out2}");
    assert_eq!(nodes2[0]["id"], "66666666-7777-8888-9999-000000000000");

    let (c3, out3, _) = run_org(
        &[
            "roam",
            "node",
            "Common Note",
            f,
            "--workspace",
            r,
            "--for-landscape",
            "billing-platform",
            "--strict-scope",
            "--format",
            "json",
        ],
        Some(root),
    );
    assert_eq!(c3, 0, "expected exit 0, got {c3}: {out3}");
    let val3: Value = serde_json::from_str(&out3).expect("valid roam json");
    let nodes3 = val3["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes3.len(), 1);
    assert_eq!(nodes3[0]["id"], "66666666-7777-8888-9999-000000000000");

    let (c4, out4, _) = run_org(
        &[
            "roam",
            "node",
            "Common Note",
            f,
            "--workspace",
            r,
            "--for-project",
            "other-cli",
            "--strict-scope",
            "--format",
            "json",
        ],
        Some(root),
    );
    assert_eq!(c4, 0, "expected exit 0, got {c4}: {out4}");
    let val4: Value = serde_json::from_str(&out4).expect("valid roam json");
    let nodes4 = val4["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes4.len(), 1);
    assert_eq!(nodes4[0]["id"], "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
}
