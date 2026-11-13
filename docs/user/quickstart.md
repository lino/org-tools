<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Five-Minute Quickstart

## Overview

**What you will learn:**

- How to install the `org` Command-Line Interface (CLI) and verify its version and manual pages.
- How to format and lint your first `.org` file without modifying literal source blocks.
- How to run your first Getting Things Done (GTD) task query, agenda projection, and fleeting note capture.

Getting started with a new command-line tool should take minutes rather than hours of configuration.
`org` ships with sensible defaults that match Emacs Org-mode right out of the box.
You can check files, query tasks, and capture notes immediately from any terminal.

## Prerequisites

- A macOS, Linux, or Windows terminal (`bash`, `zsh`, `fish`, or PowerShell).
- Homebrew, Cargo, Docker, or a prebuilt release archive (see [Multi-Platform Installation](../operations/installation.md)).
- A directory containing at least one `.org` file (or create the sample file in Step 2).

## Steps

1. Install `org` using Homebrew on macOS/Linux or Cargo from `crates.io`, then verify the installed version.
   ```sh
   brew install --cask lino/tap/org-tools
   # Or install with Cargo:
   cargo install org-tools

   org --version
   ```
1. Create a sample `tasks.org` file containing a project heading, a property drawer, and two tasks.
   ```sh
   cat << 'EOF' > tasks.org
   #+TITLE: Quickstart Tasks

   * Project Launch
   :PROPERTIES:
   :CATEGORY: launch
   :END:
   ** TODO Draft release announcement :@writing:work:
   SCHEDULED: <2026-04-06 Mon>
   ** TODO Verify package checksums :@ops:work:
   DEADLINE: <2026-04-08 Wed>
   EOF
   ```
1. Run `org fmt format` and `org fmt check` on `tasks.org` to align drawers and tags and verify zero lint issues.
   ```sh
   org fmt format tasks.org
   org fmt check tasks.org
   ```
1. Run `org query search` and `org query agenda` to list active `TODO` items and project your weekly timeline.
   ```sh
   org query search 'todo:TODO tags:work' tasks.org
   org query agenda --from 2026-04-06 --days 7 tasks.org
   ```
1. Capture a quick fleeting note into `inbox.org` using `org roam fleeting capture`.
   ```sh
   org roam fleeting capture "Review Zensical documentation links" --tags review --workspace .
   ```

## Expected Outcome

`org fmt check tasks.org` exits with code `0` and emits zero warnings.
`org query search` and `org query agenda` print your scheduled and deadline tasks in clean terminal tables:

```text
$ org query search 'todo:TODO tags:work' tasks.org
tasks.org:7: ** TODO Draft release announcement  :@writing:work:
tasks.org:9: ** TODO Verify package checksums    :@ops:work:
$ echo $?
0
```

## Power-User & Scripting Options

Chain `org query` with `org update` over standard streams (`stdin`/`stdout`) using universal `OrgLocator` addresses (`--format locator`).
Consult [Power-User & Scripting](power-user-and-scripting.md) and the [Glossary](../glossary.md) for pipeline details:

```sh
# Preview marking all @writing tasks as DONE without modifying files on disk
org query search 'todo:TODO tags:@writing' --format locator tasks.org \
  | org update set-state DONE --dry-run

# Inspect native Unix manual pages from your terminal
man 1 org
man 5 org-tools.toml
```

??? info "Deep Dive: Workspace Configuration Discovery"

    `org` looks for `.org-tools.toml` in the current directory and walks upward to the filesystem root.
    You can override the configuration path at any time with `--config <PATH>` or `ORG_TOOLS_CONFIG`.

## Troubleshooting

| Symptom                                             | Cause                                                 | Fix                                                                                |
| --------------------------------------------------- | ----------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `org: command not found`                            | Binary install directory is missing from shell `PATH` | Add `$HOME/.cargo/bin` or `$HOME/.local/bin` to `PATH` and reload your shell       |
| `"org" cannot be opened` quarantine prompt on macOS | Browser download set `com.apple.quarantine` attribute | Run `xattr -dr com.apple.quarantine "$(command -v org)"` or reinstall via Homebrew |
| `org query search` exits with code `1`              | No headings matched the filter expression             | Broaden the query predicate (for example, run `org query search 'todo' tasks.org`) |
| `org fmt check` exits with code `1`                 | Formatting or lint diagnostics were found in the file | Run `org fmt format tasks.org` to auto-fix spacing and alignment rules             |
