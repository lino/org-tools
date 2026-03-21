# org-tools — Project Instructions

## What This Is

org-tools is a standalone org-mode linter and formatter written in Rust. It runs
without Emacs, produces diagnostics in human-readable or JSON format, and
auto-fixes formatting issues. Think ruff/prettier for org-mode.

## License

GPL-3.0-or-later.

org-tools's rule set was designed by studying
[org-lint.el](https://git.savannah.gnu.org/cgit/emacs/org-mode.git/tree/lisp/org-lint.el)
and the [org-mode specification](https://orgmode.org/worg/org-syntax.html).
We chose GPL to respect the ecosystem that made this possible. The org-mode
community has built the specification, the parser, and decades of collective
knowledge under GPL — org-tools plays by the same rules.

All source files should carry the standard GPL copyright header:
```
// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
```

## Architecture

### Workspace Structure

Cargo workspace with two crates:

- `crates/org-tools-core/` — shared library: rules, runner, config, source,
  document model, locator resolution
- `crates/org/` — umbrella binary: `org fmt`, `org query`, (future) `org clock`,
  `org update`, `org export`

### Core Concepts

- **Pure Rust**, single `org` binary via clap subcommands
- **Line-based parsing** — no external org-mode parser dependency. All rules
  work on raw text with manual line/offset calculations
- **OrgDocument** (`document.rs`) — lightweight heading tree built by single-pass
  line scan. Provides parent/child relationships, property extraction, planning
  timestamps, clock entries. Used by query, clock, and locator modules.
- **OrgLocator** (`locator.rs`) — universal heading address: `id:uuid`,
  `file::#custom-id`, `file::*/path`, `file::line`. Glue between commands.
- **Two rule traits**: `FormatRule` (returns `Vec<Fix>`, auto-fixable) and
  `LintRule` (returns `Vec<Diagnostic>`, report-only)
- **Protected regions** via `regions.rs` — identifies blocks where formatting
  rules must not touch content (src, example, quote, verse, center, comment,
  export blocks)
- **Runner** orchestrates: parse → run format rules → collect fixes →
  deduplicate → apply → run lint rules on result
- **Query engine** (`crates/org/src/query/`) — recursive descent parser for
  the query language, predicate evaluation with tag inheritance, agenda view

### Keep Current

When adding/removing CLI subcommands or arguments:
- Update shell completions (when implemented)
- Update `docs/user/usage.org` with new command documentation
- Update `docs/technical/architecture.org` module table if new modules added

## Specification Reference

Every rule must reference the specific section of the org-mode specification it
implements. The two authoritative sources are:

- **Org Mode Manual**: https://orgmode.org/manual/
- **Org Syntax (formal spec)**: https://orgmode.org/worg/org-syntax.html

Additional references:
- **200ok org-parser EBNF grammar**: https://github.com/200ok-ch/org-parser/blob/master/resources/org.ebnf
  (Clojure-based parser with a formal grammar — useful for cross-checking
  syntax rules, but has known gaps: no nested markup, no list nesting,
  no citations, no inline src blocks)
- **Org ecosystem doc**: `docs/org-ecosystem.org` — catalogs packages that
  extend org syntax (org-roam, org-ref, ox-hugo, etc.) and what syntax
  they add. Consult this when implementing keyword-validity or similar
  rules to avoid false positives.

When implementing a new rule, always:

1. Read the relevant manual section first
2. Read the corresponding formal syntax section
3. Fetch and read the Emacs source code for the feature:
   - org-mode source: https://git.savannah.gnu.org/cgit/emacs/org-mode.git/tree/lisp/
   - org-lint.el: https://git.savannah.gnu.org/cgit/emacs/org-mode.git/tree/lisp/org-lint.el
   - org-element.el: https://git.savannah.gnu.org/cgit/emacs/org-mode.git/tree/lisp/org-element.el
   - For specific features, find the relevant file (e.g., org-table.el for
     tables, org-list.el for lists, org-clock.el for clocking)
4. Document the spec reference in both:
   - The rule's source file (as a doc comment on the struct)
   - The feature matrix in the plan file

The Emacs source is the ground truth — when the manual is ambiguous, the
source code defines what is actually valid.

## Testing Requirements

### Unit Tests
- Every rule gets inline `#[cfg(test)] mod tests` with small org snippets
- Test both "no issue found" (clean input) and "issue detected" (dirty input)
- Test edge cases: empty files, files without trailing newline, unicode content
- Test protected region boundaries (content inside code blocks must be ignored)
- For format rules: verify the fix output matches expected formatted text

### Edge Case Test Files
- Comprehensive `.org` fixture files live in `tests/fixtures/edge_cases/`
- Each file targets a specific org-mode feature area
- Files must include:
  - Valid/correct usage (no diagnostics expected)
  - Common mistakes (diagnostics expected)
  - Boundary cases (start of file, end of file, adjacent elements)
  - Content inside protected regions (must be ignored)
  - Unicode/special character content
  - **org-babel blocks** with various languages, header arguments, noweb
    references, sessions, tangling, variable passing, inline source blocks,
    CALL syntax, result blocks, and literate programming patterns

### CLI Integration Tests
- In `tests/cli_tests.rs` using `assert_cmd` + `predicates` + `tempfile`
- Test: exit codes, recursive directory scanning, format --check mode,
  format --stdout, JSON output, format-in-place

### Comparing with Emacs org-mode

Two scripts automate the comparison:

```sh
# Compare formatting output (tables, whitespace, etc.)
./scripts/compare-with-emacs.sh tests/fixtures/edge_cases/*.org

# Compare lint diagnostics
./scripts/compare-lint-with-emacs.sh tests/fixtures/edge_cases/*.org
```

When implementing or testing a rule:
1. Create a test `.org` file that exercises the rule
2. Run the comparison scripts above
3. Document any intentional divergences from Emacs behavior

Known divergences to track:
- Emacs right-aligns numbers in tables; org-tools left-aligns everything (TODO)
- org-tools adds blank lines before headings; Emacs does not auto-format heading spacing
- Emacs org-lint validates source block languages against its Babel language list; org-tools only checks presence

### Verification After Every Change
1. `cargo test` — all tests pass
2. `cargo clippy -- -D warnings` — clean
3. `cargo run -- fmt check tests/fixtures/edge_cases/` — runs without panic
4. Manual spot-check on a real-world `.org` file

## Documentation Requirements

### Rule Documentation
Every rule's struct must have a doc comment containing:
- One-line description of what the rule checks/formats
- Spec reference (manual section URL + syntax spec URL)
- Example of the problem it detects or the formatting it applies
- Any known divergences from Emacs behavior

### Feature Matrix
Maintain the feature matrix in the plan file mapping org spec areas to
implemented rules. Every row must include:
- Org spec area and element name
- Link to the relevant manual section
- Link to the relevant syntax spec section
- Which format rule covers it (if any)
- Which lint rule covers it (if any)
- Implementation status (done, this phase, future)

### Code Comments
- Complex parsing logic must have inline comments explaining the org-mode
  syntax being matched
- Regex-like patterns (string matching with `starts_with`, `find`, etc.)
  must document what org syntax they correspond to

## Code Style

Follow the [Google Rust style guide](https://google.github.io/comprehensive-rust/style-guide.html)
for comments and documentation:
- Use `///` doc comments on all public items (structs, functions, traits, methods).
- Doc comments should be complete sentences starting with a capital letter and
  ending with a period.
- Use `//` line comments for implementation details. These should explain *why*,
  not *what*.
- Keep comments concise. Don't restate what the code already says.
- Use `// TODO:` for planned work, `// FIXME:` for known bugs.

## Code Conventions

- Follow the existing pattern: rule struct → trait impl → tests module
- Sort fixes by `span.start` before returning from format rules
- Use `regions::protected_regions` + `regions::is_protected` in any format
  rule that modifies content (to skip code blocks)
- Lint rules should also skip protected regions where the syntax being
  checked would be literal text (e.g., footnotes in code blocks)
- Severity levels: Error for structural issues (unclosed blocks, duplicates),
  Warning for style issues, Info for suggestions
- Keep rules independent — no rule should depend on another rule's output

### Rule ID Scheme

Every rule has a unique identifier for enable/disable in config:
- `F` + 3 digits: format rules (auto-fix). E.g., `F001` trailing-whitespace.
- `E` + 3 digits: error-level lint rules (structural). E.g., `E001` unclosed-block.
- `W` + 3 digits: warning-level lint rules (correctness). E.g., `W001` heading-level-gap.
- `I` + 3 digits: info-level lint rules (suggestions).

Users can disable rules by either ID or name in `.org-tools.toml`:
```toml
[lint]
disabled_rules = ["W001", "heading-level-gap"]
```

### org-lint Feature Parity

Emacs org-lint implements ~58 checks. See `docs/org-ecosystem.org` for the full
list. When adding new lint rules, reference the corresponding org-lint checker
name for traceability. Prioritize the 35 checks that are fully implementable
with line-based parsing.

## Building and Running

```sh
cargo build                              # build
cargo test                               # run all tests
cargo clippy -- -D warnings              # lint
cargo run -- fmt check file.org              # check a file
cargo run -- fmt format --stdout file.org    # preview formatting
cargo run -- fmt check tests/fixtures/edge_cases/  # check all edge case files
```
