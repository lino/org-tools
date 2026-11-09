<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Prometheus Metrics, Telemetry & Troubleshooting

## Overview

**What you will learn:**

- Export Prometheus textfile metrics (`orgfmt_release_*`) and structured JSON Lines (`JSONL`) release telemetry events.
- Inspect Command-Line Interface (`CLI`) mutation and Model Context Protocol (`MCP`) telemetry envelopes (`duration_ms`, `cache_hit`, `idempotent`).
- Correlate W3C trace IDs across pipelines and resolve operational alerts using the remediation table.

Structured telemetry turns silent packaging or mutation failures into actionable alerts before broken artifacts reach users. Exporting Prometheus gauges and counters alongside W3C trace-correlated JSONL events lets operators audit every release asset and file write. Deterministic exit codes and runbook remedies reduce incident triage time to seconds.

## Prerequisites

- A compiled `org` binary and `bash` with `jq` for inspecting JSON and JSONL payloads.
- Access to `scripts/verify-release-packages.sh` and a populated release artifact directory (`dist/`).
- Optional: A Prometheus Node Exporter textfile collector directory for scraping `.prom` files.

## Steps

1. Export Prometheus textfile metrics and structured JSONL telemetry events using `ORGFMT_RELEASE_METRICS_FILE` and `ORGFMT_RELEASE_EVENTS_FILE`.

```sh
ORGFMT_RELEASE_METRICS_FILE="dist/telemetry/release-metrics.prom" \
ORGFMT_RELEASE_EVENTS_FILE="dist/telemetry/release-events.jsonl" \
  bash scripts/verify-release-packages.sh dist
```

2. Inspect the generated Prometheus metrics file to confirm `orgfmt_release_verification_success` and `orgfmt_release_artifacts_verified_total`.

| Metric                                         | Type      | Labels           | Operational Target & Alert Threshold                     |
| ---------------------------------------------- | --------- | ---------------- | -------------------------------------------------------- |
| `orgfmt_release_verification_success`          | `gauge`   | *(none)*         | Equals `1` on pass; alert immediately when `== 0`        |
| `orgfmt_release_artifacts_verified_total`      | `counter` | `status`         | Expect `status="verified" 12` and `status="failed" 0`    |
| `orgfmt_release_artifact_size_bytes`           | `gauge`   | `target`, `kind` | Tracks byte size per `archive`, `deb`, `rpm`, and `cask` |
| `orgfmt_release_verification_duration_seconds` | `gauge`   | *(none)*         | Wall-clock verification latency in seconds               |

```sh
cat dist/telemetry/release-metrics.prom
```

3. Query the structured JSONL event log by verification status or W3C `trace_id` correlation field using `jq`.

```sh
jq 'select(.status != "ok")' dist/telemetry/release-events.jsonl
jq 'select(.event == "release.verification.completed")' dist/telemetry/release-events.jsonl
```

4. Inspect CLI mutation and MCP tool execution telemetry (`MutationTelemetry`) by passing `--format json`.

```sh
org update set-state README.org --line 1 "DONE" --dry-run --format json | jq '.telemetry'
```

The `org lsp` server flushes Prometheus metrics to `<workspace>/.cache/telemetry/orgfmt_lsp.prom`.
You can override the destination directory by setting `ORG_LSP_TELEMETRY_DIR`.

| Metric                                   | Type      | Labels             | Operational Target & Alert Threshold             |
| ---------------------------------------- | --------- | ------------------ | ------------------------------------------------ |
| `orgfmt_lsp_requests_total`              | `counter` | `method`, `status` | Alert if `status="error"` rate increases sharply |
| `orgfmt_lsp_request_duration_seconds`    | `gauge`   | `method`           | Target latency under 50 milliseconds for editing |
| `orgfmt_lsp_diagnostics_published_total` | `counter` | `severity`         | Tracks published diagnostics by severity level   |

Each request also records an `lsp.request` line in `events.jsonl` with W3C `trace_id` correlation.

5. Check process exit codes against the canonical `0..3` contract in CI workflows and shell scripts.

| Exit Code | Constant              | Operational Meaning                                                    |
| --------- | --------------------- | ---------------------------------------------------------------------- |
| `0`       | `EXIT_OK`             | Operation completed cleanly with zero errors                           |
| `1`       | `EXIT_ISSUES`         | No query matches found, or lint/format diagnostics emitted             |
| `2`       | `EXIT_ERROR`          | Operational failure (missing file, parse error, invalid CLI arguments) |
| `3`       | `EXIT_REQUIRES_EMACS` | Table formula (`TBLFM`) or feature requires Emacs Lisp or Emacs Calc   |

## Expected Outcome

The verification script writes Prometheus exposition metrics and W3C trace-correlated JSONL events with zero failed checks. Confirming `orgfmt_release_verification_success 1` and exit status `0` proves all release artifacts are intact.

```text
# HELP orgfmt_release_verification_duration_seconds Wall-clock duration of release package verification in seconds.
# TYPE orgfmt_release_verification_duration_seconds gauge
orgfmt_release_verification_duration_seconds 0.042180
# HELP orgfmt_release_artifacts_verified_total Total number of release checks grouped by verification outcome.
# TYPE orgfmt_release_artifacts_verified_total counter
orgfmt_release_artifacts_verified_total{status="verified"} 12
orgfmt_release_artifacts_verified_total{status="failed"} 0
# HELP orgfmt_release_verification_success Whether release package verification succeeded (1 = pass, 0 = fail).
# TYPE orgfmt_release_verification_success gauge
orgfmt_release_verification_success 1
$ echo $?
0
```

## Power-User & Scripting Options

Run `mise run metrics` locally for a dry-run check, or download `release-telemetry` from GitHub Actions via `gh run download`. You can also copy `release-metrics.prom` into your Prometheus Node Exporter `--collector.textfile.directory` for automated alerting. Consult the [Release & Homebrew Tap Runbook](release-and-tap.md) and the [Glossary and Abbreviations](../glossary.md) for pipeline details.

```sh
# Download release telemetry artifacts from the latest GitHub Actions release run
gh run download <run-id> --name release-telemetry --dir ./release-telemetry
jq 'select(.event == "release.check.failed")' ./release-telemetry/release-events.jsonl
```

??? info "Deep Dive: JSONL Event Schema & WorkspaceCache Telemetry"
Every line in `ORGFMT_RELEASE_EVENTS_FILE` is a JSON object with `timestamp`, `event`, `package`, `tap`, and W3C `trace_id`. Each event also records `status` (`ok` or `error`), `verified_count`, `failed_count`, `duration_seconds`, and `message`. Supported event names include `release.verification.started`, `release.goreleaser_check.passed`, `release.artifact.verified`, `release.check.failed`, and `release.verification.completed`.

```
Every mutating CLI subcommand and MCP tool call attaches a `MutationTelemetry` (`telemetry`) object to its JSON output. This envelope reports `duration_ms`, `files_scanned`, `files_mutated`, `cache_hit` (for `.cache/org-cache.db` SQLite Write-Ahead Logging (`WAL`) `WorkspaceCache`), and `idempotent`.
```

## Troubleshooting

| Symptom                                                        | Cause                                                           | Fix                                                                                 |
| -------------------------------------------------------------- | --------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `orgfmt_release_verification_success == 0`                     | Missing archive, `.deb`, `.rpm`, Cask, or bad SHA-256 digest    | Inspect `release.check.failed` in `release-events.jsonl` and rebuild with `--clean` |
| `orgfmt_release_artifacts_verified_total{status="failed"} > 0` | Target builder ID mismatch or truncated package in `dist/`      | Verify `.goreleaser.yaml` builder IDs (`org-darwin`, `org-linux`, `org-windows`)    |
| `org` exits with code `3` (`EXIT_REQUIRES_EMACS`)              | Spreadsheet `#+TBLFM:` uses unsupported Emacs Lisp or Calc form | Evaluate that formula in batch Emacs or simplify it to standard arithmetic          |
| `cache_hit: false` on repeated CLI or MCP queries              | `--no-cache` is set or `.cache/org-cache.db` is not writable    | Ensure write permissions on `.cache/` and run `org roam sync` to warm the cache     |
| LSP diagnostics or completion lag behind edits                 | High request latency or slow filesystem                         | Check `orgfmt_lsp_request_duration_seconds` and ensure cache is warmed              |
| SQLite lock or corrupt `.cache/org-cache.db` error             | Stale `.cache/org-cache.db-wal` after an abrupt host crash      | Remove `.cache/org-cache.db*` and let `WorkspaceCache` rebuild the index cleanly    |
