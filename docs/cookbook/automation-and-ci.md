<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# CI Pipelines & Shell Automation Recipe

## Overview

**What you will learn:**

- How to enforce `org fmt format --check` and `org fmt check` in GitHub Actions and `.pre-commit-config.yaml`.
- How to publish Zensical documentation reports (`org query agenda --format md`, `org export md`) in Continuous Integration (CI).
- How to write fail-fast POSIX shell scripts that validate JSON outputs against `org schema`.

Shared team repositories accumulate broken `[[id:...]]` links, unaligned tables, and malformed drawers without automated checks.
Because `org` compiles to a single static binary with zero Emacs dependency, it runs in seconds on any CI runner.
You can gate pull requests on lint-clean `.org` files and publish live Markdown agendas automatically.

## Prerequisites

- `org` installed locally or via the `ghcr.io/lino/org-tools:latest` container in CI.
- A Git repository containing `.org` files and `.org-tools.toml`.
- Optional: `pre-commit` and `jq` installed for local hook and JSON validation workflows.

## Steps

1. Add `org fmt format --check` and `org fmt check` hooks to `.pre-commit-config.yaml` so local commits stay lint-clean.
   ```yaml
   # Copyright (C) 2026 org-tools contributors
   # SPDX-License-Identifier: GPL-3.0-or-later
   repos:
     - repo: local
       hooks:
         - id: org-fmt-check
           name: org fmt check
           entry: org fmt check
           language: system
           files: \.org$
   ```
1. Add a GitHub Actions job in `.github/workflows/org-check.yml` using the prebuilt container or release binary.
   ```yaml
   # Copyright (C) 2026 org-tools contributors
   # SPDX-License-Identifier: GPL-3.0-or-later
   name: Org Lint & Format Check
   on: [push, pull_request]
   jobs:
     org-check:
       runs-on: ubuntu-latest
       container:
         image: ghcr.io/lino/org-tools:latest
       steps:
         - uses: actions/checkout@v4
         - name: Check formatting
           run: org fmt format --check .
         - name: Run lint rules
           run: org fmt check .
   ```
1. Export Org specifications and composite agendas to Zensical Markdown (`org export md`, `org query agenda --format md`) during CI builds.
   ```sh
   org export md --workspace . --expand-includes --output docs/exported/ specs/
   org query agenda --days 14 --composite --format md -o docs/exported/agenda.md .
   uvx zensical build --strict
   ```
1. Write a POSIX shell script that queries JSON envelopes, checks exit codes (`0..3`), and validates schemas with `org schema`.
   ```sh
   #!/usr/bin/env sh
   set -eu

   org schema diagnostic --output /tmp/org-fmt.schema.json
   if ! org fmt check --format json . > /tmp/diagnostics.json; then
     jq '.[] | "\(.file):\(.line): [\(.rule_id)] \(.message)"' /tmp/diagnostics.json >&2
     exit 1
   fi
   ```

## Expected Outcome

Every pull request that introduces an unformatted table, broken `[[id:...]]` link, or unclosed block fails CI with exit code `1`.
Clean commits pass in milliseconds with exit code `0` and publish fresh Zensical Markdown pages:

```text
$ org fmt format --check . && org fmt check .
$ echo $?
0
```

## Power-User & Scripting Options

Combine `org update add-id --deterministic` with `org fmt check` in CI to verify that every heading carries a reproducible UUIDv5.
See [Power-User & Scripting](../user/power-user-and-scripting.md), [JSON Schemas & Contracts](../user/schemas.md), and the [Glossary](../glossary.md):

```sh
# Verify that deterministic UUIDv5 generation produces zero uncommitted diffs
org update add-id --deterministic .
git diff --exit-code
```

??? info "Deep Dive: Zero-Emacs Container Caching in CI"
When running `org` across thousands of `.org` files in GitHub Actions, cache the `.cache/org-cache.db` directory with `actions/cache@v4`.
`WorkspaceCache` verifies `(mtime_ns, byte_len, sha256)` on startup and only reparses files changed in the latest commit.

## Troubleshooting

| Symptom                                                  | Cause                                                           | Fix                                                                               |
| -------------------------------------------------------- | --------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `org fmt format --check .` exits with code `1` in CI     | Committed `.org` file has unaligned tables, drawers, or tags    | Run `org fmt format .` locally and commit the formatted changes                   |
| Container job fails with `dubious ownership` or perm err | Container user ID differs from the GitHub Actions workspace UID | Pass `-u "$(id -u):$(id -g)"` when invoking `docker run`                          |
| `uvx zensical build --strict` fails on exported Markdown | Broken relative link or missing target in `zensical.toml` `nav` | Pass `--workspace .` to `org export md` so `[[id:...]]` links resolve cleanly     |
| Pre-commit hook passes despite lint warnings             | Hook command appended `\|\| true` and swallowed exit code `1`   | Remove `\|\| true` from `.pre-commit-config.yaml` so non-zero codes fail the hook |
