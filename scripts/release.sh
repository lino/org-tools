#!/usr/bin/env bash
# Copyright (C) 2026 org-tools contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Bump version, commit, tag, and push to trigger release.
# Usage: ./scripts/release.sh [VERSION]
#   No argument: bumps patch version (0.1.2 → 0.1.3)
#   With argument: bumps to that version (e.g. 0.2.0)
set -euo pipefail

CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
echo "Current version: $CURRENT"

if [ -z "${1:-}" ]; then
  IFS='.' read -r major minor patch <<< "$CURRENT"
  NEXT="$major.$minor.$((patch + 1))"
else
  NEXT="$1"
fi

echo "New version: $NEXT"
read -rp "Proceed? [y/N] " confirm
[[ "$confirm" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }

# Verify clean working tree
if [ -n "$(git status --porcelain)" ]; then
  echo "Error: working tree is not clean. Commit or stash changes first."
  exit 1
fi

# Bump workspace version
sed -i.bak "s/^version = \"$CURRENT\"/version = \"$NEXT\"/" Cargo.toml
rm -f Cargo.toml.bak

# Bump org-tools-core dependency version in org crate
sed -i.bak "s/org-tools-core = { path = \"..\/org-tools-core\", version = \"$CURRENT\"/org-tools-core = { path = \"..\/org-tools-core\", version = \"$NEXT\"/" crates/org/Cargo.toml
rm -f crates/org/Cargo.toml.bak

# Update lockfile
cargo check

# Verify
cargo test
cargo clippy -- -D warnings

# Commit, tag, push
git add Cargo.toml Cargo.lock crates/org/Cargo.toml
git commit -m "chore: bump version to $NEXT"
git tag "v$NEXT"
git push origin main
git push origin "v$NEXT"

echo ""
echo "Released v$NEXT — GitHub Actions will publish to crates.io."
