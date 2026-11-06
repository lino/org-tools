#!/usr/bin/env bash
# Copyright (C) 2026 org-tools contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Verifies GoReleaser v2 release artifacts in `dist/`, validates SHA-256 checksums,
# inspects `.deb`/`.rpm` packages and the Homebrew Cask spec, optionally runs
# `goreleaser check` when `goreleaser` is available on PATH, and emits Prometheus
# textfile metrics plus structured JSONL telemetry events.
#
# Environment Variables:
#   ORGFMT_DIST_DIR              Path to release artifact directory (default: dist).
#   ORGFMT_GORELEASER_CONFIG     Path to GoReleaser YAML config (default: .goreleaser.yaml).
#   ORGFMT_PACKAGE_NAME          Expected package/cask stem (default: org-tools).
#   ORGFMT_BINARY_NAME           Expected binary name inside cask (default: org).
#   ORGFMT_RELEASE_METRICS_FILE  Optional path to write Prometheus exposition metrics.
#   ORGFMT_RELEASE_EVENTS_FILE   Optional path to write structured JSONL telemetry events.
#   HOMEBREW_TAP_OWNER           Homebrew tap GitHub owner (default: lino).
#   HOMEBREW_TAP_NAME            Homebrew tap repository name (default: homebrew-tap).
#   FRITZ_TRACE_ID               Optional W3C trace identifier propagated to JSONL events.

set -euo pipefail

# region Configuration & Defaults
DEFAULT_DIST_DIR="dist"
DEFAULT_GORELEASER_CONFIG=".goreleaser.yaml"
DEFAULT_PACKAGE_NAME="org-tools"
DEFAULT_BINARY_NAME="org"
DEFAULT_TAP_OWNER="lino"
DEFAULT_TAP_NAME="homebrew-tap"
DEFAULT_TRACE_ID="untraced"
QUARANTINE_ATTR="com.apple.quarantine"

DIST_DIR="${ORGFMT_DIST_DIR:-$DEFAULT_DIST_DIR}"
GORELEASER_CONFIG="${ORGFMT_GORELEASER_CONFIG:-$DEFAULT_GORELEASER_CONFIG}"
PACKAGE_NAME="${ORGFMT_PACKAGE_NAME:-$DEFAULT_PACKAGE_NAME}"
BINARY_NAME="${ORGFMT_BINARY_NAME:-$DEFAULT_BINARY_NAME}"
METRICS_FILE="${ORGFMT_RELEASE_METRICS_FILE:-}"
EVENTS_FILE="${ORGFMT_RELEASE_EVENTS_FILE:-}"
TAP_OWNER="${HOMEBREW_TAP_OWNER:-$DEFAULT_TAP_OWNER}"
TAP_NAME="${HOMEBREW_TAP_NAME:-$DEFAULT_TAP_NAME}"
TRACE_ID="${FRITZ_TRACE_ID:-$DEFAULT_TRACE_ID}"

VERIFIED_COUNT=0
FAILED_COUNT=0
GORELEASER_CHECKED="false"
METRIC_SIZE_LINES=""
VERIFIED_ARTIFACT_BASENAMES=""
# endregion

# region Observability & Telemetry Helpers

# Returns high-resolution epoch seconds via perl when available, falling back to integer seconds.
now_epoch_seconds() {
  if command -v perl >/dev/null 2>&1; then
    perl -MTime::HiRes=time -e 'printf "%.6f\n", time'
  else
    date +%s
  fi
}

# Computes elapsed seconds between two epoch timestamps.
elapsed_seconds() {
  local start_ts="$1"
  local end_ts="$2"
  awk -v s="$start_ts" -v e="$end_ts" 'BEGIN { d = e - s; if (d < 0) d = 0; printf "%.6f", d }'
}

# Escapes backslashes, double quotes, and control whitespace for safe JSON string interpolation.
json_escape() {
  local val="$1"
  val="${val//\\/\\\\}"
  val="${val//\"/\\\"}"
  val="${val//$'\n'/\\n}"
  val="${val//$'\r'/\\r}"
  val="${val//$'\t'/\\t}"
  printf '%s' "$val"
}

# Appends a single structured JSONL event line when ORGFMT_RELEASE_EVENTS_FILE is configured.
emit_jsonl_event() {
  [[ -n "$EVENTS_FILE" ]] || return 0
  local event_name="$1"
  local status_val="$2"
  local detail_msg="$3"
  local duration_val="${4:-0}"
  local ts
  ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  local escaped_msg escaped_tap escaped_trace
  escaped_msg="$(json_escape "$detail_msg")"
  escaped_tap="$(json_escape "${TAP_OWNER}/${TAP_NAME}")"
  escaped_trace="$(json_escape "$TRACE_ID")"

  mkdir -p "$(dirname "$EVENTS_FILE")"
  printf '{"timestamp":"%s","event":"%s","package":"%s","tap":"%s","trace_id":"%s","status":"%s","verified_count":%d,"failed_count":%d,"duration_seconds":%s,"message":"%s"}\n' \
    "$ts" "$event_name" "$(json_escape "$PACKAGE_NAME")" "$escaped_tap" "$escaped_trace" \
    "$status_val" "$VERIFIED_COUNT" "$FAILED_COUNT" "$duration_val" "$escaped_msg" >> "$EVENTS_FILE"
}

# Records a verified artifact, updating counters, Prometheus size gauges, and JSONL telemetry.
record_verified_artifact() {
  local target_label="$1"
  local kind_label="$2"
  local file_path="$3"
  local track_checksum="${4:-false}"
  local size_bytes
  size_bytes="$(file_size_bytes "$file_path")"
  local base_name
  base_name="$(basename "$file_path")"

  VERIFIED_COUNT=$((VERIFIED_COUNT + 1))
  METRIC_SIZE_LINES="${METRIC_SIZE_LINES}orgfmt_release_artifact_size_bytes{target=\"${target_label}\",kind=\"${kind_label}\"} ${size_bytes}"$'\n'
  if [[ "$track_checksum" == "true" ]]; then
    VERIFIED_ARTIFACT_BASENAMES="${VERIFIED_ARTIFACT_BASENAMES}${base_name}"$'\n'
  fi
  emit_jsonl_event "release.artifact.verified" "ok" "verified ${kind_label} ${base_name} (${size_bytes} bytes)"
}

# Records a verification failure to stderr, failure counters, and JSONL telemetry.
record_failure() {
  local reason="$1"
  FAILED_COUNT=$((FAILED_COUNT + 1))
  echo "verify-release-packages: FAIL: $reason" >&2
  emit_jsonl_event "release.check.failed" "error" "$reason"
}

# Writes Prometheus exposition format metrics when ORGFMT_RELEASE_METRICS_FILE is configured.
write_prometheus_metrics() {
  local duration_val="$1"
  local success_flag="$2"
  [[ -n "$METRICS_FILE" ]] || return 0

  mkdir -p "$(dirname "$METRICS_FILE")"
  {
    echo "# HELP orgfmt_release_verification_duration_seconds Wall-clock duration of release package verification in seconds."
    echo "# TYPE orgfmt_release_verification_duration_seconds gauge"
    echo "orgfmt_release_verification_duration_seconds ${duration_val}"
    echo "# HELP orgfmt_release_artifacts_verified_total Total number of release checks grouped by verification outcome."
    echo "# TYPE orgfmt_release_artifacts_verified_total counter"
    echo "orgfmt_release_artifacts_verified_total{status=\"verified\"} ${VERIFIED_COUNT}"
    echo "orgfmt_release_artifacts_verified_total{status=\"failed\"} ${FAILED_COUNT}"
    echo "# HELP orgfmt_release_artifact_size_bytes Size in bytes of each verified release artifact."
    echo "# TYPE orgfmt_release_artifact_size_bytes gauge"
    if [[ -n "$METRIC_SIZE_LINES" ]]; then
      printf '%s' "$METRIC_SIZE_LINES"
    fi
    echo "# HELP orgfmt_release_verification_success Whether release package verification succeeded (1 = pass, 0 = fail)."
    echo "# TYPE orgfmt_release_verification_success gauge"
    echo "orgfmt_release_verification_success ${success_flag}"
  } > "$METRICS_FILE"
}
# endregion

# region CLI Argument Parsing

# Prints CLI usage instructions to stdout.
print_usage() {
  cat <<'EOF'
Usage: verify-release-packages.sh [OPTIONS] [DIST_DIR]

Verify GoReleaser multi-arch archives, SHA-256 checksums, .deb/.rpm packages, and Homebrew cask.

Options:
  --dist-dir <DIR>       Artifact directory (overrides ORGFMT_DIST_DIR)
  --config <FILE>        GoReleaser YAML path (overrides ORGFMT_GORELEASER_CONFIG)
  --package-name <NAME>  Package and cask name (overrides ORGFMT_PACKAGE_NAME)
  --metrics-file <FILE>  Prometheus textfile output (overrides ORGFMT_RELEASE_METRICS_FILE)
  --events-file <FILE>   JSONL telemetry output (overrides ORGFMT_RELEASE_EVENTS_FILE)
  --tap-owner <OWNER>    Homebrew tap owner (overrides HOMEBREW_TAP_OWNER)
  --tap-name <NAME>      Homebrew tap repo name (overrides HOMEBREW_TAP_NAME)
  -h, --help             Show this help message and exit
EOF
}

# Exits with status 2 when a value-taking CLI flag is missing its argument.
assert_flag_has_value() {
  if [[ $# -lt 2 ]]; then
    echo "error: $1 requires a value" >&2
    exit 2
  fi
}

# Applies telemetry and Homebrew tap CLI flags, returning 0 when matched or 1 otherwise.
parse_telemetry_or_tap_flag() {
  case "$1" in
    --metrics-file) assert_flag_has_value "$@"; METRICS_FILE="$2" ;;
    --events-file)  assert_flag_has_value "$@"; EVENTS_FILE="$2" ;;
    --tap-owner)    assert_flag_has_value "$@"; TAP_OWNER="$2" ;;
    --tap-name)     assert_flag_has_value "$@"; TAP_NAME="$2" ;;
    *)              return 1 ;;
  esac
  return 0
}

# Parses command-line arguments and overrides script configuration globals.
parse_args() {
  while [[ $# -gt 0 ]]; do
    if parse_telemetry_or_tap_flag "$@"; then
      shift 2
      continue
    fi
    case "$1" in
      --dist-dir)     assert_flag_has_value "$@"; DIST_DIR="$2"; shift 2 ;;
      --config)       assert_flag_has_value "$@"; GORELEASER_CONFIG="$2"; shift 2 ;;
      --package-name) assert_flag_has_value "$@"; PACKAGE_NAME="$2"; shift 2 ;;
      -h|--help)      print_usage; exit 0 ;;
      -*)
        echo "error: unknown option: $1" >&2
        print_usage >&2
        exit 2
        ;;
      *)
        DIST_DIR="$1"
        shift
        ;;
    esac
  done
}
# endregion

# region Checksum & Artifact Validators

# Returns portable file size in bytes across BSD/macOS and GNU/Linux userlands.
file_size_bytes() {
  local target_file="$1"
  wc -c < "$target_file" | tr -d '[:space:]'
}

# Computes lowercase SHA-256 hex digest using shasum or sha256sum.
compute_sha256() {
  local target_file="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$target_file" | awk '{print tolower($1)}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$target_file" | awk '{print tolower($1)}'
  else
    echo "error: neither shasum nor sha256sum is available on PATH" >&2
    return 1
  fi
}

# Finds the first regular file in DIST_DIR matching either primary or fallback glob pattern.
find_dist_artifact() {
  local primary_glob="$1"
  local fallback_glob="$2"
  local candidate=""
  candidate="$(find "$DIST_DIR" -maxdepth 1 -type f \( -name "$primary_glob" -o -name "$fallback_glob" \) | sort | head -n 1)"
  printf '%s' "$candidate"
}

# Verifies that a single expected target artifact exists in DIST_DIR and is non-empty.
verify_expected_artifact() {
  local target_label="$1"
  local kind_label="$2"
  local primary_glob="$3"
  local fallback_glob="$4"
  local found_path
  found_path="$(find_dist_artifact "$primary_glob" "$fallback_glob")"

  if [[ -z "$found_path" || ! -f "$found_path" ]]; then
    record_failure "missing ${kind_label} artifact for ${target_label} (expected ${primary_glob})"
    return 0
  fi
  if [[ ! -s "$found_path" ]]; then
    record_failure "empty ${kind_label} artifact for ${target_label}: $(basename "$found_path")"
    return 0
  fi
  record_verified_artifact "$target_label" "$kind_label" "$found_path" "true"
}
# endregion

# region Release Matrix Verification

# Runs `goreleaser check` automatically when the `goreleaser` binary is present on PATH.
verify_goreleaser_config_if_available() {
  if ! command -v goreleaser >/dev/null 2>&1; then
    return 0
  fi
  if [[ ! -f "$GORELEASER_CONFIG" ]]; then
    record_failure "goreleaser found on PATH but config file is missing: ${GORELEASER_CONFIG}"
    return 0
  fi

  local check_output=""
  local check_rc=0
  check_output="$(goreleaser check -f "$GORELEASER_CONFIG" 2>&1)" || check_rc=$?
  if [[ "$check_rc" -eq 0 ]]; then
    GORELEASER_CHECKED="true"
    emit_jsonl_event "release.goreleaser_check.passed" "ok" "goreleaser check validated ${GORELEASER_CONFIG}"
  else
    GORELEASER_CHECKED="true"
    record_failure "goreleaser check failed (exit ${check_rc}): ${check_output}"
  fi
}

# Verifies all 6 multi-arch archives plus Linux .deb and .rpm packages in DIST_DIR.
verify_archives_and_linux_packages() {
  verify_expected_artifact "darwin_arm64"  "archive" "${PACKAGE_NAME}_*_darwin_arm64.tar.gz"  "*aarch64*apple-darwin*.tar.gz"
  verify_expected_artifact "darwin_amd64"  "archive" "${PACKAGE_NAME}_*_darwin_amd64.tar.gz"  "*x86_64*apple-darwin*.tar.gz"
  verify_expected_artifact "linux_amd64"   "archive" "${PACKAGE_NAME}_*_linux_amd64.tar.gz"   "*x86_64*linux*.tar.gz"
  verify_expected_artifact "linux_arm64"   "archive" "${PACKAGE_NAME}_*_linux_arm64.tar.gz"   "*aarch64*linux*.tar.gz"
  verify_expected_artifact "windows_amd64" "archive" "${PACKAGE_NAME}_*_windows_amd64.zip"    "*x86_64*windows*.zip"
  verify_expected_artifact "windows_arm64" "archive" "${PACKAGE_NAME}_*_windows_arm64.zip"    "*aarch64*windows*.zip"

  verify_expected_artifact "linux_amd64"   "deb"     "${PACKAGE_NAME}_*amd64*.deb"            "${PACKAGE_NAME}*x86_64*.deb"
  verify_expected_artifact "linux_arm64"   "deb"     "${PACKAGE_NAME}_*arm64*.deb"            "${PACKAGE_NAME}*aarch64*.deb"
  verify_expected_artifact "linux_amd64"   "rpm"     "${PACKAGE_NAME}_*amd64*.rpm"            "${PACKAGE_NAME}*x86_64*.rpm"
  verify_expected_artifact "linux_arm64"   "rpm"     "${PACKAGE_NAME}_*arm64*.rpm"            "${PACKAGE_NAME}*aarch64*.rpm"
}

# Verifies the generated Homebrew Cask file and its macOS quarantine post-install hook.
verify_homebrew_cask() {
  local cask_path=""
  cask_path="$(find "$DIST_DIR" -type f -name "${PACKAGE_NAME}.rb" | sort | head -n 1)"
  if [[ -z "$cask_path" || ! -s "$cask_path" ]]; then
    record_failure "missing or empty Homebrew cask ${PACKAGE_NAME}.rb under ${DIST_DIR}"
    return 0
  fi

  if ! grep -q "cask \"${PACKAGE_NAME}\"" "$cask_path"; then
    record_failure "Homebrew cask ${cask_path} missing cask \"${PACKAGE_NAME}\" header"
    return 0
  fi
  if ! grep -q "binary \"${BINARY_NAME}\"" "$cask_path"; then
    record_failure "Homebrew cask ${cask_path} missing binary \"${BINARY_NAME}\" declaration"
    return 0
  fi
  if ! grep -q "$QUARANTINE_ATTR" "$cask_path"; then
    record_failure "Homebrew cask ${cask_path} missing ${QUARANTINE_ATTR} postflight hook"
    return 0
  fi
  record_verified_artifact "homebrew_cask" "cask" "$cask_path" "false"
}

# Validates a single non-empty checksums.txt line against the corresponding dist file.
verify_single_checksum_line() {
  local line="$1"
  local expected_hash rel_name clean_name actual_hash
  expected_hash="$(printf '%s\n' "$line" | awk '{print tolower($1)}')"
  rel_name="$(printf '%s\n' "$line" | awk '{print $2}')"
  clean_name="${rel_name#\*}"
  clean_name="${clean_name#./}"
  if [[ ! "$expected_hash" =~ ^[0-9a-f]{64}$ ]]; then
    record_failure "malformed SHA-256 digest in checksums.txt for ${clean_name}"
    return 0
  fi
  if [[ ! -f "${DIST_DIR}/${clean_name}" ]]; then
    record_failure "checksums.txt references missing file: ${clean_name}"
    return 0
  fi
  actual_hash="$(compute_sha256 "${DIST_DIR}/${clean_name}")"
  if [[ "$actual_hash" != "$expected_hash" ]]; then
    record_failure "SHA-256 mismatch for ${clean_name}: expected ${expected_hash}, got ${actual_hash}"
  fi
}

# Validates every entry in checksums.txt and ensures all verified packages are covered.
verify_checksums_manifest() {
  local checksum_file="${DIST_DIR}/checksums.txt"
  if [[ ! -s "$checksum_file" ]]; then
    record_failure "missing or empty checksum manifest: ${checksum_file}"
    return 0
  fi

  local failures_before="$FAILED_COUNT"
  local line
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -n "${line// /}" ]] || continue
    verify_single_checksum_line "$line"
  done < "$checksum_file"

  verify_checksum_coverage "$checksum_file"
  if [[ "$FAILED_COUNT" -eq "$failures_before" ]]; then
    record_verified_artifact "sha256_manifest" "checksum" "$checksum_file" "false"
  fi
}

# Confirms that every verified archive and Linux package appears in checksums.txt.
verify_checksum_coverage() {
  local checksum_file="$1"
  local artifact_name
  while IFS= read -r artifact_name; do
    [[ -n "$artifact_name" ]] || continue
    if ! awk '{ sub(/^\*/, "", $2); sub(/^\.\//, "", $2); print $2 }' "$checksum_file" | grep -Fxq "$artifact_name"; then
      record_failure "verified artifact missing from checksums.txt: ${artifact_name}"
    fi
  done <<< "$VERIFIED_ARTIFACT_BASENAMES"
}
# endregion

# region Entrypoint

# Executes release verification, emits telemetry and JSON summary, and exits with status.
main() {
  parse_args "$@"
  if [[ -n "$EVENTS_FILE" ]]; then
    mkdir -p "$(dirname "$EVENTS_FILE")"
    : > "$EVENTS_FILE"
  fi
  if [[ -n "$METRICS_FILE" ]]; then
    mkdir -p "$(dirname "$METRICS_FILE")"
  fi

  local start_ts end_ts duration_sec overall_status success_num exit_code
  start_ts="$(now_epoch_seconds)"
  emit_jsonl_event "release.verification.started" "ok" "starting release verification in ${DIST_DIR}"

  verify_goreleaser_config_if_available
  if [[ ! -d "$DIST_DIR" ]]; then
    record_failure "dist directory does not exist: ${DIST_DIR}"
  else
    verify_archives_and_linux_packages
    verify_homebrew_cask
    verify_checksums_manifest
  fi

  end_ts="$(now_epoch_seconds)"
  duration_sec="$(elapsed_seconds "$start_ts" "$end_ts")"
  if [[ "$FAILED_COUNT" -eq 0 ]]; then
    overall_status="ok"
    success_num=1
    exit_code=0
  else
    overall_status="error"
    success_num=0
    exit_code=1
  fi

  write_prometheus_metrics "$duration_sec" "$success_num"
  emit_jsonl_event "release.verification.completed" "$overall_status" \
    "completed verification: ${VERIFIED_COUNT} verified, ${FAILED_COUNT} failed" "$duration_sec"

  printf '{"status":"%s","package":"%s","dist_dir":"%s","tap":"%s/%s","goreleaser_checked":%s,"verified_artifacts":%d,"failed_checks":%d,"duration_seconds":%s}\n' \
    "$overall_status" "$(json_escape "$PACKAGE_NAME")" "$(json_escape "$DIST_DIR")" \
    "$(json_escape "$TAP_OWNER")" "$(json_escape "$TAP_NAME")" \
    "$GORELEASER_CHECKED" "$VERIFIED_COUNT" "$FAILED_COUNT" "$duration_sec"

  exit "$exit_code"
}

main "$@"
# endregion
