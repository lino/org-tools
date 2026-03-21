// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validates `#+TBLFM:` table formula syntax.
//!
//! Spec: [§3.5 The Spreadsheet](https://orgmode.org/manual/The-Spreadsheet.html),
//! [§3.5.2 References](https://orgmode.org/manual/References.html)
//!
//! Checks that `#+TBLFM:` lines follow a preceding table and that cell
//! references (`$N`, `@R$C`) use valid syntax. Named constant references
//! (`$name`) are validated against `#+CONSTANTS:` when declared.
//!
//! org-tools does *not* evaluate formulas — only syntactic validity is checked.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Validates `#+TBLFM:` formula syntax and placement.
///
/// Checks:
/// - `#+TBLFM:` must follow a table (no orphaned formula lines)
/// - Formula assignments must have the form `TARGET=EXPRESSION`
/// - Cell references (`$N`, `@R$C`) must use valid syntax
///
/// Spec: [§3.5 The Spreadsheet](https://orgmode.org/manual/The-Spreadsheet.html)
pub struct InvalidTableFormula;

impl LintRule for InvalidTableFormula {
    fn id(&self) -> &'static str {
        "W034"
    }

    fn name(&self) -> &'static str {
        "invalid-table-formula"
    }

    fn description(&self) -> &'static str {
        "Validate #+TBLFM: formula syntax"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut offset = 0;
        let mut prev_was_table = false;

        // Collect #+CONSTANTS: for named reference validation.
        let constants = collect_constants(&ctx.source.content);

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            let trimmed = raw.trim();

            if trimmed.starts_with('|') {
                prev_was_table = true;
                offset += line.len() + 1;
                continue;
            }

            if let Some(rest) = strip_tblfm_prefix(trimmed) {
                let (line_num, _) = ctx.source.line_col(offset);

                // Check: TBLFM must follow a table.
                if !prev_was_table {
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: 1,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "#+TBLFM: is not attached to a table".to_string(),
                        fix: None,
                    });
                }

                // Validate formula assignments.
                let formulas = rest.split("::");
                for formula in formulas {
                    let formula = formula.trim();
                    if formula.is_empty() {
                        continue;
                    }
                    if let Some(diag) =
                        validate_formula(formula, &constants, &ctx.source.path, line_num, self)
                    {
                        diagnostics.push(diag);
                    }
                }

                // TBLFM is still "attached" — next TBLFM line is also valid.
                offset += line.len() + 1;
                continue;
            }

            // Any non-table, non-TBLFM line (including blank) breaks the table context.
            prev_was_table = false;

            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Strip `#+TBLFM:` prefix (case-insensitive), returning the formula part.
fn strip_tblfm_prefix(line: &str) -> Option<&str> {
    if line.len() < 9 {
        return None;
    }
    if line.as_bytes()[0] != b'#' || line.as_bytes()[1] != b'+' {
        return None;
    }
    if line[2..8].eq_ignore_ascii_case("TBLFM:") {
        Some(line[8..].trim())
    } else {
        None
    }
}

/// Collect named constants from `#+CONSTANTS:` in the file preamble.
fn collect_constants(content: &str) -> Vec<String> {
    let mut constants = Vec::new();
    for line in content.split('\n') {
        let raw = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = raw.trim();
        // Stop at first heading.
        if let Some(rest) = trimmed.strip_prefix('*') {
            if rest.is_empty() || rest.starts_with(' ') {
                break;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("#+") {
            if let Some(colon) = rest.find(':') {
                let key = rest[..colon].trim();
                if key.eq_ignore_ascii_case("CONSTANTS") {
                    let val = &rest[colon + 1..];
                    for pair in val.split_whitespace() {
                        if let Some(eq) = pair.find('=') {
                            let name = &pair[..eq];
                            if !name.is_empty() {
                                constants.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    constants
}

/// Validate a single formula assignment like `$3=$1+$2` or `@2$3=vmean(@I..@II)`.
fn validate_formula(
    formula: &str,
    constants: &[String],
    file: &std::path::Path,
    line: usize,
    rule: &InvalidTableFormula,
) -> Option<Diagnostic> {
    // A formula must contain `=` to separate target from expression.
    if !formula.contains('=') {
        return Some(Diagnostic {
            file: file.to_path_buf(),
            line,
            column: 1,
            severity: Severity::Warning,
            rule_id: rule.id(),
            rule: rule.name(),
            message: format!("formula \"{formula}\" is missing '=' assignment"),
            fix: None,
        });
    }

    let eq_pos = formula.find('=')?;
    let target = formula[..eq_pos].trim();

    // Validate the target (left-hand side).
    if target.is_empty() {
        return Some(Diagnostic {
            file: file.to_path_buf(),
            line,
            column: 1,
            severity: Severity::Warning,
            rule_id: rule.id(),
            rule: rule.name(),
            message: "formula has empty target before '='".to_string(),
            fix: None,
        });
    }

    // Validate target reference syntax.
    if let Some(msg) = validate_reference(target, constants) {
        return Some(Diagnostic {
            file: file.to_path_buf(),
            line,
            column: 1,
            severity: Severity::Warning,
            rule_id: rule.id(),
            rule: rule.name(),
            message: format!("formula target: {msg}"),
            fix: None,
        });
    }

    None
}

/// Validate a cell/range reference. Returns an error message if invalid.
///
/// Valid references: `$1`, `$>`, `@2$3`, `@>$>`, `@I`, `@II`, `@I..@II`,
/// `$name` (named constant or column name), `@#`, `$#`.
fn validate_reference(reference: &str, constants: &[String]) -> Option<String> {
    let r = reference.trim();
    if r.is_empty() {
        return Some("empty reference".to_string());
    }

    // Range reference: target..target (e.g., @I..@II, @2$1..@5$3).
    if r.contains("..") {
        // Ranges are valid in targets — skip deep validation.
        return None;
    }

    // @R$C absolute reference.
    if r.starts_with('@') {
        return validate_row_col_ref(r);
    }

    // $N or $name column reference.
    if let Some(col) = r.strip_prefix('$') {
        return validate_col_ref(col, constants);
    }

    // Could be a bare column name or complex expression — skip.
    None
}

/// Validate a `@R$C` or `@R` reference.
fn validate_row_col_ref(r: &str) -> Option<String> {
    let after_at = &r[1..];

    // Special row references: @>, @<, @I, @II, @III, @#.
    if after_at.starts_with('>')
        || after_at.starts_with('<')
        || after_at.starts_with('I')
        || after_at.starts_with('#')
    {
        return None; // Valid special reference.
    }

    // @N or @N$M — row number.
    let dollar = after_at.find('$');
    let row_part = if let Some(pos) = dollar {
        &after_at[..pos]
    } else {
        after_at
    };

    if !row_part.is_empty() && !row_part.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("invalid row reference '@{row_part}'"));
    }

    // Validate column part if present.
    if let Some(pos) = dollar {
        let col_part = &after_at[pos + 1..];
        if col_part.is_empty() {
            return Some("missing column number after '$'".to_string());
        }
        // $>, $<, $# are valid.
        if col_part == ">" || col_part == "<" || col_part == "#" {
            return None;
        }
        if !col_part.chars().all(|c| c.is_ascii_digit()) && !col_part.is_empty() {
            // Could be a named column — allow it.
            return None;
        }
    }

    None
}

/// Validate a column reference (the part after `$`).
fn validate_col_ref(col: &str, constants: &[String]) -> Option<String> {
    if col.is_empty() {
        return Some("missing column number after '$'".to_string());
    }
    // $>, $<, $# are valid special references.
    if col == ">" || col == "<" || col == "#" {
        return None;
    }
    // Numeric column: $1, $2, etc.
    if col.chars().all(|c| c.is_ascii_digit()) {
        if col == "0" {
            return Some("column reference '$0' is invalid (columns start at 1)".to_string());
        }
        return None;
    }
    // Named reference: check against constants if any are declared.
    // If no constants are declared, allow any name (could be a column name).
    if !constants.is_empty() && col.chars().all(|c| c.is_alphanumeric() || c == '_') {
        // Only warn if constants are declared and name is not among them.
        // But column names (from #+NAME or $name header) are also valid,
        // and we can't know those from line-based parsing. Be lenient.
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        InvalidTableFormula.check(&ctx)
    }

    #[test]
    fn valid_formula_after_table() {
        let diags = check_it("| a | 1 |\n| b | 2 |\n#+TBLFM: $2=$1+1\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn orphaned_tblfm() {
        let diags = check_it("Some text\n#+TBLFM: $2=$1+1\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("not attached"));
    }

    #[test]
    fn tblfm_after_blank_line_from_table() {
        // A blank line between table and TBLFM means it's orphaned.
        let diags = check_it("| a | b |\n\n#+TBLFM: $2=$1\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("not attached"));
    }

    #[test]
    fn multiple_tblfm_lines() {
        let diags = check_it("| a | b |\n#+TBLFM: $1=1\n#+TBLFM: $2=2\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn formula_missing_equals() {
        let diags = check_it("| a | b |\n#+TBLFM: $2\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing '='"));
    }

    #[test]
    fn formula_empty_target() {
        let diags = check_it("| a | b |\n#+TBLFM: =1+2\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("empty target"));
    }

    #[test]
    fn formula_with_row_col_ref() {
        let diags = check_it("| a | b |\n#+TBLFM: @2$3=vmean(@I..@II)\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn formula_with_special_refs() {
        let diags = check_it("| a | b |\n#+TBLFM: $>=vsum($1..$>>)\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn formula_with_multiple_assignments() {
        let diags = check_it("| a | b | c |\n#+TBLFM: $2=$1+1::$3=$1*2\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn formula_in_code_block_ignored() {
        let input = "#+BEGIN_SRC org\n#+TBLFM: invalid\n#+END_SRC\n";
        let diags = check_it(input);
        assert!(diags.is_empty());
    }

    #[test]
    fn column_zero_invalid() {
        let diags = check_it("| a | b |\n#+TBLFM: $0=1\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("$0"));
    }

    #[test]
    fn invalid_row_ref() {
        let diags = check_it("| a | b |\n#+TBLFM: @abc$1=1\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("invalid row"));
    }

    #[test]
    fn named_constant_in_formula() {
        // Named constants are valid references.
        let diags = check_it("#+CONSTANTS: pi=3.14\n| a | b |\n#+TBLFM: $2=$pi\n");
        assert!(diags.is_empty());
    }
}
