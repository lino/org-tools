#!/usr/bin/env bash
# Copyright (C) 2026 org-tools contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Generates Bash, Zsh, and Fish shell completion files for the `org` CLI.
# Invoked automatically by GoReleaser `before.hooks` and release verification tests.
#
# Environment Variables:
#   ORGFMT_COMPLETIONS_DIR  Destination directory for completion files (default: completions).
#   ORGFMT_BIN              Path to a prebuilt `org` binary; builds via Cargo when unset.
#   ORGFMT_CARGO_BIN        Cargo executable used when building `org` (default: cargo).
#   ORGFMT_PACKAGE_SPEC     Cargo package name to compile when `ORGFMT_BIN` is unset (default: org-tools).
#   ORGFMT_BINARY_NAME      Compiled binary name inside Cargo target directory (default: org).

set -euo pipefail

# region Configuration & Defaults
DEFAULT_COMPLETIONS_DIR="completions"
DEFAULT_CARGO_BIN="cargo"
DEFAULT_PACKAGE_SPEC="org-tools"
DEFAULT_BINARY_NAME="org"

COMPLETIONS_DIR="${ORGFMT_COMPLETIONS_DIR:-$DEFAULT_COMPLETIONS_DIR}"
ORG_BIN="${ORGFMT_BIN:-}"
CARGO_BIN="${ORGFMT_CARGO_BIN:-$DEFAULT_CARGO_BIN}"
PACKAGE_SPEC="${ORGFMT_PACKAGE_SPEC:-$DEFAULT_PACKAGE_SPEC}"
BINARY_NAME="${ORGFMT_BINARY_NAME:-$DEFAULT_BINARY_NAME}"
# endregion

# region CLI Argument Parsing

# Prints CLI usage instructions to stdout.
print_usage() {
  cat <<'EOF'
Usage: build-completions.sh [OPTIONS]

Generate Bash, Zsh, and Fish completion scripts for the `org` CLI.

Options:
  --output-dir <DIR>  Destination directory (overrides ORGFMT_COMPLETIONS_DIR)
  --bin <PATH>        Path to prebuilt `org` binary (overrides ORGFMT_BIN)
  -h, --help          Show this help message and exit
EOF
}

# Parses command-line flags and updates script configuration globals.
parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --output-dir)
        [[ $# -ge 2 ]] || { echo "error: --output-dir requires a value" >&2; exit 2; }
        COMPLETIONS_DIR="$2"
        shift 2
        ;;
      --bin)
        [[ $# -ge 2 ]] || { echo "error: --bin requires a value" >&2; exit 2; }
        ORG_BIN="$2"
        shift 2
        ;;
      -h|--help)
        print_usage
        exit 0
        ;;
      *)
        echo "error: unknown argument: $1" >&2
        print_usage >&2
        exit 2
        ;;
    esac
  done
}
# endregion

# region Binary Resolution & Completion Generation

# Resolves the `org` binary path, building a release binary via Cargo when ORGFMT_BIN is unset.
resolve_org_binary() {
  if [[ -n "$ORG_BIN" ]]; then
    if [[ ! -x "$ORG_BIN" ]]; then
      echo "error: configured ORGFMT_BIN is not executable: $ORG_BIN" >&2
      exit 1
    fi
    return 0
  fi

  local target_dir="${CARGO_TARGET_DIR:-target}"
  "$CARGO_BIN" build --release --locked -p "$PACKAGE_SPEC"
  ORG_BIN="${target_dir}/release/${BINARY_NAME}"
  if [[ -f "${ORG_BIN}.exe" && ! -f "$ORG_BIN" ]]; then
    ORG_BIN="${ORG_BIN}.exe"
  fi
  if [[ ! -x "$ORG_BIN" ]]; then
    echo "error: built binary not found or not executable at $ORG_BIN" >&2
    exit 1
  fi
}

# Generates a single shell completion file and verifies that the output is non-empty.
generate_shell_completion() {
  local shell_name="$1"
  local output_filename="$2"
  local destination="${COMPLETIONS_DIR}/${output_filename}"

  "$ORG_BIN" --completions "$shell_name" > "$destination"
  if [[ ! -s "$destination" ]]; then
    echo "error: generated empty completion file for ${shell_name}: ${destination}" >&2
    exit 1
  fi
}
# endregion

# region Entrypoint

# Coordinates CLI flag parsing, binary resolution, and Bash/Zsh/Fish completion emission.
main() {
  parse_args "$@"
  resolve_org_binary
  mkdir -p "$COMPLETIONS_DIR"

  generate_shell_completion "bash" "${BINARY_NAME}.bash"
  generate_shell_completion "zsh" "_${BINARY_NAME}"
  generate_shell_completion "fish" "${BINARY_NAME}.fish"
}

main "$@"
# endregion
