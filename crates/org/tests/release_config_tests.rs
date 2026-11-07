// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=830 reason="Single authorized Cargo integration test target bundling pure-Rust YAML subset parser, FIPS 180-4 SHA-256 implementation, and all four TDD slices without modifying Cargo.toml"

//! Behavioural and structural tests for GoReleaser v2 packaging, shell completions, and release verification.

use serde_json::{Map, Value};
#[cfg(unix)]
use std::fmt::Write as _;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Output};
#[cfg(unix)]
use tempfile::TempDir;

// region Constants & Path Helpers

/// Canonical description expected across Linux packages and Homebrew cask definitions.
const EXPECTED_DESCRIPTION: &str =
    "Unified pure-Rust CLI for Org-mode: lint, format, query, roam, edna, clock, export";

/// Canonical project homepage URL expected in package metadata.
const EXPECTED_HOMEPAGE: &str = "https://github.com/lino/org-tools";

/// Synthetic version string used when constructing release verification fixtures.
#[cfg(unix)]
const SYNTHETIC_VERSION: &str = "0.1.4";

/// Synthetic trace identifier propagated into release verification JSONL events.
#[cfg(unix)]
const TEST_TRACE_ID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// Returns the workspace root directory derived from the `org` crate manifest path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/org must reside two levels beneath the workspace root")
        .to_path_buf()
}

/// Reads and parses the repository `.goreleaser.yaml` file into a JSON value tree.
fn load_goreleaser_config() -> Value {
    let config_path = workspace_root().join(".goreleaser.yaml");
    let raw = fs::read_to_string(&config_path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", config_path.display());
    });
    parse_yaml_document(&raw)
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

// region Synthetic Dist & SHA-256 Builders

/// FIPS 180-4 round constants for pure-Rust SHA-256 digest calculation in tests.
#[cfg(unix)]
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Computes the lowercase hexadecimal SHA-256 digest of `data` using pure standard-library Rust.
#[cfg(unix)]
fn sha256_hex(data: &[u8]) -> String {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    let mut offset = 0;
    while offset + 64 <= padded.len() {
        let schedule = build_sha256_schedule(&padded[offset..offset + 64]);
        compress_sha256_schedule(&mut state, &schedule);
        offset += 64;
    }
    let mut digest = String::with_capacity(64);
    for word in state {
        let _ = write!(digest, "{word:08x}");
    }
    digest
}

/// Expands a 64-byte SHA-256 message block into a 64-word message schedule.
#[cfg(unix)]
fn build_sha256_schedule(chunk: &[u8]) -> [u32; 64] {
    let mut schedule = [0u32; 64];
    for (idx, slot) in schedule.iter_mut().take(16).enumerate() {
        let start = idx * 4;
        *slot = u32::from_be_bytes(chunk[start..start + 4].try_into().expect("4-byte word"));
    }
    for idx in 16..64 {
        let s0 = schedule[idx - 15].rotate_right(7)
            ^ schedule[idx - 15].rotate_right(18)
            ^ (schedule[idx - 15] >> 3);
        let s1 = schedule[idx - 2].rotate_right(17)
            ^ schedule[idx - 2].rotate_right(19)
            ^ (schedule[idx - 2] >> 10);
        schedule[idx] = schedule[idx - 16]
            .wrapping_add(s0)
            .wrapping_add(schedule[idx - 7])
            .wrapping_add(s1);
    }
    schedule
}

/// Applies the 64-round SHA-256 compression function to `state` using `schedule`.
#[cfg(unix)]
fn compress_sha256_schedule(state: &mut [u32; 8], schedule: &[u32; 64]) {
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (round, &word) in schedule.iter().enumerate() {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[round])
            .wrapping_add(word);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(s0.wrapping_add(maj));
    }
    for (slot, delta) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(delta);
    }
}

/// Populates a complete synthetic GoReleaser `dist/` directory with valid checksums and cask metadata.
#[cfg(unix)]
fn populate_synthetic_dist(dist_dir: &Path) {
    fs::create_dir_all(dist_dir).expect("create dist dir");
    let artifacts = [
        format!("org-tools_{SYNTHETIC_VERSION}_darwin_arm64.tar.gz"),
        format!("org-tools_{SYNTHETIC_VERSION}_darwin_amd64.tar.gz"),
        format!("org-tools_{SYNTHETIC_VERSION}_linux_amd64.tar.gz"),
        format!("org-tools_{SYNTHETIC_VERSION}_linux_arm64.tar.gz"),
        format!("org-tools_{SYNTHETIC_VERSION}_windows_amd64.zip"),
        format!("org-tools_{SYNTHETIC_VERSION}_windows_arm64.zip"),
        format!("org-tools_{SYNTHETIC_VERSION}_linux_amd64.deb"),
        format!("org-tools_{SYNTHETIC_VERSION}_linux_arm64.deb"),
        format!("org-tools_{SYNTHETIC_VERSION}_linux_amd64.rpm"),
        format!("org-tools_{SYNTHETIC_VERSION}_linux_arm64.rpm"),
    ];
    let mut checksum_lines = Vec::new();
    for name in &artifacts {
        let payload = format!("synthetic-artifact-payload:{name}\n");
        fs::write(dist_dir.join(name), payload.as_bytes()).expect("write synthetic artifact");
        checksum_lines.push(format!("{}  {name}", sha256_hex(payload.as_bytes())));
    }
    fs::write(
        dist_dir.join("checksums.txt"),
        format!("{}\n", checksum_lines.join("\n")),
    )
    .expect("write checksums.txt");
    write_synthetic_cask(dist_dir);
}

/// Writes a valid synthetic `homebrew/Casks/org-tools.rb` cask file inside `dist_dir`.
#[cfg(unix)]
fn write_synthetic_cask(dist_dir: &Path) {
    let cask_dir = dist_dir.join("homebrew").join("Casks");
    fs::create_dir_all(&cask_dir).expect("create cask directory");
    let cask_body = concat!(
        "cask \"org-tools\" do\n",
        "  binary \"org\"\n",
        "  postflight do\n",
        "    if OS.mac?\n",
        "      system_command \"/usr/bin/xattr\", args: [\"-dr\", \"com.apple.quarantine\", \"#{staged_path}/org\"]\n",
        "    end\n",
        "  end\n",
        "end\n",
    );
    fs::write(cask_dir.join("org-tools.rb"), cask_body).expect("write org-tools.rb cask");
}

// endregion

// region Slice 1: GoReleaser Builds, Archives & Checksums

/// Asserts that a single GoReleaser build entry matches expected Rust cross-compilation settings.
fn assert_rust_build_entry(
    build: &Value,
    expected_id: &str,
    expected_tool: &str,
    expected_cmd: &str,
    expected_targets: &[&str],
) {
    assert_eq!(build["id"], expected_id);
    assert_eq!(build["builder"], "rust");
    assert_eq!(build["binary"], "org");
    assert_eq!(build["dir"], ".");
    assert_eq!(build["tool"], expected_tool);
    assert_eq!(build["command"], expected_cmd);
    assert_eq!(
        build["flags"],
        serde_json::json!(["--release", "--locked", "-p=org-tools"])
    );
    assert_eq!(build["targets"], serde_json::json!(expected_targets));
}

/// Asserts that `.goreleaser.yaml` configures `archives` and `checksum` sections per spec.
fn assert_archives_and_checksum(doc: &Value) {
    let archive = &doc["archives"][0];
    assert_eq!(archive["id"], "org-archives");
    assert_eq!(
        archive["ids"],
        serde_json::json!(["org-darwin", "org-linux", "org-windows"])
    );
    assert!(
        archive.get("builds").is_none(),
        "deprecated `archives.builds` key must not be used; use `ids`"
    );
    assert_eq!(archive["formats"], serde_json::json!(["tar.gz"]));
    assert_eq!(archive["format_overrides"][0]["goos"], "windows");
    assert_eq!(
        archive["format_overrides"][0]["formats"],
        serde_json::json!(["zip"])
    );
    assert_eq!(
        archive["files"],
        serde_json::json!([
            "LICENSE",
            "README.org",
            "completions/org.bash",
            "completions/_org",
            "completions/org.fish"
        ])
    );
    assert_eq!(doc["checksum"]["name_template"], "checksums.txt");
    assert_eq!(doc["checksum"]["algorithm"], "sha256");
}

/// Verifies `.goreleaser.yaml` version 2 metadata, 3 Rust builders (6 targets), archives, and SHA-256 checksums.
#[test]
fn goreleaser_yaml_defines_v2_rust_builds_archives_and_checksums() {
    let doc = load_goreleaser_config();
    assert_eq!(doc["version"], 2);
    assert_eq!(doc["project_name"], "org-tools");
    assert_eq!(
        doc["before"]["hooks"],
        serde_json::json!(["bash scripts/build-completions.sh"])
    );

    let builds = doc["builds"].as_array().expect("builds must be a sequence");
    assert_eq!(builds.len(), 3);
    assert_rust_build_entry(
        &builds[0],
        "org-darwin",
        "cargo",
        "build",
        &["aarch64-apple-darwin", "x86_64-apple-darwin"],
    );
    assert_rust_build_entry(
        &builds[1],
        "org-linux",
        "cargo-zigbuild",
        "zigbuild",
        &["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"],
    );
    assert_rust_build_entry(
        &builds[2],
        "org-windows",
        "cargo-xwin",
        "build",
        &["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"],
    );
    assert_archives_and_checksum(&doc);
}

// endregion

// region Slice 2: Linux Packages (nFPM) & Homebrew Cask

/// Asserts that `.goreleaser.yaml` configures nFPM `.deb`/`.rpm` contents and metadata accurately.
fn assert_nfpm_linux_packages(nfpm: &Value) {
    assert_eq!(nfpm["id"], "org-linux-packages");
    assert_eq!(nfpm["ids"], serde_json::json!(["org-linux"]));
    assert!(
        nfpm.get("builds").is_none(),
        "deprecated `nfpms.builds` key must not be used; use `ids`"
    );
    assert_eq!(nfpm["package_name"], "org-tools");
    assert_eq!(nfpm["bindir"], "/usr/bin");
    assert_eq!(nfpm["homepage"], EXPECTED_HOMEPAGE);
    assert_eq!(nfpm["license"], "GPL-3.0-or-later");
    assert_eq!(nfpm["maintainer"], "Lino Helms <lino@lino.io>");
    assert_eq!(nfpm["description"], EXPECTED_DESCRIPTION);
    assert_eq!(nfpm["formats"], serde_json::json!(["deb", "rpm"]));

    let expected_mappings = [
        (
            "completions/org.bash",
            "/usr/share/bash-completion/completions/org",
        ),
        ("completions/_org", "/usr/share/zsh/site-functions/_org"),
        (
            "completions/org.fish",
            "/usr/share/fish/vendor_completions.d/org.fish",
        ),
        ("LICENSE", "/usr/share/doc/org-tools/LICENSE"),
        ("README.org", "/usr/share/doc/org-tools/README.org"),
    ];
    let contents = nfpm["contents"]
        .as_array()
        .expect("nfpms.contents must be a sequence");
    assert_eq!(contents.len(), expected_mappings.len());
    for (src, dst) in expected_mappings {
        let matched = contents
            .iter()
            .any(|item| item["src"] == src && item["dst"] == dst);
        assert!(matched, "missing nfpm content mapping {src} -> {dst}");
    }
}

/// Asserts that `.goreleaser.yaml` configures `homebrew_casks` for `lino/homebrew-tap` with quarantine hook.
fn assert_homebrew_cask_spec(cask: &Value) {
    assert_eq!(cask["name"], "org-tools");
    assert_eq!(cask["binaries"], serde_json::json!(["org"]));
    assert_eq!(cask["homepage"], EXPECTED_HOMEPAGE);
    assert_eq!(cask["license"], "GPL-3.0-or-later");
    assert_eq!(cask["description"], EXPECTED_DESCRIPTION);
    assert_eq!(cask["repository"]["owner"], "lino");
    assert_eq!(cask["repository"]["name"], "homebrew-tap");
    assert_eq!(
        cask["repository"]["token"],
        "{{ .Env.HOMEBREW_TAP_GITHUB_TOKEN }}"
    );
    assert_eq!(
        cask["skip_upload"],
        "{{ if .Env.HOMEBREW_TAP_GITHUB_TOKEN }}false{{ else }}true{{ end }}"
    );
    assert_eq!(cask["completions"]["bash"], "completions/org.bash");
    assert_eq!(cask["completions"]["zsh"], "completions/_org");
    assert_eq!(cask["completions"]["fish"], "completions/org.fish");

    let post_install = cask["hooks"]["post"]["install"]
        .as_str()
        .expect("homebrew_casks.hooks.post.install must be a string");
    for token in [
        "if OS.mac?",
        "/usr/bin/xattr",
        "-dr",
        "com.apple.quarantine",
        "#{staged_path}/org",
        "end",
    ] {
        assert!(
            post_install.contains(token),
            "post-install hook missing `{token}`: {post_install}"
        );
    }
}

/// Verifies `.goreleaser.yaml` nFPM `.deb`/`.rpm` packaging, `homebrew_casks` spec, and absence of `brews`.
#[test]
fn goreleaser_yaml_defines_nfpm_packages_and_homebrew_cask_without_brews() {
    let doc = load_goreleaser_config();
    assert!(
        doc.get("brews").is_none(),
        "deprecated top-level `brews` section must not be present"
    );
    assert_nfpm_linux_packages(&doc["nfpms"][0]);
    assert_homebrew_cask_spec(&doc["homebrew_casks"][0]);
}

// endregion

// region Slice 3: Shell Completion Generation Script

/// Verifies `scripts/build-completions.sh` emits valid non-empty Bash, Zsh, and Fish completion files.
#[cfg(unix)]
#[test]
fn build_completions_script_generates_bash_zsh_and_fish_files() {
    let script = workspace_root().join("scripts/build-completions.sh");
    assert!(script.is_file(), "missing script {}", script.display());

    let out_dir = TempDir::new().expect("create temporary completions directory");
    let status = Command::new("bash")
        .arg(&script)
        .env("ORGFMT_COMPLETIONS_DIR", out_dir.path())
        .env("ORGFMT_BIN", env!("CARGO_BIN_EXE_org"))
        .status()
        .expect("execute build-completions.sh");
    assert!(
        status.success(),
        "build-completions.sh exited with {status}"
    );

    for (filename, expected_marker) in [
        ("org.bash", "_org"),
        ("_org", "#compdef org"),
        ("org.fish", "complete -c org"),
    ] {
        let path = out_dir.path().join(filename);
        let body = fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("expected generated completion {}: {err}", path.display())
        });
        assert!(!body.trim().is_empty(), "{filename} must not be empty");
        assert!(
            body.contains(expected_marker),
            "{filename} missing expected shell marker `{expected_marker}`"
        );
    }
}

// endregion

// region Slice 4: Release Package Verification & Observability

/// Runs `scripts/verify-release-packages.sh` against `dist_dir` with optional telemetry file paths.
#[cfg(unix)]
fn run_verify_script(
    dist_dir: &Path,
    metrics_file: Option<&Path>,
    events_file: Option<&Path>,
    extra_path_dir: Option<&Path>,
) -> Output {
    let script = workspace_root().join("scripts/verify-release-packages.sh");
    assert!(script.is_file(), "missing script {}", script.display());

    let mut cmd = Command::new("bash");
    cmd.arg(&script)
        .current_dir(workspace_root())
        .env("ORGFMT_DIST_DIR", dist_dir)
        .env("FRITZ_TRACE_ID", TEST_TRACE_ID);
    if let Some(metrics) = metrics_file {
        cmd.env("ORGFMT_RELEASE_METRICS_FILE", metrics);
    }
    if let Some(events) = events_file {
        cmd.env("ORGFMT_RELEASE_EVENTS_FILE", events);
    }
    if let Some(bin_dir) = extra_path_dir {
        let current_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{current_path}", bin_dir.display()));
    }
    cmd.output().expect("execute verify-release-packages.sh")
}

/// Verifies `scripts/verify-release-packages.sh` passes on valid `dist/` and emits Prometheus + JSONL telemetry.
#[cfg(unix)]
#[test]
fn verify_release_packages_accepts_valid_dist_and_emits_telemetry() {
    let temp = TempDir::new().expect("create synthetic release workspace");
    let dist_dir = temp.path().join("dist");
    populate_synthetic_dist(&dist_dir);

    let metrics_file = temp.path().join("nested_metrics/sub/release.prom");
    let events_file = temp.path().join("nested_events/sub/release-events.jsonl");
    let output = run_verify_script(&dist_dir, Some(&metrics_file), Some(&events_file), None);

    assert!(
        output.status.success(),
        "expected exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be valid JSON summary");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["verified_artifacts"], 12);

    let metrics = fs::read_to_string(&metrics_file).expect("read Prometheus metrics file");
    for metric_name in [
        "orgfmt_release_verification_duration_seconds",
        "orgfmt_release_artifacts_verified_total",
        "orgfmt_release_artifact_size_bytes",
    ] {
        assert!(
            metrics.contains(metric_name),
            "missing metric {metric_name}"
        );
    }

    let events_raw = fs::read_to_string(&events_file).expect("read JSONL events file");
    let events: Vec<Value> = events_raw
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSONL event line"))
        .collect();
    assert!(!events.is_empty(), "expected structured JSONL events");
    assert!(events.iter().all(|ev| ev["trace_id"] == TEST_TRACE_ID));
}

/// Asserts that a tampered archive fails checksum validation without marking `checksums.txt` verified.
#[cfg(unix)]
fn assert_corrupted_checksum_not_marked_verified(output: &Output, metrics_path: &Path) {
    assert!(
        !output.status.success(),
        "corrupted SHA-256 checksum must cause non-zero exit"
    );
    let summary: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be valid JSON summary");
    assert_eq!(
        summary["verified_artifacts"], 11,
        "corrupted checksums.txt must not increment verified_artifacts"
    );
    assert_eq!(summary["failed_checks"], 1);

    let metrics = fs::read_to_string(metrics_path).expect("read corrupted-run metrics file");
    assert!(
        metrics.contains("orgfmt_release_artifacts_verified_total{status=\"verified\"} 11"),
        "expected 11 verified artifacts in metrics, got:\n{metrics}"
    );
    assert!(
        metrics.contains("orgfmt_release_artifacts_verified_total{status=\"failed\"} 1"),
        "expected 1 failed check in metrics, got:\n{metrics}"
    );
    assert!(
        !metrics.contains("target=\"sha256_manifest\""),
        "failed checksums.txt must not emit verified size metric:\n{metrics}"
    );
}

/// Verifies `scripts/verify-release-packages.sh` exits non-zero when a `.deb` is missing or a checksum is bad.
#[cfg(unix)]
#[test]
fn verify_release_packages_rejects_missing_deb_or_corrupted_checksum() {
    let missing_deb_temp = TempDir::new().expect("create missing-deb temp dir");
    let missing_deb_dist = missing_deb_temp.path().join("dist");
    populate_synthetic_dist(&missing_deb_dist);
    fs::remove_file(
        missing_deb_dist.join(format!("org-tools_{SYNTHETIC_VERSION}_linux_amd64.deb")),
    )
    .expect("remove amd64 deb");

    let missing_deb_out = run_verify_script(&missing_deb_dist, None, None, None);
    assert!(
        !missing_deb_out.status.success(),
        "missing .deb package must cause non-zero exit"
    );

    let bad_sum_temp = TempDir::new().expect("create bad-checksum temp dir");
    let bad_sum_dist = bad_sum_temp.path().join("dist");
    populate_synthetic_dist(&bad_sum_dist);
    fs::write(
        bad_sum_dist.join(format!("org-tools_{SYNTHETIC_VERSION}_darwin_arm64.tar.gz")),
        b"tampered-archive-bytes",
    )
    .expect("tamper archive payload");

    let bad_sum_metrics = bad_sum_temp.path().join("nested_metrics/corrupted.prom");
    let bad_sum_out = run_verify_script(&bad_sum_dist, Some(&bad_sum_metrics), None, None);
    assert_corrupted_checksum_not_marked_verified(&bad_sum_out, &bad_sum_metrics);
}

/// Verifies `scripts/verify-release-packages.sh` automatically runs `goreleaser check` when found on `PATH`.
#[cfg(unix)]
#[test]
fn verify_release_packages_invokes_goreleaser_check_when_on_path() {
    let temp = TempDir::new().expect("create goreleaser shim workspace");
    let dist_dir = temp.path().join("dist");
    populate_synthetic_dist(&dist_dir);

    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create shim bin dir");
    let shim_path = bin_dir.join("goreleaser");
    fs::write(
        &shim_path,
        "#!/usr/bin/env bash\n[ \"$1\" = \"check\" ] && exit 0\nexit 1\n",
    )
    .expect("write passing goreleaser shim");
    fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755)).expect("chmod +x shim");

    let pass_out = run_verify_script(&dist_dir, None, None, Some(&bin_dir));
    assert!(pass_out.status.success(), "expected passing shim exit 0");
    let pass_summary: Value = serde_json::from_slice(&pass_out.stdout).expect("parse summary");
    assert_eq!(pass_summary["goreleaser_checked"], true);

    fs::write(
        &shim_path,
        "#!/usr/bin/env bash\necho 'invalid' >&2\nexit 1\n",
    )
    .expect("write failing goreleaser shim");
    let fail_out = run_verify_script(&dist_dir, None, None, Some(&bin_dir));
    assert!(
        !fail_out.status.success(),
        "failing `goreleaser check` must fail release verification"
    );
}

// endregion
