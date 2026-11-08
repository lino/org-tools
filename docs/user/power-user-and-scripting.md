<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Power-User & Scripting Guide

## Overview

**What you will learn:**

- How to chain `org` subcommands over standard input (`stdin`) and standard output (`stdout`) using `OrgLocator` addresses.
- How to branch reliably in shell scripts using the canonical `0..3` exit-code contract.
- How to override workspace paths, configuration files, colors, and cache locations with environment variables.

Interactive terminal commands are only half the story when managing thousands of tasks and notes.
`org` follows strict Unix stream discipline (`stdout` for data, `stderr` for diagnostics) so scripts never parse human log noise.
You can compose multi-step task migrations and CI gates using standard POSIX pipes and `jq`.

## Prerequisites

- `org` installed on your workstation or CI host (see [Five-Minute Quickstart](quickstart.md)).
- A POSIX shell (`sh`, `bash`, `zsh`) and `jq` for inspecting JSON output envelopes.
- Familiarity with [Querying & Agenda Views](querying-and-agenda.md) and [Updating, Clocking & Exporting](updating-clocking-and-exporting.md).

## Steps

1. Emit one `OrgLocator` per line with `--format locator` and pipe the stream directly into `org update` subcommands.
   ```sh
   org query search 'todo:TODO tags:stale' --format locator ~/org/ \
     | org update set-state DONE

   org query next --context @errands --format locator ~/org/ \
     | org update set-scheduled today
   ```
1. Pipe raw Org content through `org fmt` or `org update set-state --query` to transform streams without modifying disk files.
   ```sh
   cat draft.org | org fmt format > clean.org
   cat inbox.org | org update set-state DONE --query 'tags:shipped' > inbox-updated.org
   ```
1. Override configuration, cache, namespace, and terminal color settings via environment variables in headless scripts.
   ```sh
   export NO_COLOR=1
   export ORG_TOOLS_CONFIG="$PWD/.org-tools.toml"
   export ORG_TOOLS_WORKSPACE="$PWD"
   export ORG_TOOLS_CACHE_PATH="$PWD/.cache/org-cache.db"
   export ORG_ID_NAMESPACE="6ba7b810-9dad-11d1-80b4-00c04fd430c8"
   export ORG_TAG_COLUMN=77
   ```
1. Branch on the deterministic `0..3` exit-code contract in shell automation and Continuous Integration (CI) jobs.
   ```sh
   org update calc budget.org
   status=$?
   if [ "$status" -eq 3 ]; then
     echo "Formula requires Emacs Calc; falling back to batch Emacs."
   elif [ "$status" -ne 0 ]; then
     exit "$status"
   fi
   ```
1. Inspect execution telemetry (`duration_us`, `files_scanned`, `cache_hits`, `cache_misses`) from `--format json` mutation envelopes.
   ```sh
   org update add-cookie --recursive --format json ~/org/ | jq '.telemetry'
   ```

## Expected Outcome

Piped commands process every incoming `OrgLocator` atomically and emit a consolidated summary or JSON envelope on `stdout`.
Scripts can inspect `$?` against the canonical exit-code table to distinguish clean runs (`0`), diagnostic/empty matches (`1`), errors (`2`), and Emacs fallbacks (`3`):

```text
$ org query search 'todo:TODO tags:launch' --format locator tasks.org | org update set-state NEXT --dry-run
Would update 2 entries across 1 file (dry run)
$ echo $?
0
```

## Power-User & Scripting Options

Combine `org query` JSON output with `jq` and `xargs` for conditional batch updates or cross-vault reporting.
See [JSON Schemas & Contracts](schemas.md), [CI Pipelines & Shell Automation](../cookbook/automation-and-ci.md), and the [Glossary](../glossary.md) for schema details:

```sh
# Extract locators for high-priority tasks missing an EFFORT property and assign 0:30
org query search 'todo:TODO priority:A not property:EFFORT' --format locator ~/org/ \
  | xargs -I{} org update set-property "{}" EFFORT 0:30
```

??? info "Deep Dive: Environment Variable & Exit-Code Reference"

| Variable or Code       | Default / Value | Behavior                                                                                  |
| ---------------------- | --------------- | ----------------------------------------------------------------------------------------- |
| `ORG_TOOLS_CONFIG`     | *(ancestor)*    | Explicit path to `.org-tools.toml` configuration file.                                    |
| `ORG_TOOLS_WORKSPACE`  | `cwd`           | Default workspace root for `id:` resolution and `org mcp` confinement.                    |
| `ORG_TOOLS_CACHE_PATH` | `.cache/...`    | Override path for the SQLite Write-Ahead Logging (`WAL`) `WorkspaceCache` database.       |
| `ORG_ID_NAMESPACE`     | DNS UUID        | Namespace UUID used by `org update add-id --deterministic` (`UUIDv5`).                    |
| `ORG_TAG_COLUMN`       | `77`            | Target right-margin column for `F007` heading tag alignment.                              |
| `NO_COLOR` / `TERM`    | unset / `xterm` | Disables ANSI color escapes when `NO_COLOR` is non-empty or `TERM=dumb`.                  |
| Exit codes `0..3`      | `0, 1, 2, 3`    | `0` = OK, `1` = issues/no match, `2` = operational error, `3` = requires Emacs Lisp/Calc. |

## Troubleshooting

| Symptom                                                  | Cause                                                          | Fix                                                                  |
| -------------------------------------------------------- | -------------------------------------------------------------- | -------------------------------------------------------------------- |
| `set -e` script aborts when `org query search` finds 0   | `org query` returns exit code `1` (`EXIT_ISSUES`) on zero hits | Handle exit code `1` explicitly (`org query ... \|\| [ $? -eq 1 ]`)  |
| ANSI escape codes appear in redirected log files         | Forced terminal settings in CI environment                     | Export `NO_COLOR=1` or `TERM=dumb` before invoking `org`             |
| Piped `org update set-state` prints Org text to `stdout` | Input stream contained raw Org headings instead of locators    | Pass `--format locator` to the upstream `org query` command          |
| Slow repeated queries in a tight shell loop              | `--no-cache` was set or `.cache/` directory is not writable    | Remove `--no-cache` and run `org cache rebuild` once before the loop |
