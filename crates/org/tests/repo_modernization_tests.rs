// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural and structural contract tests for repository modernization (`ORGFMT-WP-08`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// region Constants & Path Helpers

/// Maximum allowed line count per modularized CLI source file in `crates/org/src/`.
const MAX_CLI_SOURCE_LINES: usize = 450;

/// Exact line count required for the `CLAUDE.md` pointer file.
const EXPECTED_CLAUDE_MD_LINES: usize = 3;

/// Forbidden machine-specific path prefix across committed configuration and docs.
const FORBIDDEN_USER_PATH_PREFIX: &str = "/Users/";

/// Required modular command source files inside `crates/org/src/commands/`.
const REQUIRED_COMMAND_MODULES: &[&str] = &[
    "mod.rs",
    "fmt.rs",
    "query.rs",
    "update.rs",
    "clock.rs",
    "archive.rs",
    "export.rs",
    "schema.rs",
];

/// Required standard lifecycle tasks in `mise.toml`.
const REQUIRED_MISE_TASKS: &[&str] = &[
    "setup", "fmt", "lint", "test", "verify", "docs", "bench", "metrics",
];

/// Returns the workspace root directory derived from the `org` crate manifest path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/org must reside two levels beneath the workspace root")
        .to_path_buf()
}

/// Reads a UTF-8 text file relative to the workspace root.
fn read_repo_file(rel_path: &str) -> String {
    let full = workspace_root().join(rel_path);
    fs::read_to_string(&full).unwrap_or_else(|err| {
        panic!(
            "expected {} to exist and be readable: {err}",
            full.display()
        )
    })
}

/// Asserts that `content` contains zero hardcoded `/Users/` machine paths.
fn assert_no_hardcoded_user_paths(rel_path: &str, content: &str) {
    assert!(
        !content.contains(FORBIDDEN_USER_PATH_PREFIX),
        "{rel_path} must not contain hardcoded '{FORBIDDEN_USER_PATH_PREFIX}' paths"
    );
}

// endregion

// region Slice 1: Modular CLI Split & Craft Bounds

/// Verifies `main.rs` and all `commands/*.rs` modules exist, carry SPDX/module docs, and stay `<= 450` lines.
#[test]
fn cli_main_and_command_modules_exist_within_line_budget() {
    let src_dir = workspace_root().join("crates/org/src");
    let main_rs = src_dir.join("main.rs");
    let main_content = fs::read_to_string(&main_rs).expect("main.rs must exist");
    let main_lines = main_content.lines().count();
    assert!(
        main_lines <= MAX_CLI_SOURCE_LINES,
        "crates/org/src/main.rs has {main_lines} lines (exceeds {MAX_CLI_SOURCE_LINES})"
    );

    let commands_dir = src_dir.join("commands");
    assert!(
        commands_dir.is_dir(),
        "crates/org/src/commands/ directory must exist"
    );

    for required in REQUIRED_COMMAND_MODULES {
        let mod_path = commands_dir.join(required);
        assert!(
            mod_path.is_file(),
            "missing required command module {}",
            mod_path.display()
        );
    }

    for entry in fs::read_dir(&commands_dir).expect("read commands directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).expect("read command module");
        let line_count = content.lines().count();
        assert!(
            line_count <= MAX_CLI_SOURCE_LINES,
            "{} has {line_count} lines (exceeds {MAX_CLI_SOURCE_LINES})",
            path.display()
        );
        assert!(
            content.contains("SPDX-License-Identifier: GPL-3.0-or-later"),
            "{} must carry SPDX GPL-3.0-or-later header",
            path.display()
        );
        assert!(
            content.lines().any(|line| line.starts_with("//!")),
            "{} must carry a //! module docstring",
            path.display()
        );
    }
}

// endregion

// region Slice 2: Self-Dogfood Org Documentation Gate

/// Verifies `org fmt check README.org docs/` exits 0 with zero diagnostics.
#[test]
fn org_fmt_check_self_dogfood_readme_and_docs_is_clean() {
    let output = Command::new(env!("CARGO_BIN_EXE_org"))
        .current_dir(workspace_root())
        .args(["fmt", "check", "README.org", "docs/"])
        .output()
        .expect("failed to execute org fmt check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "org fmt check README.org docs/ exited {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );
    assert!(
        stdout.trim().is_empty(),
        "expected zero diagnostics on stdout, got:\n{stdout}"
    );
}

/// Verifies CI workflow and pre-commit hooks enforce workspace clippy and self-dogfood `org fmt check`.
#[test]
fn ci_and_pre_commit_enforce_all_targets_clippy_and_org_fmt_check() {
    let ci_yml = read_repo_file(".github/workflows/ci.yml");
    assert_no_hardcoded_user_paths(".github/workflows/ci.yml", &ci_yml);
    assert!(
        ci_yml.contains("cargo clippy --all-targets -- -D warnings"),
        "ci.yml must run cargo clippy --all-targets -- -D warnings"
    );
    assert!(
        ci_yml.contains("fmt check README.org docs/"),
        "ci.yml must self-dogfood org fmt check README.org docs/"
    );

    let pre_commit = read_repo_file(".pre-commit-config.yaml");
    assert_no_hardcoded_user_paths(".pre-commit-config.yaml", &pre_commit);
    assert!(
        pre_commit.contains("cargo clippy --all-targets -- -D warnings"),
        ".pre-commit-config.yaml must run cargo clippy --all-targets -- -D warnings"
    );
    assert!(
        pre_commit.contains("cargo run --quiet -- fmt check"),
        ".pre-commit-config.yaml must run cargo run --quiet -- fmt check"
    );
    assert!(
        !pre_commit.contains("|| true"),
        ".pre-commit-config.yaml must not swallow hook failures with || true"
    );
}

// endregion

// region Slice 3: AGENTS.md, CLAUDE.md, Tooling Configs & Refreshed Docs

/// Verifies `AGENTS.md` is agent-neutral and `CLAUDE.md` is an exact 3-line pointer.
#[test]
fn agents_md_and_three_line_claude_pointer_meet_contract() {
    let agents = read_repo_file("AGENTS.md");
    assert_no_hardcoded_user_paths("AGENTS.md", &agents);
    for needle in [
        "GPL-3.0-or-later",
        "crates/org/src/commands/",
        "https://orgmode.org/worg/org-syntax.html",
        "EXIT_OK",
        "EXIT_ISSUES",
        "EXIT_ERROR",
        "EXIT_REQUIRES_EMACS",
        ".regions.md",
        "mise run verify",
    ] {
        assert!(
            agents.contains(needle),
            "AGENTS.md missing expected section/token '{needle}'"
        );
    }

    let claude = read_repo_file("CLAUDE.md");
    assert_no_hardcoded_user_paths("CLAUDE.md", &claude);
    assert_eq!(
        claude.lines().count(),
        EXPECTED_CLAUDE_MD_LINES,
        "CLAUDE.md must be an exact 3-line pointer to AGENTS.md"
    );
    assert!(
        claude.contains("AGENTS.md"),
        "CLAUDE.md must point readers to AGENTS.md"
    );
}

/// Verifies `.fritz.toml` defines hub workspace, active Rust rulesets, hygiene, and observability.
#[test]
fn fritz_toml_defines_orgfmt_hub_workspace_and_rulesets() {
    let raw = read_repo_file(".fritz.toml");
    assert_no_hardcoded_user_paths(".fritz.toml", &raw);
    let parsed: toml::Table = raw.parse().expect(".fritz.toml must be valid TOML");

    let ws = parsed["workspace"]
        .as_table()
        .expect("[workspace] table required");
    assert_eq!(ws["mode"].as_str(), Some("hub"));
    assert_eq!(ws["project"].as_str(), Some("orgfmt"));
    assert_eq!(ws["backend"].as_str(), Some("metrorg"));
    assert_eq!(ws["hub_env_var"].as_str(), Some("BABELSBERG_HUB"));
    assert_eq!(ws["local_plan_dir"].as_str(), Some(".plan"));
    assert_eq!(ws["metrorg_bin_env_var"].as_str(), Some("METRORG_BIN"));
    assert_eq!(ws["default_metrorg_bin"].as_str(), Some("org"));

    let active = parsed["rulesets"]["active"]
        .as_array()
        .expect("[rulesets].active array required");
    let active_strs: Vec<&str> = active.iter().filter_map(|v| v.as_str()).collect();
    assert!(active_strs.contains(&"local:baseline/core-craft@1.1.0"));
    assert!(active_strs.contains(&"local:stack/rust-cli@1.0.0"));

    assert!(parsed.get("models").and_then(|v| v.as_table()).is_some());
    assert!(parsed.get("hygiene").and_then(|v| v.as_table()).is_some());
    assert_eq!(
        parsed["observability"]["namespace"].as_str(),
        Some("orgfmt")
    );
}

/// Verifies `mise.toml` and `cliff.toml` define standard tasks and conventional changelog settings.
#[test]
fn mise_toml_and_cliff_toml_define_standard_tasks_and_changelog() {
    let mise_raw = read_repo_file("mise.toml");
    assert_no_hardcoded_user_paths("mise.toml", &mise_raw);
    let mise: toml::Table = mise_raw.parse().expect("mise.toml must be valid TOML");
    assert_eq!(mise["tools"]["rust"].as_str(), Some("stable"));

    let tasks = mise["tasks"].as_table().expect("[tasks] table required");
    for task_name in REQUIRED_MISE_TASKS {
        assert!(
            tasks.contains_key(*task_name),
            "mise.toml missing required task '{task_name}'"
        );
    }
    let verify_cmd = tasks["verify"]["run"]
        .as_str()
        .expect("[tasks.verify].run string required");
    for gate in [
        "cargo fmt -- --check",
        "cargo clippy --all-targets -- -D warnings",
        "cargo test",
        "fmt check README.org docs/",
    ] {
        assert!(
            verify_cmd.contains(gate),
            "[tasks.verify] must run '{gate}'"
        );
    }

    let cliff_raw = read_repo_file("cliff.toml");
    assert_no_hardcoded_user_paths("cliff.toml", &cliff_raw);
    let cliff: toml::Table = cliff_raw.parse().expect("cliff.toml must be valid TOML");
    assert!(cliff.contains_key("changelog"));
    assert!(cliff.contains_key("git"));
    assert_eq!(cliff["git"]["conventional_commits"].as_bool(), Some(true));
}

/// Verifies `architecture.org` and `release.org` reflect modular commands, data models, and GoReleaser ops.
#[test]
fn architecture_and_release_docs_are_refreshed() {
    let arch = read_repo_file("docs/technical/architecture.org");
    assert_no_hardcoded_user_paths("docs/technical/architecture.org", &arch);
    for needle in [
        "commands/mod.rs",
        "commands/update.rs",
        "property.rs",
        "org-roam",
        "ChecklistItem",
        "MutationResult",
        "telemetry",
        "GoReleaser",
    ] {
        assert!(
            arch.contains(needle),
            "architecture.org missing expected refreshed content '{needle}'"
        );
    }

    let release = read_repo_file("docs/operations/release.org");
    assert_no_hardcoded_user_paths("docs/operations/release.org", &release);
    for needle in [
        "scripts/release.sh",
        "cliff.toml",
        "fmt check README.org docs/",
        "GoReleaser",
        "homebrew-tap",
        "CARGO_REGISTRY_TOKEN",
        "publish-container",
        "ghcr.io/lino/org-tools",
    ] {
        assert!(
            release.contains(needle),
            "release.org missing expected refreshed content '{needle}'"
        );
    }
}

/// Verifies `Dockerfile`, `.dockerignore`, and GHCR container jobs in `ci.yml` and `release.yml`.
#[test]
fn dockerfile_and_ghcr_workflows_provide_zero_toolchain_container_runtime() {
    let dockerignore = read_repo_file(".dockerignore");
    assert_no_hardcoded_user_paths(".dockerignore", &dockerignore);
    assert!(dockerignore.contains("SPDX-License-Identifier: GPL-3.0-or-later"));
    for ignored in ["target/", "dist/", ".git/", ".cache/"] {
        assert!(
            dockerignore.lines().any(|l| l.trim() == ignored),
            ".dockerignore must exclude `{ignored}`"
        );
    }

    let dockerfile = read_repo_file("Dockerfile");
    assert_no_hardcoded_user_paths("Dockerfile", &dockerfile);
    for token in [
        "SPDX-License-Identifier: GPL-3.0-or-later",
        "cargo build --release --locked -p org-tools",
        "scripts/build-completions.sh",
        "ENV ORG_TOOLS_WORKSPACE=/workspace",
        "WORKDIR /workspace",
        "ENTRYPOINT [\"org\"]",
    ] {
        assert!(
            dockerfile.contains(token),
            "Dockerfile missing required directive `{token}`"
        );
    }

    let ci_yml = read_repo_file(".github/workflows/ci.yml");
    for token in [
        "packages: write",
        "ghcr.io/${{ github.repository }}",
        "docker/build-push-action@v6",
        "docker run --rm org-tools:ci --version",
    ] {
        assert!(ci_yml.contains(token), "ci.yml missing `{token}`");
    }

    let release_yml = read_repo_file(".github/workflows/release.yml");
    for token in [
        "packages: write",
        "publish-container:",
        "platforms: linux/amd64,linux/arm64",
        "docker/build-push-action@v6",
    ] {
        assert!(release_yml.contains(token), "release.yml missing `{token}`");
    }
}

// endregion
