<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# `org-tools` — Canonical Engineering & Agent Instructions

`org-tools` is a standalone, pure-Rust CLI suite (`org`) for Org-mode linting, formatting, querying, mutation, time-clocking, archiving, and calendar export without requiring an Emacs runtime.

## License & File Headers

All repository code is licensed under **`GPL-3.0-or-later`** in solidarity with the Org-mode ecosystem. Every source, configuration, and script file must carry the standard SPDX header:

```rust
// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
```

(Use `#` comments for Bash/TOML/YAML and `<!-- ... -->` for Markdown.)

## Workspace & Module Architecture

Cargo workspace with two crates:

- `crates/org-tools-core/` — Shared library: `SourceFile`, `OrgDocument` (including level-0 `org-roam` v2 file `:ID:` drawers, `KEY+` continuations, and `ChecklistItem` parsing), `OrgLocator`, `FormatRule` / `LintRule` traits and rules, `Runner`, `property` (F005 drawer mutations, F007 column-77 tag alignment, and Stempelamt RFC 9562 UUIDv5 minting), `id`, `entry`, `state`, `cookie`, `archive`, `edna`, and `tblfm`.
- `crates/org/` — Umbrella `org` binary:
  - `crates/org/src/main.rs` — Top-level `clap` CLI parser, exit-code constants, config loading, and subcommand dispatch.
  - `crates/org/src/commands/` — Modular subcommand handlers (`mod.rs`, `fmt.rs`, `query.rs`, `query_gtd.rs`, `update.rs`, `update_id.rs`, `update_property.rs`, `update_state.rs`, `update_table.rs`, `update_todo.rs`, `clock.rs`, `archive.rs`, `export.rs`, `schema.rs`).
  - `crates/org/src/{query,clock,export,date}/` — Query parser/evaluator/output/agenda/deps, clock report/status, and iCalendar/JSCalendar exporters.

When adding or changing CLI flags, subcommands, or output schemas, keep shell completions (`--completions`), `schemas/*.schema.json`, `docs/user/usage.org`, and `docs/technical/architecture.org` synchronized.

## Org Specification Citations & Rule Conventions

Every format or lint rule must cite the authoritative Org-mode specification in its struct docstring:

- **Org Mode Manual**: <https://orgmode.org/manual/>
- **Org Syntax (formal spec)**: <https://orgmode.org/worg/org-syntax.html>
- **Emacs Ground Truth**: `org-lint.el` and `org-element.el` (<https://git.savannah.gnu.org/cgit/emacs/org-mode.git/tree/lisp/>)
- **Ecosystem Extensions**: `docs/org-ecosystem.org`

### Rule ID Scheme & Protected Regions

- `F` + 3 digits: auto-fixable format rules (e.g., `F001` `trailing-whitespace`), returning `Vec<Fix>` sorted by `span.start`.
- `E` + 3 digits: structural error lint rules (e.g., `E001` `unclosed-block`).
- `W` + 3 digits: correctness/style warning lint rules (e.g., `W001` `heading-level-gap`).
- `I` + 3 digits: informational lint suggestions (e.g., `I003` `keyword-validity`).
- Always respect `regions::protected_regions` and `regions::is_protected` so literal block bodies (`src`, `example`, `export`, `quote`, `verse`, `center`, `comment`) are never corrupted. Keep rules independent of one another.

## Exit Codes

All `org` subcommands return consistent exit codes defined in `crates/org/src/main.rs`:

| Code | Constant              | Meaning                                                    |
| ---- | --------------------- | ---------------------------------------------------------- |
| `0`  | `EXIT_OK`             | Operation completed cleanly                                |
| `1`  | `EXIT_ISSUES`         | No query matches found, or lint/format diagnostics emitted |
| `2`  | `EXIT_ERROR`          | Operational failure (missing file, parse error, bad args)  |
| `3`  | `EXIT_REQUIRES_EMACS` | Formula or feature requires Emacs Lisp or full Emacs Calc  |

## F.R.I.T.Z. Craft & Quality Invariants

- **Red-to-Green TDD**: Write failing unit and integration tests (`crates/org/tests/`) before implementing production changes, including clean/dirty inputs, protected regions, and Unicode edge cases.
- **Function Size & Complexity**: Every function/method must stay `<= 50` non-blank statement lines with McCabe cyclomatic complexity `<= 10`.
- **File Modularity**: Keep source files `<= 450` lines (`// fritz: allow-file-length=<max> reason="<why>"` only when splitting genuinely hurts cohesion).
- **100% Google-Style Docstrings**: Every module (`//!`), struct, enum, variant, field, trait, method, function, and test (`///`) must carry complete-sentence documentation starting with a capital letter and ending with a period.
- **JetBrains Regions**: Files with `> 3` logical sections must wrap sections in `// region <Name>` / `// endregion` (or `# region <Name>`) and register them in git-ignored `.regions.md`.
- **Zero Magic Values & Zero Hardcoded Paths**: Never commit machine-specific paths; expose defaults via named constants overridable by CLI flags, `.org-tools.toml`, or environment variables.

## Verification Gates

Run the full verification suite via `mise` or `cargo` before concluding any change:

```sh
mise run test                                    # Run unit and CLI integration tests
mise run verify                                  # Run fmt, clippy --all-targets, tests, and self-dogfood docs check
cargo fmt -- --check                             # Check Rust formatting
cargo clippy --all-targets -- -D warnings        # Lint library, binary, and all test targets
cargo test                                       # Run full workspace test suite
cargo run -- fmt check README.org docs/          # Self-dogfood org fmt check on repository docs
```
