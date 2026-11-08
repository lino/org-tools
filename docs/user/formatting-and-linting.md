<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Formatting & Linting (`org fmt`)

## Overview

**What you will learn:**

- How `org fmt format` automatically repairs whitespace, tables, property drawers, tags, lists, and timestamps (`F001..F009`).
- How `org fmt check` reports structural errors (`E001..E010`), warnings (`W001..W037`), and style tips (`I001..I004`).
- How to customize formatting rules and disable specific lint rules in `.org-tools.toml`.

Manual edits across multiple editors often leave Org tables crooked, property drawers unaligned, and blocks unclosed.
`org fmt` formats cosmetic spacing first and then lints the normalized document in a single fast pass.
Protected literal blocks (`src`, `example`, `quote`, `verse`, `export`, `center`, `comment`) are never modified.

## Prerequisites

- `org` installed on your workstation or CI runner (see [Five-Minute Quickstart](quickstart.md)).
- One or more `.org` files or a workspace directory to check.
- Optional: An `.org-tools.toml` configuration file in your project root.

## Steps

1. Run `org fmt check` on a file or directory to inspect formatting and lint diagnostics without changing files.
   ```sh
   org fmt check notes.org
   org fmt check ~/org/
   ```
1. Preview auto-formatted output on `stdout` before writing changes to disk by passing `--stdout`.
   ```sh
   org fmt format --stdout notes.org
   ```
1. Run `org fmt format` (or `org fmt check --fix`) to apply all enabled format rules (`F001..F009`) in place.
   ```sh
   org fmt format notes.org
   org fmt check --fix ~/org/
   ```
1. Configure rule behavior or disable specific lint rules in `.org-tools.toml` using Tom's Obvious Minimal Language (TOML).
   ```toml
   [format]
   trailing_whitespace = true
   blank_lines = false
   heading_blank_lines = false
   table_format = true
   property_drawer_align = true
   tag_column = 77

   [lint]
   disabled_rules = ["W001", "I001"]
   ```
1. Enforce formatting in Continuous Integration (CI) pipelines with `org fmt format --check`.
   ```sh
   org fmt format --check .
   org fmt check .
   ```

## Expected Outcome

When all files conform to your `.org-tools.toml` rules, `org fmt check` prints nothing and exits with code `0`.
When issues exist, `org fmt check` prints `file:line:col` diagnostics and exits with code `1`:

```text
$ org fmt check broken.org
broken.org:5:1: warning [W001/heading-level-gap] heading level jumps from 1 to 3
broken.org:12:1: error [E001/unclosed-block] #+BEGIN_SRC without matching #+END_SRC
$ echo $?
1
```

## Power-User & Scripting Options

Emit machine-readable diagnostics with `--format json` (validated against `org schema diagnostic`) or pipe buffer text over `stdin`.
See the [CLI, Config & Rule Reference](reference.md) and [JSON Schemas & Contracts](schemas.md) for every rule ID:

```sh
# Filter only error-severity diagnostics with jq
org fmt check --format json ~/org/ | jq '.[] | select(.severity == "error")'

# Format an Org snippet from stdin and write the result to stdout
cat draft.org | org fmt format > formatted.org
```

??? info "Deep Dive: Two-Phase Pipeline & Protected Literal Regions"
`Runner` executes all enabled `FormatRule` passes first, sorts non-overlapping byte-span `Fix` values, and applies them in one pass.
`LintRule` passes then inspect the formatted buffer so cosmetic spacing never causes false lint warnings.
Every format rule checks `regions::is_protected` to skip lines inside `#+BEGIN_*` and `#+END_*` literal blocks.

## Troubleshooting

| Symptom                                                      | Cause                                                               | Fix                                                                                |
| ------------------------------------------------------------ | ------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `error [E001/unclosed-block]`                                | A `#+BEGIN_*` block is missing its matching `#+END_*` delimiter     | Add the closing `#+END_*` line at the end of the block                             |
| `error [E003/include-validity]`                              | `#+INCLUDE:` target is missing, escapes workspace, or loops         | Fix the relative file path inside `#+INCLUDE:` so it resolves inside the workspace |
| `error [E009/duplicate-id]` or `[E010/invalid-id-seed]`      | Two nodes share an `:ID:` or `:ID:` mismatches `:ID_SEED:`          | Run `org update add-id --deterministic` to regenerate clean UUIDv5 identifiers     |
| `warning [W001/heading-level-gap]`                           | A heading jumps more than one asterisk level (such as `*` to `***`) | Adjust the heading asterisks or add `"W001"` to `[lint].disabled_rules`            |
| Content inside `#+BEGIN_SRC` was not indented or reformatted | Source blocks are protected regions by design                       | Format code inside source blocks with a language-specific formatter                |
