<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Contributing & Red-to-Green TDD Guide

## Overview

**What you will learn:**

- How to practice Red-to-Green Test-Driven Development (TDD) when adding features or fixing bugs.
- How to implement, document, and register a new `FormatRule` or `LintRule` in `org-tools-core`.
- How to pass all complexity, formatting, `clippy`, `rustdoc`, and self-dogfooding verification gates.

Writing failing behavioral tests before production code prevents regressions and clarifies edge cases upfront.
Strict function size and complexity limits keep the codebase approachable for every contributor.
Independent rule modules let you add new Org-mode checks without breaking existing formatters or queries.

## Prerequisites

- Rust stable toolchain (`rustc`, `cargo`, `rustfmt`, and `clippy`) installed via `mise` or `rustup`.
- `mise` task runner installed for executing standard repository workflows (`mise run test`, `mise run verify`).
- Familiarity with the [Architecture (`arc42`)](architecture.md) crate map and the [Glossary](../glossary.md).

## Steps

1. Run `mise run setup` in the repository root to verify your Rust toolchain and build dependencies.
   ```sh
   mise run setup
   ```
1. Write a failing unit test in `crates/org-tools-core/` or an integration test in `crates/org/tests/` first.
   Include clean inputs, dirty inputs, Unicode characters, and protected literal blocks (`#+BEGIN_SRC`).
1. Run `mise run test` and confirm your new test fails with the expected assertion message (the **Red** phase).
   ```sh
   mise run test
   ```
1. Create or update the rule module under `crates/org-tools-core/src/rules/` with the required Software Package Data Exchange (SPDX) header.
   Cite the authoritative Org Mode Manual or Org Syntax specification URL in the struct `///` docstring:
   ```rust
   // Copyright (C) 2026 org-tools contributors
   // SPDX-License-Identifier: GPL-3.0-or-later

   //! Format or lint rule implementation.

   use crate::diagnostic::{Diagnostic, Severity, Span};
   use crate::rules::regions;
   use crate::rules::{LintContext, LintRule};

   /// Checks headings according to <https://orgmode.org/worg/org-syntax.html>.
   pub struct ExampleLintRule;

   impl LintRule for ExampleLintRule {
       fn id(&self) -> &'static str {
           "W099"
       }

       fn name(&self) -> &'static str {
           "example-heading-check"
       }

       fn description(&self) -> &'static str {
           "Reports headings that violate the example convention."
       }

       fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
           let protected = regions::protected_regions(ctx.source);
           let mut out = Vec::new();
           for (idx, line) in ctx.source.lines().enumerate() {
               let line_num = idx + 1;
               if regions::is_protected(&protected, line_num) {
                   continue;
               }
               if line.starts_with("* FIXME") {
                   let start = ctx.source.line_start(line_num);
                   out.push(Diagnostic {
                       file: ctx.source.path().to_string(),
                       line: line_num,
                       column: 1,
                       severity: Severity::Warning,
                       rule_id: self.id().to_string(),
                       rule_name: self.name().to_string(),
                       message: "avoid FIXME keyword in heading".to_string(),
                       span: Span { start, end: start + line.len() },
                   });
               }
           }
           out
       }
   }
   ```
1. Register your new rule in `crates/org-tools-core/src/rules/mod.rs` and `crates/org-tools-core/src/runner.rs`.
1. Refactor your implementation to satisfy all craft and modularity invariants (the **Green** and **Refactor** phases):
   - Keep every function or method at or below 50 non-blank statement lines.
   - Keep McCabe cyclomatic complexity at or below 10 branches per function.
   - Keep every source file at or below 450 lines.
   - Add complete-sentence Google-style `//!` and `///` docstrings starting with a capital letter and ending with a period.
1. Run `mise run verify` to execute formatting, `clippy`, unit/integration tests, and self-dogfooding checks.
   ```sh
   mise run verify
   ```

## Expected Outcome

`mise run verify` compiles the workspace cleanly, passes all unit and CLI tests, and exits with code `0`.
Self-dogfooding (`org fmt check README.org docs/`) and `rustdoc` checks complete with zero warnings:

```text
$ mise run verify
Running cargo fmt -- --check... OK
Running cargo clippy --all-targets -- -D warnings... OK
Running cargo test... test result: ok. 0 failed
Running cargo run -- fmt check README.org docs/... OK (0 diagnostics)
```

## Power-User & Scripting Options

Run targeted test binaries during rapid Red-to-Green iteration to get sub-second feedback:

```sh
# Run a single integration test target
cargo test --test repo_modernization_tests

# Verify rustdoc intra-doc links with warnings denied
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# List all embedded JSON Schema Draft 2020-12 contracts
cargo run -- schema --list
```

??? info "Deep Dive: Rule ID Prefixes, JetBrains Regions, and Schema Sync"
\- **Rule ID Prefixes**: Use `F` for auto-fixes, `E` for errors, `W` for warnings, and `I` for info suggestions.
\- **JetBrains Regions**: Wrap files with more than three logical sections in `// region <Name>` comments and update `.regions.md`.
\- **Synchronized Contracts**: Update `--completions`, `schemas/*.schema.json`, `man/org.1`, `man/org-tools.toml.5`, and Zensical manuals whenever CLI contracts change.

## Troubleshooting

| Symptom                                                   | Cause                                                                        | Fix                                                                                        |
| --------------------------------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `cargo fmt -- --check` exits with code `1`                | Rust source files contain unformatted lines.                                 | Run `cargo fmt` across the workspace and re-run `mise run verify`.                         |
| `clippy::missing_docs_in_private_items` or doc error      | A module, struct, field, function, or test lacks a `//!` or `///` docstring. | Add a complete-sentence docstring starting with a capital letter and ending with a period. |
| Format rule corrupts code inside `#+BEGIN_SRC`            | The rule did not check `regions::protected_regions` before emitting a `Fix`. | Call `regions::is_protected(&protected, line_num)` and skip protected lines.               |
| `org fmt check README.org docs/` reports `F005` or `F007` | A repository `.org` file has unaligned drawers or tags.                      | Run `cargo run -- fmt format README.org docs/` and inspect the diff.                       |
| `schema` contract test fails on `--format json`           | New JSON fields were added without updating `schemas/*.schema.json`.         | Update the matching schema in `schemas/` and verify with `cargo test`.                     |
