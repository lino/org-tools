// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=920 reason="Single authorized Cargo integration test target bundling pure-Rust YAML subset parser and ORGFMT-WP-10/WP-11 CI and release workflow TDD slices without modifying Cargo.toml"

//! Behavioural and structural tests for the GitHub Actions release pipeline, crates.io gate, `scripts/release.sh --dry-run`, and release documentation.

use serde_json::{Map, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::process::Output;
#[cfg(unix)]
use tempfile::TempDir;

// region Constants & Path Helpers

/// Expected default GoReleaser OSS v2 version configured for the release pipeline.
const EXPECTED_GORELEASER_DEFAULT: &str = "v2.18.0";

/// Expected notice emitted when `CARGO_REGISTRY_TOKEN` is absent during release runs.
const EXPECTED_SKIP_CRATES_NOTICE: &str =
    "::notice::CARGO_REGISTRY_TOKEN is not configured; skipping crates.io publish.";

/// All six Rust cross-compilation target triples required by the release workflow.
const EXPECTED_RUST_TARGETS: [&str; 6] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

/// Returns the workspace root directory derived from the `org` crate manifest path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/org must reside two levels beneath the workspace root")
        .to_path_buf()
}

/// Reads a workspace-relative file into a UTF-8 string or panics with path context.
fn read_workspace_file(rel_path: &str) -> String {
    let full_path = workspace_root().join(rel_path);
    fs::read_to_string(&full_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", full_path.display()))
}

/// Loads and parses `.github/workflows/release.yml` into a `serde_json::Value` tree.
fn load_release_workflow() -> (String, Value) {
    let raw = read_workspace_file(".github/workflows/release.yml");
    let parsed = parse_yaml_document(&raw);
    (raw, parsed)
}

// endregion

// region Minimal YAML Subset Parser

/// Parses an indentation-structured YAML subset document into a `serde_json::Value`.
fn parse_yaml_document(input: &str) -> Value {
    let lines: Vec<&str> = input.lines().collect();
    let mut index = 0;
    parse_yaml_block(&lines, &mut index, 0)
}

/// Advances past blank and comment-only lines, returning the next `(line_index, indent)`.
fn next_meaningful_line(lines: &[&str], start: usize) -> Option<(usize, usize)> {
    let mut idx = start;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            let indent = lines[idx].len() - lines[idx].trim_start_matches(' ').len();
            return Some((idx, indent));
        }
        idx += 1;
    }
    None
}

/// Parses either a YAML sequence or mapping starting at or beyond `min_indent`.
fn parse_yaml_block(lines: &[&str], idx: &mut usize, min_indent: usize) -> Value {
    let Some((next_idx, indent)) = next_meaningful_line(lines, *idx) else {
        return Value::Null;
    };
    if indent < min_indent {
        return Value::Null;
    }
    *idx = next_idx;
    let trimmed = lines[next_idx].trim();
    if trimmed == "-" || trimmed.starts_with("- ") {
        parse_yaml_sequence(lines, idx, indent)
    } else {
        let mut map = Map::new();
        parse_yaml_mapping_entries(lines, idx, indent, &mut map);
        Value::Object(map)
    }
}

/// Parses a YAML sequence whose `-` items are aligned at `seq_indent`.
fn parse_yaml_sequence(lines: &[&str], idx: &mut usize, seq_indent: usize) -> Value {
    let mut items = Vec::new();
    while let Some((line_idx, indent)) = next_meaningful_line(lines, *idx) {
        if indent != seq_indent {
            break;
        }
        let trimmed = lines[line_idx].trim();
        let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| (trimmed == "-").then_some(""))
        else {
            break;
        };
        *idx = line_idx + 1;
        items.push(parse_sequence_item(lines, idx, seq_indent, rest.trim()));
    }
    Value::Array(items)
}

/// Parses the body of a single YAML sequence element following `-`.
fn parse_sequence_item(lines: &[&str], idx: &mut usize, seq_indent: usize, rest: &str) -> Value {
    if rest.is_empty() {
        return parse_yaml_block(lines, idx, seq_indent + 1);
    }
    if let Some((key, val_part)) = split_yaml_key_value(rest) {
        let mut map = Map::new();
        let entry_indent = seq_indent + 2;
        let first_val = resolve_yaml_value(lines, idx, entry_indent, val_part);
        map.insert(key, first_val);
        parse_yaml_mapping_entries(lines, idx, entry_indent, &mut map);
        return Value::Object(map);
    }
    parse_yaml_scalar(rest)
}

/// Parses YAML mapping `key: value` entries aligned at `map_indent` into `map`.
fn parse_yaml_mapping_entries(
    lines: &[&str],
    idx: &mut usize,
    map_indent: usize,
    map: &mut Map<String, Value>,
) {
    while let Some((line_idx, indent)) = next_meaningful_line(lines, *idx) {
        if indent != map_indent {
            break;
        }
        let trimmed = lines[line_idx].trim();
        if trimmed == "-" || trimmed.starts_with("- ") {
            break;
        }
        let Some((key, val_part)) = split_yaml_key_value(trimmed) else {
            panic!("invalid YAML mapping line {}: {trimmed}", line_idx + 1);
        };
        *idx = line_idx + 1;
        let value = resolve_yaml_value(lines, idx, map_indent, val_part);
        map.insert(key, value);
    }
}

/// Resolves the value portion of a `key: value` line, supporting nested blocks and `|` literals.
fn resolve_yaml_value(lines: &[&str], idx: &mut usize, parent_indent: usize, raw: &str) -> Value {
    let cleaned = strip_unquoted_comment(raw).trim();
    if cleaned == "|" || cleaned == "|-" {
        return parse_literal_block(lines, idx, parent_indent);
    }
    if cleaned.is_empty() {
        return parse_yaml_block(lines, idx, parent_indent + 1);
    }
    parse_yaml_scalar(cleaned)
}

/// Parses an indented `|` YAML literal block into a single multi-line string value.
fn parse_literal_block(lines: &[&str], idx: &mut usize, parent_indent: usize) -> Value {
    let mut collected: Vec<&str> = Vec::new();
    let mut block_indent: Option<usize> = None;
    while *idx < lines.len() {
        let line = lines[*idx];
        if line.trim().is_empty() {
            collected.push("");
            *idx += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent <= parent_indent {
            break;
        }
        let base = *block_indent.get_or_insert(indent);
        collected.push(&line[base.min(line.len())..]);
        *idx += 1;
    }
    Value::String(collected.join("\n").trim_end_matches('\n').to_string())
}

/// Returns `true` when `segment` starts with a quote or inline-sequence delimiter.
fn starts_with_scalar_delimiter(segment: &str) -> bool {
    matches!(segment.as_bytes().first(), Some(b'"' | b'\'' | b'['))
}

/// Splits a `key: value` segment when an unquoted colon separator is present.
fn split_yaml_key_value(segment: &str) -> Option<(String, &str)> {
    if starts_with_scalar_delimiter(segment) {
        return None;
    }
    let colon_pos = segment.find(':')?;
    let after = &segment[colon_pos + 1..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let key = segment[..colon_pos].trim().to_string();
    Some((key, after.trim()))
}

/// Updates single-quote and double-quote tracking flags for one character.
fn update_quote_state(ch: char, in_single: &mut bool, in_double: &mut bool) {
    if ch == '\'' && !*in_double {
        *in_single = !*in_single;
    } else if ch == '"' && !*in_single {
        *in_double = !*in_double;
    }
}

/// Returns `true` when `ch` at byte index `idx` starts an unquoted `#` YAML comment.
fn is_unquoted_comment_start(raw: &str, idx: usize, ch: char, quoted: bool) -> bool {
    if ch != '#' || quoted || idx == 0 {
        return false;
    }
    raw.as_bytes()[idx - 1] == b' '
}

/// Strips trailing `# ...` comments that appear outside single or double quotes.
fn strip_unquoted_comment(raw: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in raw.char_indices() {
        update_quote_state(ch, &mut in_single, &mut in_double);
        if is_unquoted_comment_start(raw, idx, ch, in_single || in_double) {
            return &raw[..idx];
        }
    }
    raw
}

/// Strips matching outer single or double quotes from a YAML scalar token when present.
fn unquote_yaml_string(cleaned: &str) -> Option<&str> {
    cleaned
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            cleaned
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
}

/// Parses a scalar or inline `[...]` array token into a `serde_json::Value`.
fn parse_yaml_scalar(raw: &str) -> Value {
    let cleaned = strip_unquoted_comment(raw).trim();
    if let Some(inner) = cleaned.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if inner.trim().is_empty() {
            return Value::Array(Vec::new());
        }
        let items = inner
            .split(',')
            .map(|part| parse_yaml_scalar(part.trim()))
            .collect();
        return Value::Array(items);
    }
    if let Some(unquoted) = unquote_yaml_string(cleaned) {
        return Value::String(unquoted.to_string());
    }
    match cleaned {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => cleaned
            .parse::<i64>()
            .map(|n| Value::Number(n.into()))
            .unwrap_or_else(|_| Value::String(cleaned.to_string())),
    }
}

// endregion

// region Slice 1: Multi-Target GoReleaser Workflow

/// Concatenates all `run` scripts inside a workflow job's `steps` sequence.
fn collect_job_run_scripts(job: &Value) -> String {
    job["steps"]
        .as_array()
        .expect("job steps must be an array")
        .iter()
        .filter_map(|step| step.get("run").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Finds a step in `job["steps"]` whose `uses` prefix matches `uses_prefix`.
fn find_step_by_uses<'a>(job: &'a Value, uses_prefix: &str) -> &'a Value {
    job["steps"]
        .as_array()
        .expect("job steps must be an array")
        .iter()
        .find(|step| {
            step.get("uses")
                .and_then(Value::as_str)
                .is_some_and(|u| u.starts_with(uses_prefix))
        })
        .unwrap_or_else(|| panic!("missing step with uses starting with `{uses_prefix}`"))
}

/// Asserts that the `validate` job checks formatting, clippy, tests, docs, and tag-only version match.
fn assert_validate_job(validate: &Value) {
    assert_eq!(validate["runs-on"], "ubuntu-latest");
    let combined_runs = collect_job_run_scripts(validate);
    for expected_cmd in [
        "cargo fmt -- --check",
        "cargo clippy -p org-tools --all-targets -- -D warnings",
        "cargo clippy -- -D warnings",
        "cargo test",
        "fmt check docs/operations/installation.org docs/operations/homebrew-tap.org",
        "tests/fixtures/edge_cases/",
    ] {
        assert!(
            combined_runs.contains(expected_cmd),
            "validate job missing command `{expected_cmd}`"
        );
    }
    let version_step = validate["steps"]
        .as_array()
        .expect("validate steps")
        .iter()
        .find(|step| {
            step.get("run")
                .and_then(Value::as_str)
                .is_some_and(|r| r.contains("CARGO_VERSION"))
        })
        .expect("validate job must include tag-vs-Cargo.toml version step");
    let condition = version_step["if"]
        .as_str()
        .expect("tag version verification step must be guarded by `if:` for workflow_dispatch");
    assert!(
        condition.contains("startsWith(github.ref, 'refs/tags/v')"),
        "expected tag-only condition on version check, got: {condition}"
    );
}

/// Verifies `.github/workflows/release.yml` triggers, env defaults, SPDX header, and `validate` job.
#[test]
fn release_workflow_triggers_on_version_tags_and_workflow_dispatch() {
    let (raw, doc) = load_release_workflow();
    assert!(
        raw.contains("SPDX-License-Identifier: GPL-3.0-or-later"),
        "release.yml must include GPL-3.0-or-later SPDX header"
    );
    assert_eq!(doc["on"]["push"]["tags"], serde_json::json!(["v*"]));
    assert!(
        doc["on"].get("workflow_dispatch").is_some(),
        "release.yml must support manual workflow_dispatch dry-runs"
    );

    let env_map = doc["env"].as_object().expect("workflow top-level env map");
    let goreleaser_ver = env_map
        .get("ORGFMT_GORELEASER_VERSION")
        .and_then(Value::as_str)
        .expect("env.ORGFMT_GORELEASER_VERSION must be configured");
    assert!(
        goreleaser_ver.contains(EXPECTED_GORELEASER_DEFAULT),
        "ORGFMT_GORELEASER_VERSION must default to {EXPECTED_GORELEASER_DEFAULT}, got {goreleaser_ver}"
    );
    assert!(env_map.contains_key("ORGFMT_RELEASE_METRICS_FILE"));
    assert!(env_map.contains_key("ORGFMT_RELEASE_EVENTS_FILE"));

    assert_validate_job(&doc["jobs"]["validate"]);
}

/// Asserts that `release-packages` configures GoReleaser v2 action for tag and snapshot builds.
fn assert_goreleaser_step(release_job: &Value) {
    let gr_step = find_step_by_uses(release_job, "goreleaser/goreleaser-action@v6");
    assert_eq!(gr_step["with"]["distribution"], "goreleaser");
    let version_expr = gr_step["with"]["version"]
        .as_str()
        .expect("goreleaser with.version must be set");
    assert!(
        version_expr.contains("~>")
            && (version_expr.contains("ORGFMT_GORELEASER_VERSION")
                || version_expr.contains(EXPECTED_GORELEASER_DEFAULT)),
        "goreleaser version must use ~> v2.18.0 / ORGFMT_GORELEASER_VERSION, got: {version_expr}"
    );
    let args_expr = gr_step["with"]["args"]
        .as_str()
        .expect("goreleaser with.args must be set");
    assert!(
        args_expr.contains("startsWith(github.ref, 'refs/tags/v')")
            && args_expr.contains("release --clean")
            && args_expr.contains("release --snapshot --clean"),
        "goreleaser args must switch between release and snapshot modes, got: {args_expr}"
    );
    assert_eq!(
        gr_step["env"]["GITHUB_TOKEN"],
        "${{ secrets.GITHUB_TOKEN }}"
    );
    assert_eq!(
        gr_step["env"]["HOMEBREW_TAP_GITHUB_TOKEN"],
        "${{ secrets.HOMEBREW_TAP_GITHUB_TOKEN }}"
    );
}

/// Asserts that `release-packages` installs all 6 Rust cross-compilation targets plus `zig`, `cargo-zigbuild`, and `cargo-xwin`.
fn assert_cross_compilation_toolchains(release_job: &Value) {
    let toolchain = find_step_by_uses(release_job, "dtolnay/rust-toolchain@stable");
    let targets = toolchain["with"]["targets"]
        .as_str()
        .expect("rust-toolchain targets string");
    for target in EXPECTED_RUST_TARGETS {
        assert!(
            targets.contains(target),
            "missing Rust target `{target}` in `{targets}`"
        );
    }
    let _ = find_step_by_uses(release_job, "taiki-e/install-action@cargo-zigbuild");
    let _ = find_step_by_uses(release_job, "taiki-e/install-action@cargo-xwin");
    let has_zig = release_job["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .any(|s| {
            s.get("uses")
                .and_then(Value::as_str)
                .is_some_and(|u| u.starts_with("goto-bus-stop/setup-zig@v2"))
                || s.get("run")
                    .and_then(Value::as_str)
                    .is_some_and(|r| r.contains("brew install zig"))
        });
    assert!(has_zig, "release-packages must install zig");
}

/// Verifies `release-packages` cross-compilation toolchains, GoReleaser invocation, verification, and telemetry upload.
#[test]
fn release_packages_job_builds_all_six_targets_via_goreleaser_and_verifies_dist() {
    let (_, doc) = load_release_workflow();
    let jobs = doc["jobs"].as_object().expect("workflow jobs object");
    for legacy_job in [
        "publish-core",
        "publish-cli",
        "build-binaries",
        "github-release",
    ] {
        assert!(
            !jobs.contains_key(legacy_job),
            "legacy matrix/manual release job `{legacy_job}` must be replaced"
        );
    }

    let release_job = &doc["jobs"]["release-packages"];
    assert_eq!(release_job["runs-on"], "macos-latest");
    assert_eq!(
        release_job["needs"], "validate",
        "release-packages must depend ONLY on validate, never on publish-crates"
    );

    assert_cross_compilation_toolchains(release_job);
    assert_goreleaser_step(release_job);

    let run_scripts = collect_job_run_scripts(release_job);
    assert!(
        run_scripts.contains("bash scripts/verify-release-packages.sh --dist-dir dist"),
        "release-packages must execute verify-release-packages.sh"
    );
    assert!(
        run_scripts.contains("GITHUB_STEP_SUMMARY"),
        "release-packages must append summary and Prometheus metrics to GITHUB_STEP_SUMMARY"
    );
    let _ = find_step_by_uses(release_job, "actions/upload-artifact@v4");

    let publish_script = extract_publish_crates_script(&doc);
    assert!(
        publish_script.contains(EXPECTED_SKIP_CRATES_NOTICE)
            && publish_script.contains("cargo publish -p org-tools-core")
            && publish_script.contains("cargo publish -p org-tools"),
        "publish-crates job must gate crates.io publish behind CARGO_REGISTRY_TOKEN"
    );
}

// endregion

// region Slice 2: Graceful Crates.io Publish Gate

/// Executes the `publish-crates` shell script extracted from `release.yml` with a fake `cargo` shim.
#[cfg(unix)]
fn run_extracted_publish_crates_step(
    script: &str,
    token: &str,
    github_ref: &str,
) -> (Output, String) {
    let temp = TempDir::new().expect("create temp dir for publish-crates gate test");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    let cargo_log = temp.path().join("cargo-invocations.log");
    let cargo_shim = bin_dir.join("cargo");
    fs::write(
        &cargo_shim,
        format!(
            "#!/usr/bin/env bash\necho \"$*\" >> \"{}\"\nexit 0\n",
            cargo_log.display()
        ),
    )
    .expect("write fake cargo shim");
    fs::set_permissions(&cargo_shim, fs::Permissions::from_mode(0o755)).expect("chmod cargo shim");

    let current_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .args(["-euo", "pipefail", "-c", script])
        .current_dir(temp.path())
        .env("PATH", format!("{}:{current_path}", bin_dir.display()))
        .env("CARGO_REGISTRY_TOKEN", token)
        .env("GITHUB_REF", github_ref)
        .output()
        .expect("execute publish-crates step script");
    let recorded = fs::read_to_string(&cargo_log).unwrap_or_default();
    (output, recorded)
}

/// Extracts the `CARGO_REGISTRY_TOKEN`-gated `run` script from `jobs.publish-crates`.
fn extract_publish_crates_script(doc: &Value) -> String {
    let publish_job = &doc["jobs"]["publish-crates"];
    assert_eq!(publish_job["runs-on"], "ubuntu-latest");
    assert_eq!(publish_job["needs"], "validate");
    let publish_step = publish_job["steps"]
        .as_array()
        .expect("publish-crates steps must be an array")
        .iter()
        .find(|step| {
            step.get("run")
                .and_then(Value::as_str)
                .is_some_and(|r| r.contains("CARGO_REGISTRY_TOKEN"))
        })
        .expect("publish-crates must contain a token-gated publish run step");
    assert_eq!(
        publish_step["env"]["CARGO_REGISTRY_TOKEN"],
        "${{ secrets.CARGO_REGISTRY_TOKEN }}"
    );
    publish_step["run"]
        .as_str()
        .expect("step run script")
        .to_string()
}

/// Verifies `publish-crates` is decoupled from `release-packages`, skips cleanly without token, and publishes in order when set.
#[cfg(unix)]
#[test]
fn publish_crates_job_gracefully_skips_when_token_absent_and_publishes_in_order() {
    let (_, doc) = load_release_workflow();
    let step_script = extract_publish_crates_script(&doc);

    let (empty_out, empty_log) =
        run_extracted_publish_crates_step(&step_script, "", "refs/tags/v0.1.4");
    assert!(
        empty_out.status.success(),
        "empty CARGO_REGISTRY_TOKEN must exit 0, stderr: {}",
        String::from_utf8_lossy(&empty_out.stderr)
    );
    let empty_stdout = String::from_utf8_lossy(&empty_out.stdout);
    assert!(
        empty_stdout.contains(EXPECTED_SKIP_CRATES_NOTICE),
        "expected skip notice `{EXPECTED_SKIP_CRATES_NOTICE}`, got: {empty_stdout}"
    );
    assert!(
        empty_log.trim().is_empty(),
        "cargo publish must not run when token is empty, got: {empty_log}"
    );

    let (pub_out, pub_log) =
        run_extracted_publish_crates_step(&step_script, "test-crates-token", "refs/tags/v0.1.4");
    assert!(
        pub_out.status.success(),
        "populated CARGO_REGISTRY_TOKEN on tag must exit 0, stderr: {}",
        String::from_utf8_lossy(&pub_out.stderr)
    );
    let invocations: Vec<&str> = pub_log.lines().collect();
    assert_eq!(
        invocations,
        vec!["publish -p org-tools-core", "publish -p org-tools"]
    );
}

// endregion

// region Slice 3: Release Script Dry-Run Behaviour

/// Creates an isolated temporary git repository seeded with workspace manifests for `scripts/release.sh` testing.
#[cfg(unix)]
fn init_synthetic_release_repo(workspace_ver: &str, cli_dep_ver: &str) -> TempDir {
    let temp = TempDir::new().expect("create synthetic release repo");
    let root = temp.path();
    fs::create_dir_all(root.join("crates").join("org")).expect("create crates/org");

    let root_cargo = format!(
        "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"{workspace_ver}\"\nedition = \"2021\"\n"
    );
    let cli_cargo = format!(
        "[package]\nname = \"org-tools\"\nversion.workspace = true\n\n[dependencies]\norg-tools-core = {{ path = \"../org-tools-core\", version = \"{cli_dep_ver}\" }}\n"
    );
    fs::write(root.join("Cargo.toml"), root_cargo).expect("write root Cargo.toml");
    fs::write(root.join("crates/org/Cargo.toml"), cli_cargo).expect("write crates/org/Cargo.toml");
    fs::write(
        root.join(".goreleaser.yaml"),
        "version: 2\nproject_name: org-tools\n",
    )
    .expect("write synthetic .goreleaser.yaml");

    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "test@example.com"][..],
        &["config", "user.name", "Test Operator"][..],
        &["config", "commit.gpgsign", "false"][..],
        &["add", "."][..],
        &["commit", "-qm", "initial"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .expect("run git setup");
        assert!(status.success(), "git {args:?} failed");
    }
    temp
}

/// Installs an executable `goreleaser` shim inside `repo_dir/shim-bin` that logs invocations to `goreleaser.log`.
#[cfg(unix)]
fn install_goreleaser_check_shim(repo_dir: &Path) -> (PathBuf, PathBuf) {
    let bin_dir = repo_dir.join("shim-bin");
    fs::create_dir_all(&bin_dir).expect("create shim-bin");
    let gr_log = repo_dir.join("goreleaser.log");
    let gr_shim = bin_dir.join("goreleaser");
    fs::write(
        &gr_shim,
        format!(
            "#!/usr/bin/env bash\necho \"$*\" >> \"{}\"\n[ \"$1\" = \"check\" ] && exit 0\nexit 1\n",
            gr_log.display()
        ),
    )
    .expect("write goreleaser shim");
    fs::set_permissions(&gr_shim, fs::Permissions::from_mode(0o755)).expect("chmod shim");
    (bin_dir, gr_log)
}

/// Asserts that `repo_dir` manifests and git tags remain unchanged after `--dry-run` and `goreleaser check` ran.
#[cfg(unix)]
fn assert_repo_unmodified_after_dry_run(
    repo_dir: &Path,
    before_root: &str,
    before_cli: &str,
    gr_log: &Path,
) {
    assert_eq!(
        fs::read_to_string(repo_dir.join("Cargo.toml")).expect("read Cargo.toml after"),
        before_root
    );
    assert_eq!(
        fs::read_to_string(repo_dir.join("crates/org/Cargo.toml"))
            .expect("read cli Cargo.toml after"),
        before_cli
    );
    let tags = Command::new("git")
        .args(["tag", "-l"])
        .current_dir(repo_dir)
        .output()
        .expect("list git tags");
    assert!(
        String::from_utf8_lossy(&tags.stdout).trim().is_empty(),
        "dry-run must not create git tags"
    );
    assert!(
        fs::read_to_string(gr_log)
            .expect("read goreleaser shim log")
            .contains("check"),
        "dry-run must invoke `goreleaser check` when goreleaser is on PATH"
    );
}

/// Verifies `scripts/release.sh --dry-run` checks version sync and `goreleaser check` without mutating files or tags.
#[cfg(unix)]
#[test]
fn release_script_dry_run_validates_sync_and_goreleaser_without_mutating_repo() {
    let script_path = workspace_root().join("scripts").join("release.sh");
    assert!(
        read_workspace_file("scripts/release.sh")
            .contains("SPDX-License-Identifier: GPL-3.0-or-later"),
        "scripts/release.sh must carry GPL-3.0-or-later SPDX header"
    );
    let mode = fs::metadata(&script_path)
        .expect("stat scripts/release.sh")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "scripts/release.sh must be executable");

    let repo = init_synthetic_release_repo("0.1.4", "0.1.4");
    let before_root = fs::read_to_string(repo.path().join("Cargo.toml")).expect("read Cargo.toml");
    let before_cli =
        fs::read_to_string(repo.path().join("crates/org/Cargo.toml")).expect("read cli Cargo.toml");
    let (bin_dir, gr_log) = install_goreleaser_check_shim(repo.path());

    let current_path = std::env::var("PATH").unwrap_or_default();
    for extra_args in [
        vec!["--dry-run", "--repo-dir"],
        vec!["--dry-run", "0.2.0", "--repo-dir"],
    ] {
        let out = Command::new(&script_path)
            .args(&extra_args)
            .arg(repo.path())
            .env("PATH", format!("{}:{current_path}", bin_dir.display()))
            .output()
            .expect("execute scripts/release.sh --dry-run");
        assert!(
            out.status.success(),
            "release.sh --dry-run failed: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    assert_repo_unmodified_after_dry_run(repo.path(), &before_root, &before_cli, &gr_log);
}

/// Verifies `scripts/release.sh` exits non-zero on invalid semver inputs or desynchronized manifests.
#[cfg(unix)]
#[test]
fn release_script_rejects_invalid_semver_and_out_of_sync_manifests() {
    let script_path = workspace_root().join("scripts").join("release.sh");
    let valid_repo = init_synthetic_release_repo("0.1.4", "0.1.4");

    for bad_version in ["not-a-semver", "0.2", "v0.2.0", "1.2.3-rc1"] {
        let out = Command::new("bash")
            .arg(&script_path)
            .args(["--dry-run", "--repo-dir"])
            .arg(valid_repo.path())
            .arg(bad_version)
            .output()
            .expect("run release.sh with invalid version");
        assert!(
            !out.status.success(),
            "invalid semver `{bad_version}` must exit non-zero"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("X.Y.Z"),
            "invalid semver `{bad_version}` must report expected X.Y.Z format on stderr, got: {stderr}"
        );
    }

    let desynced_repo = init_synthetic_release_repo("0.1.4", "0.1.2");
    let desync_out = Command::new("bash")
        .arg(&script_path)
        .args(["--dry-run", "--repo-dir"])
        .arg(desynced_repo.path())
        .output()
        .expect("run release.sh against out-of-sync manifests");
    assert!(
        !desync_out.status.success(),
        "out-of-sync Cargo.toml vs crates/org/Cargo.toml must fail dry-run"
    );
    let desync_stderr = String::from_utf8_lossy(&desync_out.stderr);
    assert!(
        desync_stderr.contains("mismatch") || desync_stderr.contains("sync"),
        "desynced manifests must report mismatch/sync diagnostic on stderr, got: {desync_stderr}"
    );
}

// endregion

// region Slice 4: Installation Manual & Homebrew Tap Runbook

/// Asserts that `docs/operations/installation.org` fixes stale references and documents all install channels.
fn assert_installation_manual_contents(install_doc: &str) {
    assert!(
        !install_doc.contains("github.com/org-tools/org-tools"),
        "installation.org must not reference stale github.com/org-tools/org-tools URL"
    );
    assert!(
        !install_doc
            .lines()
            .any(|line| line.trim() == "cargo install org"),
        "installation.org must not contain stale `cargo install org` command"
    );
    for required_snippet in [
        "https://github.com/lino/org-tools.git",
        "cargo install org-tools",
        "brew install --cask lino/tap/org-tools",
        "ghcr.io/lino/org-tools",
        "dpkg -i",
        "rpm -Uvh",
        "sha256sum -c checksums.txt",
        "mise",
        "--completions",
    ] {
        assert!(
            install_doc.contains(required_snippet),
            "installation.org missing required snippet `{required_snippet}`"
        );
    }
}

/// Asserts that `docs/operations/homebrew-tap.org` covers tap operations, PAT rotation, observability, and Mermaid diagramming.
fn assert_homebrew_tap_runbook_contents(tap_doc: &str) {
    for required_snippet in [
        "lino/homebrew-tap",
        "HOMEBREW_TAP_GITHUB_TOKEN",
        "Contents: Read and write",
        "CARGO_REGISTRY_TOKEN",
        "workflow_dispatch",
        "orgfmt_release_",
        "FRITZ_TRACE_ID",
        "com.apple.quarantine",
        "#+begin_src mermaid",
    ] {
        assert!(
            tap_doc.contains(required_snippet),
            "homebrew-tap.org missing required operational topic `{required_snippet}`"
        );
    }
}

/// Verifies `installation.org` and `homebrew-tap.org` content completeness and zero `org fmt check` diagnostics.
#[test]
fn installation_and_homebrew_tap_docs_cover_all_channels_and_pass_org_fmt_check() {
    let install_doc = read_workspace_file("docs/operations/installation.org");
    assert_installation_manual_contents(&install_doc);

    let tap_path = workspace_root()
        .join("docs")
        .join("operations")
        .join("homebrew-tap.org");
    assert!(
        tap_path.is_file(),
        "missing operator runbook {}",
        tap_path.display()
    );
    let tap_doc = read_workspace_file("docs/operations/homebrew-tap.org");
    assert_homebrew_tap_runbook_contents(&tap_doc);

    let fmt_out = Command::new(env!("CARGO_BIN_EXE_org"))
        .args([
            "fmt",
            "check",
            "docs/operations/installation.org",
            "docs/operations/homebrew-tap.org",
        ])
        .current_dir(workspace_root())
        .output()
        .expect("execute `org fmt check` on operations docs");
    assert!(
        fmt_out.status.success(),
        "`org fmt check` failed on operations docs:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&fmt_out.stdout),
        String::from_utf8_lossy(&fmt_out.stderr)
    );
}

/// Asserts that `.gitattributes` carries the SPDX header and enforces `eol=lf` across text/script extensions.
fn assert_gitattributes_enforces_lf(gitattributes: &str) {
    assert!(
        gitattributes.contains("Copyright (C) 2026 org-tools contributors")
            && gitattributes.contains("SPDX-License-Identifier: GPL-3.0-or-later"),
        ".gitattributes must include standard GPL-3.0-or-later SPDX header"
    );
    for expected_rule in [
        "* text=auto eol=lf",
        "*.rs text eol=lf",
        "*.org text eol=lf",
        "*.toml text eol=lf",
        "*.yaml text eol=lf",
        "*.yml text eol=lf",
        "*.json text eol=lf",
        "*.md text eol=lf",
        "*.sh text eol=lf",
    ] {
        assert!(
            gitattributes
                .lines()
                .any(|line| line.trim() == expected_rule),
            ".gitattributes missing required LF rule `{expected_rule}`"
        );
    }
}

/// Verifies `.github/workflows/ci.yml` cross-platform release cache SLO gates and `.gitattributes` LF rules.
#[test]
fn ci_workflow_and_gitattributes_enforce_cross_platform_slos_and_lf_endings() {
    let raw_ci = read_workspace_file(".github/workflows/ci.yml");
    assert!(
        raw_ci.contains("SPDX-License-Identifier: GPL-3.0-or-later"),
        "ci.yml must include GPL-3.0-or-later SPDX header"
    );
    let ci_doc = parse_yaml_document(&raw_ci);
    let cross = &ci_doc["jobs"]["cross-platform"];
    assert_eq!(
        cross["strategy"]["fail-fast"],
        Value::Bool(false),
        "jobs.cross-platform.strategy.fail-fast must be false"
    );
    for job_name in ["build", "cross-platform"] {
        let scripts = collect_job_run_scripts(&ci_doc["jobs"][job_name]);
        assert!(
            scripts.contains("cargo test --release --test cache_tests"),
            "ci.yml job `{job_name}` must run `cargo test --release --test cache_tests`"
        );
    }
    assert_eq!(ci_doc["jobs"]["container"]["needs"], "validate");
    let (_, rel_doc) = load_release_workflow();
    assert_eq!(rel_doc["jobs"]["publish-container"]["needs"], "validate");
    let gitattributes = read_workspace_file(".gitattributes");
    assert_gitattributes_enforces_lf(&gitattributes);
}

// endregion
