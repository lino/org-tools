#!/usr/bin/env bash
# Copyright (C) 2026 org-tools contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Validates or executes an org-tools release: verifies X.Y.Z semver format and
# Cargo workspace manifest version synchronization, optionally checks
# `.goreleaser.yaml` when `goreleaser` is installed, and (when not `--dry-run`)
# bumps manifests, runs verification checks, commits, tags, and pushes.
#
# Environment Variables:
#   ORGFMT_REPO_DIR            Repository root directory (default: .).
#   ORGFMT_GORELEASER_CONFIG   GoReleaser YAML path relative to repo root (default: .goreleaser.yaml).
#   ORGFMT_GORELEASER_VERSION  Expected GoReleaser version reference (default: v2.18.0).
#   ORGFMT_GIT_REMOTE          Git remote name for release push (default: origin).
#   ORGFMT_GIT_BRANCH          Git branch name for release push (default: main).

set -euo pipefail

# region Configuration & Defaults
DEFAULT_REPO_DIR="."
DEFAULT_GORELEASER_CONFIG=".goreleaser.yaml"
DEFAULT_GORELEASER_VERSION="v2.18.0"
DEFAULT_GIT_REMOTE="origin"
DEFAULT_GIT_BRANCH="main"
CLI_MANIFEST_REL_PATH="crates/org/Cargo.toml"
ROOT_MANIFEST_REL_PATH="Cargo.toml"

REPO_DIR="${ORGFMT_REPO_DIR:-$DEFAULT_REPO_DIR}"
GORELEASER_CONFIG="${ORGFMT_GORELEASER_CONFIG:-$DEFAULT_GORELEASER_CONFIG}"
GORELEASER_VERSION="${ORGFMT_GORELEASER_VERSION:-$DEFAULT_GORELEASER_VERSION}"
GIT_REMOTE="${ORGFMT_GIT_REMOTE:-$DEFAULT_GIT_REMOTE}"
GIT_BRANCH="${ORGFMT_GIT_BRANCH:-$DEFAULT_GIT_BRANCH}"

DRY_RUN="false"
ASSUME_YES="false"
REQUESTED_VERSION=""
# endregion

# region CLI Argument Parsing

# Prints command-line usage instructions to stdout.
print_usage() {
  cat <<'EOF'
Usage: release.sh [OPTIONS] [VERSION]

Bump workspace version, verify manifests and GoReleaser config, tag, and push.

Arguments:
  [VERSION]            Explicit target version in X.Y.Z format (default: bump patch)

Options:
  -n, --dry-run        Validate semver, manifest sync, and goreleaser config without mutating files
  -y, --yes            Skip interactive confirmation prompt during live releases
      --repo-dir <DIR> Repository root path (overrides ORGFMT_REPO_DIR)
      --config <FILE>  GoReleaser config path (overrides ORGFMT_GORELEASER_CONFIG)
  -h, --help           Show this help message and exit
EOF
}

# Exits with status 2 when a value-taking CLI option is missing its argument.
assert_option_value() {
  if [[ $# -lt 2 ]]; then
    echo "error: option $1 requires a value" >&2
    exit 2
  fi
}

# Parses CLI flags and optional positional target version into script globals.
parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -n|--dry-run)  DRY_RUN="true"; shift ;;
      -y|--yes)      ASSUME_YES="true"; shift ;;
      --repo-dir)    assert_option_value "$@"; REPO_DIR="$2"; shift 2 ;;
      --config)      assert_option_value "$@"; GORELEASER_CONFIG="$2"; shift 2 ;;
      -h|--help)     print_usage; exit 0 ;;
      -*)
        echo "error: unknown option: $1" >&2
        print_usage >&2
        exit 2
        ;;
      *)
        if [[ -n "$REQUESTED_VERSION" ]]; then
          echo "error: unexpected extra argument: $1" >&2
          exit 2
        fi
        REQUESTED_VERSION="$1"
        shift
        ;;
    esac
  done
}
# endregion

# region Version & Manifest Helpers

# Validates that a version string strictly matches `X.Y.Z` numeric semver format.
validate_semver_xyz() {
  local candidate="$1"
  local label="$2"
  if [[ ! "$candidate" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: invalid ${label} '${candidate}'; expected strict X.Y.Z numeric semver (e.g. 0.2.0)" >&2
    exit 1
  fi
}

# Extracts `[workspace.package]` version from the root `Cargo.toml` manifest.
extract_workspace_version() {
  local manifest_path="$1"
  awk '
    /^\[workspace\.package\]$/ { in_ws = 1; next }
    /^\[/ { in_ws = 0 }
    in_ws && /^version *= *"/ {
      sub(/^version *= *"/, "")
      sub(/".*$/, "")
      print
      exit
    }
  ' "$manifest_path"
}

# Extracts the `org-tools-core` dependency version from `crates/org/Cargo.toml`.
extract_cli_core_dep_version() {
  local cli_manifest_path="$1"
  awk '
    /^org-tools-core *=.*version *= *"/ {
      line = $0
      sub(/^.*version *= *"/, "", line)
      sub(/".*$/, "", line)
      print line
      exit
    }
  ' "$cli_manifest_path"
}

# Computes the target `X.Y.Z` release version from current version and optional CLI input.
resolve_next_version() {
  local current_ver="$1"
  if [[ -n "$REQUESTED_VERSION" ]]; then
    validate_semver_xyz "$REQUESTED_VERSION" "target version"
    printf '%s\n' "$REQUESTED_VERSION"
    return 0
  fi
  local major minor patch
  IFS='.' read -r major minor patch <<< "$current_ver"
  printf '%s.%s.%s\n' "$major" "$minor" "$((patch + 1))"
}

# Renders an updated root `Cargo.toml` with `[workspace.package]` version set to `new_ver`.
render_bumped_workspace_manifest() {
  local src_manifest="$1"
  local old_ver="$2"
  local new_ver="$3"
  local dst_manifest="$4"
  awk -v old_v="$old_ver" -v new_v="$new_ver" '
    /^\[workspace\.package\]$/ { in_ws = 1 }
    /^\[/ && $0 != "[workspace.package]" { in_ws = 0 }
    in_ws && $0 == "version = \"" old_v "\"" {
      print "version = \"" new_v "\""
      next
    }
    { print }
  ' "$src_manifest" > "$dst_manifest"
}

# Renders an updated `crates/org/Cargo.toml` with `org-tools-core` version set to `new_ver`.
render_bumped_cli_manifest() {
  local src_manifest="$1"
  local old_ver="$2"
  local new_ver="$3"
  local dst_manifest="$4"
  awk -v old_v="$old_ver" -v new_v="$new_ver" '
    BEGIN {
      needle = "version = \"" old_v "\""
      repl = "version = \"" new_v "\""
    }
    /^org-tools-core *=/ {
      idx = index($0, needle)
      if (idx > 0) {
        $0 = substr($0, 1, idx - 1) repl substr($0, idx + length(needle))
      }
    }
    { print }
  ' "$src_manifest" > "$dst_manifest"
}

# Verifies current `Cargo.toml` and `crates/org/Cargo.toml` versions exist, match X.Y.Z, and are in sync.
read_and_verify_synced_version() {
  if [[ ! -f "$ROOT_MANIFEST_REL_PATH" || ! -f "$CLI_MANIFEST_REL_PATH" ]]; then
    echo "error: missing ${ROOT_MANIFEST_REL_PATH} or ${CLI_MANIFEST_REL_PATH} in $(pwd)" >&2
    exit 1
  fi
  local ws_ver cli_ver
  ws_ver="$(extract_workspace_version "$ROOT_MANIFEST_REL_PATH")"
  cli_ver="$(extract_cli_core_dep_version "$CLI_MANIFEST_REL_PATH")"
  validate_semver_xyz "$ws_ver" "Cargo.toml workspace version"
  validate_semver_xyz "$cli_ver" "crates/org/Cargo.toml org-tools-core version"
  if [[ "$ws_ver" != "$cli_ver" ]]; then
    echo "error: version sync mismatch between Cargo.toml (${ws_ver}) and ${CLI_MANIFEST_REL_PATH} (${cli_ver})" >&2
    exit 1
  fi
  printf '%s\n' "$ws_ver"
}
# endregion

# region Dry-Run & Live Release Execution

# Simulates manifest updates in a temporary directory without mutating tracked repository files.
simulate_manifest_bump() {
  local current_ver="$1"
  local next_ver="$2"
  local sim_dir
  sim_dir="$(mktemp -d)"
  render_bumped_workspace_manifest "$ROOT_MANIFEST_REL_PATH" "$current_ver" "$next_ver" "${sim_dir}/Cargo.toml"
  render_bumped_cli_manifest "$CLI_MANIFEST_REL_PATH" "$current_ver" "$next_ver" "${sim_dir}/cli-Cargo.toml"

  local sim_ws sim_cli
  sim_ws="$(extract_workspace_version "${sim_dir}/Cargo.toml")"
  sim_cli="$(extract_cli_core_dep_version "${sim_dir}/cli-Cargo.toml")"
  rm -rf "$sim_dir"

  if [[ "$sim_ws" != "$next_ver" || "$sim_cli" != "$next_ver" ]]; then
    echo "error: simulated manifest sync failed (root=${sim_ws}, cli=${sim_cli}, expected=${next_ver})" >&2
    exit 1
  fi
}

# Runs `goreleaser check` when the `goreleaser` binary is available on PATH.
verify_goreleaser_if_installed() {
  if ! command -v goreleaser >/dev/null 2>&1; then
    echo "goreleaser not found on PATH; skipping goreleaser check (pipeline uses ${GORELEASER_VERSION})."
    return 0
  fi
  if [[ ! -f "$GORELEASER_CONFIG" ]]; then
    echo "error: GoReleaser config not found at ${GORELEASER_CONFIG}" >&2
    exit 1
  fi
  goreleaser check -f "$GORELEASER_CONFIG"
}

# Prompts interactively for operator confirmation unless `--yes` was provided.
confirm_live_release() {
  local next_ver="$1"
  if [[ "$ASSUME_YES" == "true" ]]; then
    return 0
  fi
  local confirm=""
  read -rp "Proceed with release v${next_ver}? [y/N] " confirm
  if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
    echo "Aborted." >&2
    exit 1
  fi
}

# Applies version updates, runs Cargo checks/tests/clippy, commits, tags, and pushes.
execute_live_release() {
  local current_ver="$1"
  local next_ver="$2"
  confirm_live_release "$next_ver"

  if [[ -n "$(git status --porcelain)" ]]; then
    echo "error: working tree is not clean; commit or stash changes first" >&2
    exit 1
  fi

  local tmp_dir
  tmp_dir="$(mktemp -d)"
  render_bumped_workspace_manifest "$ROOT_MANIFEST_REL_PATH" "$current_ver" "$next_ver" "${tmp_dir}/Cargo.toml"
  render_bumped_cli_manifest "$CLI_MANIFEST_REL_PATH" "$current_ver" "$next_ver" "${tmp_dir}/cli-Cargo.toml"
  mv "${tmp_dir}/Cargo.toml" "$ROOT_MANIFEST_REL_PATH"
  mv "${tmp_dir}/cli-Cargo.toml" "$CLI_MANIFEST_REL_PATH"
  rmdir "$tmp_dir"

  cargo check
  cargo test
  cargo clippy -- -D warnings

  git add "$ROOT_MANIFEST_REL_PATH" Cargo.lock "$CLI_MANIFEST_REL_PATH"
  git commit -m "chore: bump version to ${next_ver}"
  git tag -a "v${next_ver}" -m "Release v${next_ver}"
  git push "$GIT_REMOTE" "$GIT_BRANCH"
  git push "$GIT_REMOTE" "v${next_ver}"
  echo "Released v${next_ver} — GitHub Actions will build packages and run the crates.io gate."
}
# endregion

# region Entrypoint

# Orchestrates CLI argument parsing, manifest validation, dry-run checks, or live release execution.
main() {
  parse_args "$@"
  if [[ ! -d "$REPO_DIR" ]]; then
    echo "error: repository directory does not exist: ${REPO_DIR}" >&2
    exit 1
  fi
  cd "$REPO_DIR"

  local current_ver next_ver
  current_ver="$(read_and_verify_synced_version)"
  next_ver="$(resolve_next_version "$current_ver")"

  echo "Current version: ${current_ver}"
  echo "Target version:  ${next_ver} (tag v${next_ver})"

  simulate_manifest_bump "$current_ver" "$next_ver"
  verify_goreleaser_if_installed

  if [[ "$DRY_RUN" == "true" ]]; then
    echo "Dry run succeeded: ${ROOT_MANIFEST_REL_PATH} and ${CLI_MANIFEST_REL_PATH} verified for ${current_ver} -> ${next_ver} without modifying files."
    exit 0
  fi

  execute_live_release "$current_ver" "$next_ver"
}

main "$@"
# endregion
