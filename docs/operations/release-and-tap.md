<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# GoReleaser v2 Release & Homebrew Tap Runbook

## Overview

**What you will learn:**

- Execute pre-release verification gates, changelog previews, and dry-run manifest checks.
- Package multi-architecture archives, `.deb`/`.rpm` packages, Homebrew Casks, completions, and Unix `roff` (`ROFF`) manual pages (`man/org.1`, `man/org-tools.toml.5`) via GoReleaser v2.
- Bootstrap `lino/homebrew-tap` and rotate least-privilege GitHub tokens without downtime.

Automating multi-platform builds and artifact checks prevents manual packaging errors during software releases. GoReleaser v2 compiles six target architectures, bundles shell completions and Unix manual pages, and verifies every SHA-256 digest before publication. Decoupled Homebrew tap and `crates.io` jobs ensure missing optional tokens never block binary distributions.

## Prerequisites

- Maintainer write access to the `lino/org-tools` and `lino/homebrew-tap` GitHub repositories.
- Local `bash`, `git-cliff`, Rust 1.83+, and GoReleaser OSS v2 (`~> v2.18.0`) for dry-run verification.
- A Fine-Grained Personal Access Token (`PAT`) in `HOMEBREW_TAP_GITHUB_TOKEN` with `Contents: Read and write` permission on `lino/homebrew-tap`.

## Steps

1. Run the full workspace verification gate and smoke-test the edge-case fixtures before cutting a release.

```sh
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- fmt check README.org docs/
cargo run -- fmt check tests/fixtures/edge_cases/
```

2. Preview the conventional commit changelog via `git-cliff` (`cliff.toml`) and validate manifest parity in dry-run mode.

```sh
git cliff --config cliff.toml --unreleased
bash scripts/release.sh --dry-run X.Y.Z
```

3. Configure or rotate the scoped `HOMEBREW_TAP_GITHUB_TOKEN` and optional `CARGO_REGISTRY_TOKEN` secrets on `lino/org-tools`.

```sh
gh secret set HOMEBREW_TAP_GITHUB_TOKEN --repo lino/org-tools
gh secret set CARGO_REGISTRY_TOKEN --repo lino/org-tools
```

4. Trigger a remote `workflow_dispatch` snapshot build on `main` to test all six cross-compilation targets before tagging.

```sh
gh workflow run release.yml --repo lino/org-tools -f snapshot_note="pre-release verification"
gh run list --workflow=release.yml --repo lino/org-tools --limit 1
```

5. Cut the release commit, create tag `vX.Y.Z`, and push to `origin` using `scripts/release.sh`.

```sh
bash scripts/release.sh X.Y.Z
```

6. Verify all generated archives, `.deb`/`.rpm` packages, Homebrew Cask hooks, and `checksums.txt` digests via `scripts/verify-release-packages.sh`.

```sh
bash scripts/verify-release-packages.sh dist
```

## Expected Outcome

The verification script validates all six target archives, four Linux `.deb`/`.rpm` packages, `Casks/org-tools.rb`, and `checksums.txt`. It prints a single-line JSON verification report to standard output and exits with code `0`.

```text
$ bash scripts/verify-release-packages.sh dist
{"status":"ok","package":"org-tools","dist_dir":"dist","tap":"lino/homebrew-tap","goreleaser_checked":true,"verified_artifacts":12,"failed_checks":0,"duration_seconds":0.042180}
$ echo $?
0
```

## Power-User & Scripting Options

Pass `--yes` to `scripts/release.sh` to skip interactive confirmation prompts in automated release pipelines. Override `verify-release-packages.sh` paths via `--dist-dir`, `--config`, `--metrics-file`, and `--events-file` (or `ORGFMT_DIST_DIR`, `ORGFMT_GORELEASER_CONFIG`, `ORGFMT_RELEASE_METRICS_FILE`, and `ORGFMT_RELEASE_EVENTS_FILE`). See [Prometheus Metrics & Troubleshooting](metrics-and-troubleshooting.md) and the [Glossary and Abbreviations](../glossary.md) for telemetry schemas.

```sh
# Run non-interactive release tagging and custom artifact verification
bash scripts/release.sh --yes 0.2.0
bash scripts/verify-release-packages.sh \
  --dist-dir dist \
  --config .goreleaser.yaml \
  --metrics-file dist/telemetry/release-metrics.prom \
  --events-file dist/telemetry/release-events.jsonl
```

??? info "Deep Dive: Pipeline Architecture, Manpage Packaging & Tap Bootstrap"

    Pushing a `v*` tag triggers `.github/workflows/release.yml`, which executes `validate` first and then runs `release-packages`, `publish-crates`, and `publish-container` (`ghcr.io/lino/org-tools`) in parallel.

    ```mermaid
    flowchart TD
        Trigger["Trigger: git tag v* or workflow_dispatch"] --> Validate["Job: validate (fmt, clippy, test, org fmt check)"]
        Validate --> ReleasePkg["Job: release-packages (6 Rust targets + GoReleaser v2)"]
        Validate --> PublishCrates["Job: publish-crates (optional CARGO_REGISTRY_TOKEN gate)"]
        Validate --> PublishContainer["Job: publish-container (GHCR linux/amd64 + linux/arm64)"]
        ReleasePkg --> Verify["scripts/verify-release-packages.sh (SHA-256, .deb/.rpm, Cask, manpages)"]
        Verify --> Assets["GitHub Release + lino/homebrew-tap + release-telemetry"]
    ```

    `.goreleaser.yaml` packages `man/org.1` and `man/org-tools.toml.5` across all distribution channels:

    - **Release Archives (`archives[].files`)**: Bundles `man/org.1` and `man/org-tools.toml.5` alongside `completions/`.
    - **Linux Packages (`nfpms[].contents`)**: Installs `/usr/share/man/man1/org.1` and `/usr/share/man/man5/org-tools.toml.5`.
    - **Homebrew Cask (`homebrew_casks[].manpages`)**: Installs `man/org.1` (`man 1 org`) and `man/org-tools.toml.5` (`man 5 org-tools.toml`).

    If `lino/homebrew-tap` does not exist yet, bootstrap it once with GitHub CLI (`CLI`):

    ```sh
    gh repo create lino/homebrew-tap --public \
      --description "Homebrew tap for org-tools and companion CLI utilities" --clone
    cd homebrew-tap && mkdir -p Casks
    printf "# lino/homebrew-tap\n\nInstall \`org-tools\`:\n\n\`\`\`sh\nbrew install --cask lino/tap/org-tools\n\`\`\`\n" > README.md
    git add README.md && git commit -m "chore: initialize homebrew-tap repository"
    git branch -M main && git push -u origin main
    ```

## Troubleshooting

| Symptom                                                  | Cause                                                         | Fix                                                                                |
| -------------------------------------------------------- | ------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `Cargo.toml vs crates/org/Cargo.toml version mismatch`   | Workspace root and binary crate versions are out of sync      | Run `bash scripts/release.sh X.Y.Z` to synchronize both manifests automatically    |
| `401 Bad credentials` or `403` pushing to `homebrew-tap` | `HOMEBREW_TAP_GITHUB_TOKEN` expired or lacks `Contents` write | Mint a fine-grained PAT for `lino/homebrew-tap` and run `gh secret set`            |
| `missing com.apple.quarantine postflight hook`           | `.goreleaser.yaml` Cask post-install hook was modified        | Restore the `xattr -dr com.apple.quarantine` block in `.goreleaser.yaml`           |
| `CARGO_REGISTRY_TOKEN is not configured; skipping`       | Optional `crates.io` secret is unset in GitHub Actions        | Add `CARGO_REGISTRY_TOKEN` when ready to publish crates, or ignore the safe notice |
