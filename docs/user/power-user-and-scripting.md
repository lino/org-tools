<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Power-User & Scripting Guide

## Overview

**What you will learn:**

- How to chain `org` subcommands over standard streams using `OrgLocator` addresses.
- How to branch reliably in shell scripts using the canonical `0..3` exit-code contract.
- How to filter entries by project landscape and ranked full-text queries (`--fts`).
- How to inspect binary capability envelopes via `org --version --format json`.
- How to enable dynamic shell completions powered by `.cache/org-cache.db`.

Interactive terminal commands are only half the story when managing thousands of notes.
The CLI follows strict Unix stream discipline with standard output for structured data.
Diagnostics stream to standard error so scripts never parse log noise.
You can compose multi-step task migrations using standard POSIX pipes and `jq`.

## Prerequisites

- `org` installed on your workstation or Continuous Integration runner.
- A POSIX shell (`sh`, `bash`, `zsh`) and `jq` for parsing JSON payloads.
- Familiarity with querying, agenda projection, and entry mutation workflows.

## Steps

1. Emit one `OrgLocator` per line with `--format locator` and pipe into mutating subcommands.
   ```sh
   org query search 'todo:TODO tags:stale' --format locator ~/org/ \
     | org update set-state DONE

   org query next --context @errands --format locator ~/org/ \
     | org update set-scheduled today
   ```
1. Assign deterministic identifiers to file-level drawers using `org update add-id --file`.
   ```sh
   org update add-id --file --write-seed note.org
   org update add-id --headings --max-level 2 tasks.org
   ```
1. Filter notes across project landscapes with ranked full-text search (`--fts`) and scope flags.
   ```sh
   org query search --fts "idempotency" --for-project payments-api --format json
   org query search --for-landscape billing-platform --strict-scope --format locator
   ```
1. Enforce cross-file link integrity across federated vaults with `--id-scope vault`.
   ```sh
   org fmt check --id-scope vault notes/
   ```
1. Inspect binary version capabilities programmatically using `org --version --format json`.
   ```sh
   org --version --format json | jq '.capabilities'
   ```
1. Branch on the deterministic `0..3` exit-code contract in automation pipelines.
   ```sh
   org update calc budget.org
   status=$?
   if [ "$status" -eq 3 ]; then
     echo "Formula requires Emacs Calc; falling back to batch Emacs."
   elif [ "$status" -ne 0 ]; then
     exit "$status"
   fi
   ```
1. Enable workspace-aware dynamic tab completions in `zsh`, `fish`, or `bash` to complete `OrgLocator` targets, `roam-node` titles, `todo-state` keywords, and `date-spec` shifts.
   ```sh
   # Zsh (~/.zshrc)
   eval "$(org --completions zsh)"

   # Fish (~/.config/fish/config.fish)
   org --completions fish | source

   # Bash (~/.bashrc)
   eval "$(org --completions bash)"
   ```

## Expected Outcome

Piped commands process every incoming locator atomically and emit a consolidated summary on standard output.
Every mutation performs incremental BLAKE3 write-through sync to `.cache/org-cache.db`.
Scripts inspect exit codes to distinguish clean runs (`0`), empty matches (`1`), and operational errors (`2`):

```text
$ org query search --fts "idempotency" --for-project payments-api --format json
[
  {
    "file": "billing.org",
    "line": 12,
    "title": "Payment Idempotency Keys",
    "scope": "landscape:billing-platform"
  }
]
$ echo $?
0
```

## Power-User & Scripting Options

Combine query JSON output with `jq` and `xargs` for conditional updates.
Pass `--strict-scope` to exclude implicit global entries during landscape evaluations.
Every mutating subcommand supports `--dry-run` to inspect proposed edits before writing files:

```sh
# Assign effort estimates to high-priority tasks missing an EFFORT property
org query search 'todo:TODO priority:A not property:EFFORT' --format locator ~/org/ \
  | xargs -I{} org update set-property "{}" EFFORT 0:30
```

??? info "Deep Dive: Environment Variable & Exit-Code Reference"

    | Variable or Code       | Default / Value | Behavior                                                                                  |
    | ---------------------- | --------------- | ----------------------------------------------------------------------------------------- |
    | `ORG_TOOLS_CONFIG`     | *(ancestor)*    | Explicit path to `.org-tools.toml` configuration file.                                    |
    | `ORG_TOOLS_WORKSPACE`  | `cwd`           | Default workspace root for `id:` resolution and MCP confinement.                          |
    | `ORG_TOOLS_CACHE_PATH` | `.cache/...`    | Override path for the SQLite Write-Ahead Logging `WorkspaceCache` database.               |
    | `ORG_ID_NAMESPACE`     | DNS UUID        | Namespace UUID used by `org update add-id --deterministic` (`UUIDv5`).                    |
    | `ORG_TAG_COLUMN`       | `77`            | Target right-margin column for `F007` heading tag alignment.                              |
    | `NO_COLOR` / `TERM`    | unset / `xterm` | Disables ANSI color escapes when `NO_COLOR` is non-empty or `TERM=dumb`.                  |
    | Exit codes `0..3`      | `0, 1, 2, 3`    | `0` = OK, `1` = issues/no match, `2` = operational error, `3` = requires Emacs Lisp/Calc. |

## Troubleshooting

| Symptom                                                | Cause                                                          | Fix                                                                  |
| ------------------------------------------------------ | -------------------------------------------------------------- | -------------------------------------------------------------------- |
| `set -e` script aborts when `org query search` finds 0 | `org query` returns exit code `1` (`EXIT_ISSUES`) on zero hits | Handle exit code `1` explicitly (`org query ... \|\| [ $? -eq 1 ]`)  |
| Global notes appear in landscape query results         | Unscoped notes default to global validity                      | Pass `--strict-scope` to restrict results strictly to the target     |
| ANSI escape codes appear in redirected log files       | Forced terminal settings in CI environment                     | Export `NO_COLOR=1` or `TERM=dumb` before invoking `org`             |
| Slow repeated queries in a tight shell loop            | `--no-cache` was set or `.cache/` directory is not writable    | Remove `--no-cache` and run `org cache rebuild` once before the loop |
