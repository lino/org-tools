<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Prometheus Metrics, Telemetry & Troubleshooting

## Overview

**What you will learn:**

- Export Prometheus textfile metrics and structured release telemetry events.
- Inspect mutation telemetry envelopes covering BLAKE3 cache synchronization.
- Verify binary capabilities using `org --version --format json`.
- Correlate trace IDs across pipelines and resolve operational alerts.

Structured telemetry turns silent packaging or mutation failures into actionable alerts.
Exporting Prometheus gauges and counters lets operators audit every release asset.
Deterministic exit codes and runbook remedies reduce incident triage time to seconds.
Write-through BLAKE3 cache synchronization ensures fast subsequent queries.

## Prerequisites

- A compiled `org` binary and `bash` with `jq` for parsing JSON payloads.
- Access to release verification scripts and a populated `dist/` directory.
- Optional: A Prometheus Node Exporter textfile collector directory.

## Steps

1. Export Prometheus textfile metrics and structured JSONL telemetry events.

```sh
ORGFMT_RELEASE_METRICS_FILE="dist/telemetry/release-metrics.prom" \
ORGFMT_RELEASE_EVENTS_FILE="dist/telemetry/release-events.jsonl" \
  bash scripts/verify-release-packages.sh dist
```

2. Inspect the generated Prometheus metrics file to confirm verification success.

| Metric                                         | Type      | Labels           | Operational Target & Alert Threshold                     |
| ---------------------------------------------- | --------- | ---------------- | -------------------------------------------------------- |
| `orgfmt_release_verification_success`          | `gauge`   | *(none)*         | Equals `1` on pass; alert immediately when `== 0`        |
| `orgfmt_release_artifacts_verified_total`      | `counter` | `status`         | Expect `status="verified" 12` and `status="failed" 0`    |
| `orgfmt_release_artifact_size_bytes`           | `gauge`   | `target`, `kind` | Tracks byte size per `archive`, `deb`, `rpm`, and `cask` |
| `orgfmt_release_verification_duration_seconds` | `gauge`   | *(none)*         | Wall-clock verification latency in seconds               |

```sh
cat dist/telemetry/release-metrics.prom
```

3. Query the structured event log by verification status or correlation field.

```sh
jq 'select(.status != "ok")' dist/telemetry/release-events.jsonl
jq 'select(.event == "release.verification.completed")' dist/telemetry/release-events.jsonl
```

4. Inspect mutation telemetry and inline BLAKE3 cache synchronization status.

```sh
org update add-id --file --format json note.org | jq '{cache_synced, records, telemetry}'
```

The Language Server Protocol server flushes metrics to `<workspace>/.cache/telemetry/orgfmt_lsp.prom`.
You can override the destination directory by setting `ORG_LSP_TELEMETRY_DIR`.

| Metric                                   | Type      | Labels             | Operational Target & Alert Threshold             |
| ---------------------------------------- | --------- | ------------------ | ------------------------------------------------ |
| `orgfmt_lsp_requests_total`              | `counter` | `method`, `status` | Alert if `status="error"` rate increases sharply |
| `orgfmt_lsp_request_duration_seconds`    | `gauge`   | `method`           | Target latency under 50 milliseconds for editing |
| `orgfmt_lsp_diagnostics_published_total` | `counter` | `severity`         | Tracks published diagnostics by severity level   |

5. Inspect installed binary capability flags and compile-time features.

```sh
org --version --format json | jq '.capabilities'
```

6. Check process exit codes against the canonical `0..3` contract.

| Exit Code | Constant              | Operational Meaning                                                    |
| --------- | --------------------- | ---------------------------------------------------------------------- |
| `0`       | `EXIT_OK`             | Operation completed cleanly with zero errors                           |
| `1`       | `EXIT_ISSUES`         | No query matches found, or lint/format diagnostics emitted             |
| `2`       | `EXIT_ERROR`          | Operational failure (missing file, parse error, invalid CLI arguments) |
| `3`       | `EXIT_REQUIRES_EMACS` | Table formula (`TBLFM`) or feature requires Emacs Lisp or Emacs Calc   |

## Expected Outcome

The verification script writes Prometheus exposition metrics and structured events.
Confirming `orgfmt_release_verification_success 1` and exit code `0` validates all packages.
Mutations report `cache_synced: true` upon committing BLAKE3 updates to disk:

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

Run `mise run metrics` locally for a dry-run check.
Download release telemetry from GitHub Actions via `gh run download`.
Full-text search queries execute through SQLite FTS5 BM25 scoring.
Scoped landscape queries prune entries before ranking to maximize query throughput:

```sh
# Download release telemetry artifacts from the latest GitHub Actions release run
gh run download <run-id> --name release-telemetry --dir ./release-telemetry
jq 'select(.event == "release.check.failed")' ./release-telemetry/release-events.jsonl
```

??? info "Deep Dive: Structured Event Schema & WorkspaceCache Telemetry"
Every line in `ORGFMT_RELEASE_EVENTS_FILE` is a JSON object with `timestamp`, `event`, and `trace_id`.
Each event records `status`, `verified_count`, `failed_count`, `duration_seconds`, and `message`.
Every mutating CLI subcommand attaches a `telemetry` object to its JSON output.
This envelope reports `duration_us`, `files_scanned`, `cache_hits`, and `cache_misses`.

## Troubleshooting

| Symptom                                                        | Cause                                                           | Fix                                                                                 |
| -------------------------------------------------------------- | --------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `orgfmt_release_verification_success == 0`                     | Missing archive, `.deb`, `.rpm`, Cask, or bad SHA-256 digest    | Inspect `release.check.failed` in `release-events.jsonl` and rebuild with `--clean` |
| `orgfmt_release_artifacts_verified_total{status="failed"} > 0` | Target builder ID mismatch or truncated package in `dist/`      | Verify `.goreleaser.yaml` builder IDs (`org-darwin`, `org-linux`, `org-windows`)    |
| `org` exits with code `3` (`EXIT_REQUIRES_EMACS`)              | Spreadsheet `#+TBLFM:` uses unsupported Emacs Lisp or Calc form | Evaluate that formula in batch Emacs or simplify it to standard arithmetic          |
| `cache_misses` increases on repeated CLI queries               | `--no-cache` is set or `.cache/org-cache.db` is not writable    | Ensure write permissions on `.cache/` and run `org cache rebuild` once              |
| External file edits not visible in queries                     | External tool bypassed BLAKE3 write-through sync                | Run `org cache rebuild` to refresh the index                                        |
| Stale SQLite lock on `.cache/org-cache.db`                     | Unclean process termination left WAL lock files                 | Remove `.cache/org-cache.db*` and let `org` re-index the workspace                  |
