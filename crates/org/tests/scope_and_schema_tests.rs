// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural tests for scope queries, landscape expansion, and version JSON.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

#[path = "scope_and_schema_tests/schema_parity.rs"]
mod schema_parity;

/// Executes the compiled `org` binary with `args` in `cwd`.
pub(crate) fn run_org(args: &[&str], cwd: Option<&Path>) -> (i32, String, String) {
    let working_dir = cwd.unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")));
    let output = Command::new(env!("CARGO_BIN_EXE_org"))
        .current_dir(working_dir)
        .env("XDG_CONFIG_HOME", "/nonexistent")
        .env_remove("ORG_TOOLS_CONFIG")
        .env_remove("ORG_ROAM_DIRECTORIES")
        .args(args)
        .output()
        .expect("execute org binary");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (code, stdout, stderr)
}

/// Helper executing `org query search <extra_args...> <file> --format locator`.
fn run_query_loc(extra_args: &[&str], file_path: &str) -> (i32, String) {
    let mut cmd = vec!["query", "search"];
    cmd.extend_from_slice(extra_args);
    cmd.extend_from_slice(&[file_path, "--format", "locator"]);
    let (code, stdout, _) = run_org(&cmd, None);
    (code, stdout)
}

/// Creates an Org mode fixture file and TOML configuration in `dir`.
fn create_test_fixtures(dir: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(dir.join(".git")).expect("create .git marker");
    let org_path = dir.join("projects.org");
    let content = concat!(
        ":PROPERTIES:\n:ID:       11111111-1111-1111-1111-111111111111\n:SCOPE:    global\n:END:\n",
        "#+TITLE: Global Project Index\n\n",
        "* Alpha Core Task\n:PROPERTIES:\n:ID:       22222222-2222-2222-2222-222222222222\n:SCOPE:    project:alpha\n:END:\nAlpha tasks.\n\n",
        "* Java Fleet Task\n:PROPERTIES:\n:ID:       33333333-3333-3333-3333-333333333333\n:SCOPE:    landscape:java-fleet !project:legacy-billing\n:END:\nBackend service across java fleet.\n\n",
        "* Payment Gateway\n:PROPERTIES:\n:ID:       44444444-4444-4444-4444-444444444444\n:SCOPE:    forge:acme/payments/*\n:END:\nPayment webhook.\n\n",
        "* Generic Notes\nUnscoped entry inheriting global scope.\n"
    );
    fs::write(&org_path, content).expect("write org fixture");

    let cfg_path = dir.join("scope-config.toml");
    let cfg_content = concat!(
        "[landscapes.java-fleet]\ndescription = \"Java backend services\"\nprojects = [\"payment-service\", \"legacy-billing\"]\nforge_prefixes = [\"acme/payments/*\"]\n\n",
        "[landscapes.super-fleet]\ndescription = \"Composite fleet\"\nincludes = [\"java-fleet\"]\n"
    );
    fs::write(&cfg_path, cfg_content).expect("write toml config");
    (org_path, cfg_path)
}

/// Verifies basic scope filtering and `--strict-scope` behaviour on scoped and unscoped nodes.
#[test]
fn test_scope_basic_and_strict() {
    let temp = TempDir::new().expect("create temp dir");
    let (org_path, _) = create_test_fixtures(temp.path());
    let f = org_path.to_str().unwrap();

    let (c1, out1) = run_query_loc(&["", "--for-project", "alpha"], f);
    assert_eq!(c1, 0);
    assert!(out1.contains("id:11111111-1111-1111-1111-111111111111"));
    assert!(out1.contains("id:22222222-2222-2222-2222-222222222222"));
    assert!(out1.contains("Generic Notes"));

    let (c2, out2) = run_query_loc(&["", "--for-project", "alpha", "--strict-scope"], f);
    assert_eq!(c2, 0);
    assert!(!out2.contains("id:11111111-1111-1111-1111-111111111111"));
    assert!(out2.contains("id:22222222-2222-2222-2222-222222222222"));
    assert!(!out2.contains("Generic Notes"));
}

/// Verifies landscape expansion, exclusion negation, forge globs, and `--fts` integration.
#[test]
fn test_scope_landscape_expansion_and_negation() {
    let temp = TempDir::new().expect("create temp dir");
    let (org_path, cfg_path) = create_test_fixtures(temp.path());
    let f = org_path.to_str().unwrap();
    let cfg = cfg_path.to_str().unwrap();

    let (c3, out3) = run_query_loc(
        &[
            "",
            "--for-project",
            "payment-service",
            "--strict-scope",
            "--scope-config",
            cfg,
        ],
        f,
    );
    assert_eq!(c3, 0);
    assert!(out3.contains("id:33333333-3333-3333-3333-333333333333"));

    let (c4, _) = run_query_loc(
        &[
            "",
            "--for-project",
            "legacy-billing",
            "--strict-scope",
            "--scope-config",
            cfg,
        ],
        f,
    );
    assert_eq!(c4, 1, "negated project legacy-billing must be excluded");

    let (c5, out5) = run_query_loc(
        &[
            "",
            "--for-landscape",
            "super-fleet",
            "--strict-scope",
            "--scope-config",
            cfg,
        ],
        f,
    );
    assert_eq!(c5, 0);
    assert!(out5.contains("id:33333333-3333-3333-3333-333333333333"));
    assert!(out5.contains("id:44444444-4444-4444-4444-444444444444"));

    let (c6, out6) = run_query_loc(
        &[
            "",
            "--forge-path",
            "acme/payments/checkout",
            "--strict-scope",
        ],
        f,
    );
    assert_eq!(c6, 0);
    assert!(out6.contains("id:44444444-4444-4444-4444-444444444444"));

    let (c7, out7) = run_query_loc(
        &[
            "--fts",
            "backend",
            "--for-project",
            "payment-service",
            "--strict-scope",
            "--scope-config",
            cfg,
        ],
        f,
    );
    assert_eq!(c7, 0);
    assert!(out7.contains("id:33333333-3333-3333-3333-333333333333"));
}

/// Tests the exact spec scenario: billing-platform landscape with payments-api and idempotency FTS search.
#[test]
fn test_exact_spec_scenario_billing_platform_and_idempotency() {
    let temp = TempDir::new().expect("create temp dir");
    fs::create_dir_all(temp.path().join(".git")).expect("create .git marker");
    let org_path = temp.path().join("idempotency.org");
    let content = r#":PROPERTIES:
:ID:       11111111-aaaa-bbbb-cccc-111111111111
:SCOPE:    global
:END:
#+TITLE: Global Architecture Principles

* Idempotency in Distributed Systems
General idempotency guidelines and retry mechanisms.

* Billing Platform Idempotency
:PROPERTIES:
:ID:       22222222-aaaa-bbbb-cccc-222222222222
:SCOPE:    landscape:billing-platform
:END:
Strict idempotency keys for payment transactions and ledger workers.

* CLI Tool Idempotency
:PROPERTIES:
:ID:       33333333-aaaa-bbbb-cccc-333333333333
:SCOPE:    project:unrelated-cli
:END:
Local idempotency token cache for CLI commands.
"#;
    fs::write(&org_path, content).expect("write org fixture");

    let cfg_path = temp.path().join("kb.toml");
    let cfg_content = r#"
[landscapes.billing-platform]
description = "Billing and ledger microservices"
projects = ["payments-api", "ledger-worker"]
"#;
    fs::write(&cfg_path, cfg_content).expect("write kb.toml");

    let f = org_path.to_str().unwrap();
    let cfg = cfg_path.to_str().unwrap();

    let (c1, out1, _) = run_org(
        &[
            "query",
            "search",
            "--fts",
            "idempotency",
            "--for-project",
            "payments-api",
            "--scope-config",
            cfg,
            "--format",
            "json",
            f,
        ],
        None,
    );
    assert_eq!(c1, 0);
    let items: Vec<Value> = serde_json::from_str(&out1).expect("valid json output");
    assert_eq!(items.len(), 2, "expected 2 matching items, got {out1}");
    assert!(items.iter().any(
        |i| i["title"].as_str() == Some("Global Architecture Principles")
            || i["title"].as_str() == Some("Idempotency in Distributed Systems")
    ));
    assert!(items
        .iter()
        .any(|i| i["title"].as_str() == Some("Billing Platform Idempotency")));
    assert!(!items
        .iter()
        .any(|i| i["title"].as_str() == Some("CLI Tool Idempotency")));

    let (c2, out2, _) = run_org(
        &[
            "query",
            "search",
            "--fts",
            "idempotency",
            "--for-project",
            "payments-api",
            "--strict-scope",
            "--scope-config",
            cfg,
            "--format",
            "json",
            f,
        ],
        None,
    );
    assert_eq!(c2, 0);
    let items_strict: Vec<Value> = serde_json::from_str(&out2).expect("valid json output");
    assert_eq!(
        items_strict.len(),
        1,
        "expected 1 matching item, got {out2}"
    );
    assert_eq!(
        items_strict[0]["title"].as_str(),
        Some("Billing Platform Idempotency")
    );
}

/// Verifies auto-discovery resolves `kb.toml` landscapes when sibling and child `.org-tools.toml` exist.
#[test]
fn test_scope_discovery_merges_sibling_and_parent_kb_toml() {
    let temp = TempDir::new().expect("create temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join(".git")).expect("create .git marker");
    fs::write(
        root.join(".org-tools.toml"),
        "[format]\ncolumn_width = 80\n",
    )
    .expect("write root tools");
    fs::write(
        root.join("kb.toml"),
        "[landscapes.billing-platform]\nprojects = [\"payments-api\"]\n",
    )
    .expect("write kb.toml");

    let sub = root.join("notes");
    fs::create_dir_all(&sub).expect("create sub dir");
    fs::write(
        sub.join(".org-tools.toml"),
        "[lint]\nid_scope = \"vault\"\n",
    )
    .expect("write sub tools");

    let org_file = sub.join("billing.org");
    let content = concat!(
        ":PROPERTIES:\n:ID:       10101010-1010-1010-1010-101010101010\n:SCOPE:    landscape:billing-platform\n:END:\n#+TITLE: Billing Spec\n\n",
        "* Invoice Generator\n:PROPERTIES:\n:ID:       20202020-2020-2020-2020-202020202020\n:SCOPE:    project:unrelated-cli\n:END:\nCLI invoice generator.\n"
    );
    fs::write(&org_file, content).expect("write org file");

    let f = org_file.to_str().unwrap();
    let (c1, out1, _) = run_org(
        &[
            "query",
            "search",
            "--for-project",
            "payments-api",
            "--strict-scope",
            "--format",
            "json",
            f,
        ],
        Some(&sub),
    );
    assert_eq!(c1, 0);
    let items: Vec<Value> = serde_json::from_str(&out1).expect("valid json output");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"].as_str(), Some("Billing Spec"));
}

/// Verifies `org --version` human and `--version --format json` capability envelope.
#[test]
fn test_version_and_capabilities_json() {
    let (c1, out1, _) = run_org(&["--version"], None);
    assert_eq!(c1, 0);
    assert!(out1.starts_with("org "));

    let (c2, out2, _) = run_org(&["--version", "--format", "json"], None);
    assert_eq!(c2, 0);
    let val: Value = serde_json::from_str(&out2).expect("valid json version envelope");
    assert_eq!(val["name"].as_str(), Some("org"));
    assert_eq!(val["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));

    let caps = val["capabilities"].as_array().expect("capabilities array");
    for req in [
        "vault_id_scope",
        "add_id_auto_sync",
        "fts5_search",
        "scope_filter",
        "rich_show_digest",
    ] {
        assert!(
            caps.iter().any(|c| c.as_str() == Some(req)),
            "missing capability `{req}` in {out2}"
        );
    }
}
