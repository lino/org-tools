// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table formula (`#+TBLFM:`) parser and evaluator.
//!
//! Evaluates the subset of org-mode table formulas that can be computed
//! without Emacs Calc or Emacs Lisp. Supported: basic arithmetic (`+`, `-`,
//! `*`, `/`), aggregate functions (`vsum`, `vmean`, `vmin`, `vmax`, `vcount`),
//! math functions (`abs`, `round`, `ceil`, `floor`), cell references (`$N`,
//! `@R$C`), ranges (`@I..@II`), and named constants (`#+CONSTANTS:`).
//!
//! Spec: [§3.5 The Spreadsheet](https://orgmode.org/manual/The-Spreadsheet.html),
//! [§3.5.2 References](https://orgmode.org/manual/References.html)

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Table model
// ---------------------------------------------------------------------------

/// A table parsed for formula evaluation.
#[derive(Debug, Clone)]
pub struct EvalTable {
    /// Table rows in document order (data and separator).
    pub rows: Vec<EvalRow>,
    /// Number of columns (max across all data rows).
    pub col_count: usize,
    /// Indices into `rows` that are separator rows (for @I, @II, etc.).
    pub separators: Vec<usize>,
}

/// A single row in the evaluation table.
#[derive(Debug, Clone)]
pub enum EvalRow {
    /// A horizontal rule / separator row.
    Separator,
    /// A data row with cell values.
    Data(Vec<CellValue>),
}

/// A cell value.
#[derive(Debug, Clone)]
pub enum CellValue {
    /// Numeric value.
    Number(f64),
    /// Non-numeric text.
    Text(String),
    /// Empty cell.
    Empty,
}

impl CellValue {
    /// Try to interpret as a number.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            CellValue::Number(n) => Some(*n),
            CellValue::Text(s) => parse_number(s),
            CellValue::Empty => None,
        }
    }
}

/// Parse a string as a number, handling thousands separators and currency.
fn parse_number(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Strip currency symbols and percent.
    let cleaned: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '$' | '€' | '£' | '¥' | '%'))
        .collect();
    // Strip thousands separators (commas between digits).
    let no_commas: String = cleaned.replace(',', "");
    no_commas.trim().parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// Cell references
// ---------------------------------------------------------------------------

/// A row reference in a formula.
#[derive(Debug, Clone, PartialEq)]
pub enum RowRef {
    /// `@N` — absolute row (1-indexed, counting only data rows).
    Absolute(usize),
    /// `@>` — last data row.
    Last,
    /// `@<` — first data row.
    First,
    /// `@I`, `@II`, etc. — first data row after the Nth separator.
    HGroup(usize),
    /// `@-N`, `@+N` — relative to the current row.
    Relative(i32),
}

/// A column reference in a formula.
#[derive(Debug, Clone, PartialEq)]
pub enum ColRef {
    /// `$N` — absolute column (1-indexed).
    Absolute(usize),
    /// `$>` — last column.
    Last,
    /// `$<` — first column.
    First,
    /// `$name` — named constant or column header.
    Named(String),
}

/// A target specifying where to write a formula result.
#[derive(Debug, Clone, PartialEq)]
pub enum CellTarget {
    /// `@R$C` — single cell.
    Cell(RowRef, ColRef),
    /// `$C` — entire column (apply to each data row).
    Column(ColRef),
}

// ---------------------------------------------------------------------------
// Expression AST
// ---------------------------------------------------------------------------

/// A parsed expression from a TBLFM formula.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Literal number.
    Number(f64),
    /// Cell reference: `@R$C`.
    CellRef(RowRef, ColRef),
    /// Range: `@R1$C1..@R2$C2`.
    Range(Box<RangeRef>),
    /// Binary operation.
    BinOp(Box<Expr>, Op, Box<Expr>),
    /// Unary negation.
    Neg(Box<Expr>),
    /// Function call: `vsum(...)`, `abs(...)`.
    FnCall(String, Vec<Expr>),
    /// Named constant from `#+CONSTANTS:`.
    Constant(String),
    /// Unsupported Emacs Lisp expression (triggers exit code 3).
    Elisp(String),
}

/// A range reference.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeRef {
    /// Start row.
    pub start_row: RowRef,
    /// Start column.
    pub start_col: ColRef,
    /// End row.
    pub end_row: RowRef,
    /// End column.
    pub end_col: ColRef,
}

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
    /// `%`.
    Mod,
}

/// A parsed TBLFM assignment.
#[derive(Debug, Clone)]
pub struct Assignment {
    /// Where to write the result.
    pub target: CellTarget,
    /// The expression to evaluate.
    pub expr: Expr,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error during formula parsing or evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum TblfmError {
    /// Syntax error in the formula.
    Parse(String),
    /// Formula uses unsupported Emacs features.
    RequiresEmacs(String),
    /// Runtime evaluation error (division by zero, invalid reference, etc.).
    Eval(String),
}

impl std::fmt::Display for TblfmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TblfmError::Parse(msg) => write!(f, "formula parse error: {msg}"),
            TblfmError::RequiresEmacs(msg) => write!(f, "requires Emacs: {msg}"),
            TblfmError::Eval(msg) => write!(f, "evaluation error: {msg}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Table parsing (from org source text)
// ---------------------------------------------------------------------------

/// Parse table lines into an [`EvalTable`].
///
/// `lines` should be the raw text lines of the table (each starting with `|`).
pub fn parse_eval_table(lines: &[&str]) -> EvalTable {
    let mut rows = Vec::new();
    let mut separators = Vec::new();
    let mut max_cols = 0usize;

    for line in lines {
        let trimmed = line.trim();
        if is_separator(trimmed) {
            separators.push(rows.len());
            rows.push(EvalRow::Separator);
        } else {
            let cells = parse_data_cells(trimmed);
            max_cols = max_cols.max(cells.len());
            rows.push(EvalRow::Data(cells));
        }
    }

    EvalTable {
        rows,
        col_count: max_cols,
        separators,
    }
}

fn is_separator(line: &str) -> bool {
    if !line.starts_with('|') {
        return false;
    }
    let inner = line.trim_start_matches('|').trim_end_matches('|');
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| c == '-' || c == '+' || c == '|' || c == ' ')
        && inner.contains('-')
}

fn parse_data_cells(line: &str) -> Vec<CellValue> {
    let inner = line.strip_prefix('|').unwrap_or(line);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner
        .split('|')
        .map(|cell| {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                CellValue::Empty
            } else if let Some(n) = parse_number(trimmed) {
                CellValue::Number(n)
            } else {
                CellValue::Text(trimmed.to_string())
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reference parsing
// ---------------------------------------------------------------------------

/// Parse a row reference like `@2`, `@>`, `@I`, `@-1`.
fn parse_row_ref(s: &str) -> Result<RowRef, TblfmError> {
    let s = s.trim();
    match s {
        ">" => Ok(RowRef::Last),
        "<" => Ok(RowRef::First),
        "#" => Ok(RowRef::Relative(0)), // Current row.
        _ => {
            // Roman numeral hgroup references.
            if s.chars().all(|c| c == 'I' || c == 'V' || c == 'X') && !s.is_empty() {
                let n = parse_roman(s);
                return Ok(RowRef::HGroup(n));
            }
            // Relative: -1, +2.
            if s.starts_with('-') || s.starts_with('+') {
                return s
                    .parse::<i32>()
                    .map(RowRef::Relative)
                    .map_err(|_| TblfmError::Parse(format!("invalid relative row: @{s}")));
            }
            // Absolute.
            s.parse::<usize>()
                .map(RowRef::Absolute)
                .map_err(|_| TblfmError::Parse(format!("invalid row reference: @{s}")))
        }
    }
}

/// Parse a column reference like `$1`, `$>`, `$name`.
fn parse_col_ref(s: &str) -> Result<ColRef, TblfmError> {
    let s = s.trim();
    match s {
        ">" => Ok(ColRef::Last),
        "<" => Ok(ColRef::First),
        "#" => Ok(ColRef::Absolute(0)), // Column counter (context-dependent).
        _ => {
            if let Ok(n) = s.parse::<usize>() {
                Ok(ColRef::Absolute(n))
            } else {
                Ok(ColRef::Named(s.to_string()))
            }
        }
    }
}

/// Parse a simple Roman numeral (I, II, III, IV, V, etc.).
fn parse_roman(s: &str) -> usize {
    let mut total = 0;
    let mut prev = 0;
    for ch in s.chars().rev() {
        let val = match ch {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            _ => 0,
        };
        if val < prev {
            total -= val;
        } else {
            total += val;
        }
        prev = val;
    }
    total
}

/// Parse a cell reference from a formula token.
///
/// Formats: `@R$C`, `$C`, `@R`.
fn parse_cell_ref(s: &str) -> Result<(Option<RowRef>, Option<ColRef>), TblfmError> {
    if let Some(rest) = s.strip_prefix('@') {
        if let Some(dollar_pos) = rest.find('$') {
            let row_part = &rest[..dollar_pos];
            let col_part = &rest[dollar_pos + 1..];
            Ok((
                Some(parse_row_ref(row_part)?),
                Some(parse_col_ref(col_part)?),
            ))
        } else {
            Ok((Some(parse_row_ref(rest)?), None))
        }
    } else if let Some(rest) = s.strip_prefix('$') {
        Ok((None, Some(parse_col_ref(rest)?)))
    } else {
        Err(TblfmError::Parse(format!("expected cell reference: {s}")))
    }
}

// ---------------------------------------------------------------------------
// Formula parser
// ---------------------------------------------------------------------------

/// Parse a TBLFM line into assignments.
///
/// A TBLFM line has the form: `TARGET=EXPR[::TARGET=EXPR...]`
pub fn parse_tblfm_line(line: &str) -> Result<Vec<Assignment>, TblfmError> {
    let formulas = line.split("::");
    let mut assignments = Vec::new();

    for formula in formulas {
        let formula = formula.trim();
        if formula.is_empty() {
            continue;
        }

        let eq_pos = formula
            .find('=')
            .ok_or_else(|| TblfmError::Parse(format!("missing '=' in formula: {formula}")))?;

        let target_str = formula[..eq_pos].trim();
        let expr_str = formula[eq_pos + 1..].trim();

        // Strip format specifiers (;%.2f etc.) from expression.
        let expr_str = strip_format_spec(expr_str);

        let target = parse_target(target_str)?;
        let expr = parse_expr(expr_str)?;
        assignments.push(Assignment { target, expr });
    }

    Ok(assignments)
}

/// Strip format specifiers (`;%...` or `;N` at end of expression).
fn strip_format_spec(s: &str) -> &str {
    // Format specs follow the last `;` not inside parens.
    let mut depth = 0i32;
    let mut last_semi = None;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth == 0 => last_semi = Some(i),
            _ => {}
        }
    }
    match last_semi {
        Some(i) => s[..i].trim(),
        None => s,
    }
}

/// Parse a target like `$3`, `@2$4`, `@>$4`.
fn parse_target(s: &str) -> Result<CellTarget, TblfmError> {
    let (row, col) = parse_cell_ref(s)?;
    match (row, col) {
        (Some(r), Some(c)) => Ok(CellTarget::Cell(r, c)),
        (None, Some(c)) => Ok(CellTarget::Column(c)),
        _ => Err(TblfmError::Parse(format!("invalid target: {s}"))),
    }
}

/// Parse an expression string into an [`Expr`] AST.
pub fn parse_expr(s: &str) -> Result<Expr, TblfmError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(TblfmError::Parse("empty expression".to_string()));
    }

    // Check for unsupported Emacs Lisp.
    if s.starts_with("lisp:") || s.starts_with("'(") {
        return Ok(Expr::Elisp(s.to_string()));
    }

    let tokens = tokenize_expr(s)?;
    let (expr, rest) = parse_additive(&tokens)?;
    if !rest.is_empty() {
        return Err(TblfmError::Parse(format!(
            "unexpected tokens after expression: {:?}",
            rest
        )));
    }
    Ok(expr)
}

// -- Tokenizer --

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    CellRef(String),       // @R$C or $C
    Range(String, String), // ref..ref
    Ident(String),         // function name or constant
    LParen,
    RParen,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
}

fn tokenize_expr(s: &str) -> Result<Vec<Token>, TblfmError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' => i += 1,
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' if !tokens.is_empty()
                && !matches!(
                    tokens.last(),
                    Some(
                        Token::Plus
                            | Token::Minus
                            | Token::Star
                            | Token::Slash
                            | Token::Percent
                            | Token::LParen
                            | Token::Comma
                    )
                ) =>
            {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '@' | '$' => {
                // Cell reference or range.
                let start = i;
                let ref1 = read_cell_ref(&chars, &mut i);
                // Check for range: ref..ref
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1] == '.' {
                    i += 2;
                    let ref2 = read_cell_ref(&chars, &mut i);
                    tokens.push(Token::Range(ref1, ref2));
                } else if ref1.is_empty() {
                    return Err(TblfmError::Parse(format!(
                        "invalid reference at position {start}"
                    )));
                } else {
                    tokens.push(Token::CellRef(ref1));
                }
            }
            c if c.is_ascii_digit() || (c == '-' && tokens.is_empty()) => {
                // Number (possibly negative at start of expression).
                let start = i;
                if c == '-' {
                    i += 1;
                }
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let n: f64 = num_str
                    .parse()
                    .map_err(|_| TblfmError::Parse(format!("invalid number: {num_str}")))?;
                tokens.push(Token::Number(n));
            }
            '-' => {
                // Unary minus (no previous token or after operator).
                tokens.push(Token::Minus);
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                // Identifier (function name or constant).
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(ident));
            }
            c => {
                return Err(TblfmError::Parse(format!("unexpected character: '{c}'")));
            }
        }
    }

    Ok(tokens)
}

/// Read a cell reference from the character stream.
fn read_cell_ref(chars: &[char], i: &mut usize) -> String {
    let start = *i;
    // May start with @ and/or $.
    if *i < chars.len() && chars[*i] == '@' {
        *i += 1;
        // Row part: digits, >, <, I, V, X, +, -, #
        while *i < chars.len()
            && (chars[*i].is_ascii_alphanumeric()
                || matches!(chars[*i], '>' | '<' | '+' | '-' | '#'))
        {
            *i += 1;
        }
    }
    if *i < chars.len() && chars[*i] == '$' {
        *i += 1;
        // Column part: digits, >, <, alphanumeric, _
        while *i < chars.len()
            && (chars[*i].is_ascii_alphanumeric() || matches!(chars[*i], '>' | '<' | '_' | '#'))
        {
            *i += 1;
        }
    }
    chars[start..*i].iter().collect()
}

// -- Recursive descent parser --

fn parse_additive(tokens: &[Token]) -> Result<(Expr, &[Token]), TblfmError> {
    let (mut left, mut rest) = parse_multiplicative(tokens)?;

    loop {
        match rest.first() {
            Some(Token::Plus) => {
                let (right, new_rest) = parse_multiplicative(&rest[1..])?;
                left = Expr::BinOp(Box::new(left), Op::Add, Box::new(right));
                rest = new_rest;
            }
            Some(Token::Minus) => {
                let (right, new_rest) = parse_multiplicative(&rest[1..])?;
                left = Expr::BinOp(Box::new(left), Op::Sub, Box::new(right));
                rest = new_rest;
            }
            _ => break,
        }
    }

    Ok((left, rest))
}

fn parse_multiplicative(tokens: &[Token]) -> Result<(Expr, &[Token]), TblfmError> {
    let (mut left, mut rest) = parse_unary(tokens)?;

    loop {
        match rest.first() {
            Some(Token::Star) => {
                let (right, new_rest) = parse_unary(&rest[1..])?;
                left = Expr::BinOp(Box::new(left), Op::Mul, Box::new(right));
                rest = new_rest;
            }
            Some(Token::Slash) => {
                let (right, new_rest) = parse_unary(&rest[1..])?;
                left = Expr::BinOp(Box::new(left), Op::Div, Box::new(right));
                rest = new_rest;
            }
            Some(Token::Percent) => {
                let (right, new_rest) = parse_unary(&rest[1..])?;
                left = Expr::BinOp(Box::new(left), Op::Mod, Box::new(right));
                rest = new_rest;
            }
            _ => break,
        }
    }

    Ok((left, rest))
}

fn parse_unary(tokens: &[Token]) -> Result<(Expr, &[Token]), TblfmError> {
    if matches!(tokens.first(), Some(Token::Minus)) {
        let (inner, rest) = parse_atom(&tokens[1..])?;
        return Ok((Expr::Neg(Box::new(inner)), rest));
    }
    parse_atom(tokens)
}

fn parse_atom(tokens: &[Token]) -> Result<(Expr, &[Token]), TblfmError> {
    match tokens.first() {
        Some(Token::Number(n)) => Ok((Expr::Number(*n), &tokens[1..])),
        Some(Token::CellRef(r)) => {
            let (row, col) = parse_cell_ref(r)?;
            match (row, col) {
                (Some(r), Some(c)) => Ok((Expr::CellRef(r, c), &tokens[1..])),
                (None, Some(c)) => {
                    // Column-only ref like $3 — row determined by context.
                    Ok((Expr::CellRef(RowRef::Relative(0), c), &tokens[1..]))
                }
                _ => Err(TblfmError::Parse(format!("incomplete cell reference: {r}"))),
            }
        }
        Some(Token::Range(r1, r2)) => {
            let (sr, sc) = parse_cell_ref(r1)?;
            let (er, ec) = parse_cell_ref(r2)?;
            Ok((
                Expr::Range(Box::new(RangeRef {
                    start_row: sr.unwrap_or(RowRef::First),
                    start_col: sc.unwrap_or(ColRef::First),
                    end_row: er.unwrap_or(RowRef::Last),
                    end_col: ec.unwrap_or(ColRef::Last),
                })),
                &tokens[1..],
            ))
        }
        Some(Token::Ident(name)) => {
            // Check if it's a function call.
            if tokens.len() > 1 && tokens[1] == Token::LParen {
                let func_name = name.clone();
                let mut rest = &tokens[2..]; // skip ident and lparen
                let mut args = Vec::new();

                if rest.first() != Some(&Token::RParen) {
                    let (arg, new_rest) = parse_additive(rest)?;
                    args.push(arg);
                    rest = new_rest;

                    while rest.first() == Some(&Token::Comma) {
                        let (arg, new_rest) = parse_additive(&rest[1..])?;
                        args.push(arg);
                        rest = new_rest;
                    }
                }

                if rest.first() != Some(&Token::RParen) {
                    return Err(TblfmError::Parse(format!(
                        "missing ')' in function call: {func_name}"
                    )));
                }

                // Check for unsupported functions.
                let supported = [
                    "vsum", "vmean", "vmin", "vmax", "vcount", "abs", "round", "ceil", "floor",
                ];
                if !supported.contains(&func_name.as_str()) {
                    return Ok((Expr::Elisp(format!("{func_name}(...)")), &rest[1..]));
                }

                Ok((Expr::FnCall(func_name, args), &rest[1..]))
            } else {
                // Bare identifier — constant name.
                Ok((Expr::Constant(name.clone()), &tokens[1..]))
            }
        }
        Some(Token::LParen) => {
            let (inner, rest) = parse_additive(&tokens[1..])?;
            if rest.first() != Some(&Token::RParen) {
                return Err(TblfmError::Parse("missing ')'".to_string()));
            }
            Ok((inner, &rest[1..]))
        }
        Some(tok) => Err(TblfmError::Parse(format!("unexpected token: {tok:?}"))),
        None => Err(TblfmError::Parse(
            "unexpected end of expression".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

/// Context for evaluating a formula against a table.
pub struct EvalContext<'a> {
    /// The table being evaluated.
    pub table: &'a EvalTable,
    /// Named constants from `#+CONSTANTS:`.
    pub constants: &'a HashMap<String, String>,
    /// Current row index (for `$N` column-only references). `None` for whole-column formulas.
    pub current_row: Option<usize>,
}

/// Evaluate a formula and return the result as a string to write into a cell.
///
/// Returns `Err(TblfmError::RequiresEmacs(_))` if the formula uses unsupported features.
pub fn evaluate(
    assignment: &Assignment,
    table: &EvalTable,
    constants: &HashMap<String, String>,
) -> Result<Vec<(usize, usize, String)>, TblfmError> {
    let mut results = Vec::new();

    match &assignment.target {
        CellTarget::Cell(row_ref, col_ref) => {
            let row = resolve_row(row_ref, table, None)?;
            let col = resolve_col(col_ref, table, constants)?;
            let ctx = EvalContext {
                table,
                constants,
                current_row: Some(row),
            };
            let val = eval_expr(&assignment.expr, &ctx)?;
            results.push((row, col, format_result(val)));
        }
        CellTarget::Column(col_ref) => {
            let col = resolve_col(col_ref, table, constants)?;
            // Apply to each data row, skipping header rows (rows before first separator).
            let first_sep = table.separators.first().copied().unwrap_or(0);
            for (i, row) in table.rows.iter().enumerate() {
                if matches!(row, EvalRow::Data(_)) && i > first_sep {
                    let ctx = EvalContext {
                        table,
                        constants,
                        current_row: Some(i),
                    };
                    let val = eval_expr(&assignment.expr, &ctx)?;
                    results.push((i, col, format_result(val)));
                }
            }
        }
    }

    Ok(results)
}

/// Format a numeric result for display in a table cell.
fn format_result(val: f64) -> String {
    if val == val.floor() && val.abs() < 1e15 {
        format!("{:.0}", val)
    } else {
        // Use up to 2 decimal places, trim trailing zeros.
        let s = format!("{:.2}", val);
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

/// Resolve a row reference to an index into `table.rows`.
fn resolve_row(
    row_ref: &RowRef,
    table: &EvalTable,
    current: Option<usize>,
) -> Result<usize, TblfmError> {
    let data_rows: Vec<usize> = table
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, EvalRow::Data(_)))
        .map(|(i, _)| i)
        .collect();

    match row_ref {
        RowRef::Absolute(n) => {
            if *n == 0 || *n > data_rows.len() {
                Err(TblfmError::Eval(format!(
                    "row @{n} out of range (1..{})",
                    data_rows.len()
                )))
            } else {
                Ok(data_rows[n - 1])
            }
        }
        RowRef::Last => data_rows
            .last()
            .copied()
            .ok_or_else(|| TblfmError::Eval("no data rows".to_string())),
        RowRef::First => data_rows
            .first()
            .copied()
            .ok_or_else(|| TblfmError::Eval("no data rows".to_string())),
        RowRef::HGroup(n) => {
            // @I = first data row after first separator.
            // @II = first data row after second separator.
            if *n == 0 || *n > table.separators.len() {
                return Err(TblfmError::Eval(format!(
                    "hgroup @{} out of range (only {} separators)",
                    "I".repeat(*n),
                    table.separators.len()
                )));
            }
            let sep_idx = table.separators[n - 1];
            // Find first data row after this separator.
            for i in (sep_idx + 1)..table.rows.len() {
                if matches!(table.rows[i], EvalRow::Data(_)) {
                    return Ok(i);
                }
            }
            Err(TblfmError::Eval(format!(
                "no data row after separator @{}",
                "I".repeat(*n)
            )))
        }
        RowRef::Relative(offset) => {
            let base = current.ok_or_else(|| {
                TblfmError::Eval("relative row reference without current row context".to_string())
            })?;
            let target = base as i32 + offset;
            if target < 0 || target as usize >= table.rows.len() {
                Err(TblfmError::Eval(format!(
                    "relative row @{offset:+} out of range from row {base}"
                )))
            } else {
                Ok(target as usize)
            }
        }
    }
}

/// Resolve a column reference to a 0-indexed column number.
fn resolve_col(
    col_ref: &ColRef,
    table: &EvalTable,
    constants: &HashMap<String, String>,
) -> Result<usize, TblfmError> {
    match col_ref {
        ColRef::Absolute(n) => {
            if *n == 0 || *n > table.col_count {
                Err(TblfmError::Eval(format!(
                    "column ${n} out of range (1..{})",
                    table.col_count
                )))
            } else {
                Ok(n - 1) // Convert to 0-indexed.
            }
        }
        ColRef::Last => {
            if table.col_count == 0 {
                Err(TblfmError::Eval("table has no columns".to_string()))
            } else {
                Ok(table.col_count - 1)
            }
        }
        ColRef::First => Ok(0),
        ColRef::Named(name) => {
            // Check constants first.
            if constants.contains_key(name) {
                return Err(TblfmError::Eval(format!(
                    "${name} is a constant, not a column reference"
                )));
            }
            // Try to find column by header name.
            // Look at first data row for column names.
            for row in &table.rows {
                if let EvalRow::Data(cells) = row {
                    for (j, cell) in cells.iter().enumerate() {
                        if let CellValue::Text(txt) = cell {
                            if txt.eq_ignore_ascii_case(name) {
                                return Ok(j);
                            }
                        }
                    }
                    break; // Only check first data row.
                }
            }
            Err(TblfmError::Eval(format!("unknown column name: ${name}")))
        }
    }
}

/// Evaluate an expression in the given context.
fn eval_expr(expr: &Expr, ctx: &EvalContext<'_>) -> Result<f64, TblfmError> {
    match expr {
        Expr::Number(n) => Ok(*n),
        Expr::Elisp(s) => Err(TblfmError::RequiresEmacs(s.clone())),
        Expr::Constant(name) => {
            let val = ctx
                .constants
                .get(name)
                .ok_or_else(|| TblfmError::Eval(format!("unknown constant: {name}")))?;
            val.parse::<f64>()
                .map_err(|_| TblfmError::Eval(format!("constant {name} is not numeric: {val}")))
        }
        Expr::CellRef(row_ref, col_ref) => {
            let row = resolve_row(row_ref, ctx.table, ctx.current_row)?;
            let col = resolve_col(col_ref, ctx.table, ctx.constants)?;
            get_cell_value(ctx.table, row, col)
        }
        Expr::Range(_) => Err(TblfmError::Eval(
            "range used outside of aggregate function".to_string(),
        )),
        Expr::BinOp(left, op, right) => {
            let l = eval_expr(left, ctx)?;
            let r = eval_expr(right, ctx)?;
            match op {
                Op::Add => Ok(l + r),
                Op::Sub => Ok(l - r),
                Op::Mul => Ok(l * r),
                Op::Div => {
                    if r == 0.0 {
                        Err(TblfmError::Eval("division by zero".to_string()))
                    } else {
                        Ok(l / r)
                    }
                }
                Op::Mod => {
                    if r == 0.0 {
                        Err(TblfmError::Eval("modulo by zero".to_string()))
                    } else {
                        Ok(l % r)
                    }
                }
            }
        }
        Expr::Neg(inner) => {
            let v = eval_expr(inner, ctx)?;
            Ok(-v)
        }
        Expr::FnCall(name, args) => eval_function(name, args, ctx),
    }
}

/// Get the numeric value of a cell.
fn get_cell_value(table: &EvalTable, row: usize, col: usize) -> Result<f64, TblfmError> {
    match &table.rows[row] {
        EvalRow::Separator => Err(TblfmError::Eval(format!(
            "cell @{}${} is a separator row",
            row + 1,
            col + 1
        ))),
        EvalRow::Data(cells) => {
            if col >= cells.len() {
                Ok(0.0) // Missing cells treated as 0.
            } else {
                match &cells[col] {
                    CellValue::Number(n) => Ok(*n),
                    CellValue::Empty => Ok(0.0),
                    CellValue::Text(s) => s.parse::<f64>().map_err(|_| {
                        TblfmError::Eval(format!(
                            "cell @{}${} is not numeric: {s}",
                            row + 1,
                            col + 1
                        ))
                    }),
                }
            }
        }
    }
}

/// Collect numeric values from a range.
fn collect_range_values(range: &RangeRef, ctx: &EvalContext<'_>) -> Result<Vec<f64>, TblfmError> {
    let start_row = resolve_row(&range.start_row, ctx.table, ctx.current_row)?;
    let end_row = resolve_row(&range.end_row, ctx.table, ctx.current_row)?;
    let start_col = resolve_col(&range.start_col, ctx.table, ctx.constants)?;
    let end_col = resolve_col(&range.end_col, ctx.table, ctx.constants)?;

    let r_lo = start_row.min(end_row);
    let r_hi = start_row.max(end_row);
    let c_lo = start_col.min(end_col);
    let c_hi = start_col.max(end_col);

    let mut values = Vec::new();
    for r in r_lo..=r_hi {
        if let EvalRow::Data(cells) = &ctx.table.rows[r] {
            for c in c_lo..=c_hi {
                if c < cells.len() {
                    if let Some(n) = cells[c].as_number() {
                        values.push(n);
                    }
                }
            }
        }
    }

    Ok(values)
}

/// Evaluate a function call.
fn eval_function(name: &str, args: &[Expr], ctx: &EvalContext<'_>) -> Result<f64, TblfmError> {
    match name {
        "vsum" => {
            let values = collect_fn_range_values(args, ctx)?;
            Ok(values.iter().sum())
        }
        "vmean" => {
            let values = collect_fn_range_values(args, ctx)?;
            if values.is_empty() {
                Ok(0.0)
            } else {
                Ok(values.iter().sum::<f64>() / values.len() as f64)
            }
        }
        "vmin" => {
            let values = collect_fn_range_values(args, ctx)?;
            values
                .iter()
                .copied()
                .reduce(f64::min)
                .ok_or_else(|| TblfmError::Eval("vmin on empty range".to_string()))
        }
        "vmax" => {
            let values = collect_fn_range_values(args, ctx)?;
            values
                .iter()
                .copied()
                .reduce(f64::max)
                .ok_or_else(|| TblfmError::Eval("vmax on empty range".to_string()))
        }
        "vcount" => {
            let values = collect_fn_range_values(args, ctx)?;
            Ok(values.len() as f64)
        }
        "abs" => {
            if args.len() != 1 {
                return Err(TblfmError::Eval("abs() expects 1 argument".to_string()));
            }
            Ok(eval_expr(&args[0], ctx)?.abs())
        }
        "round" => {
            if args.len() != 1 {
                return Err(TblfmError::Eval("round() expects 1 argument".to_string()));
            }
            Ok(eval_expr(&args[0], ctx)?.round())
        }
        "ceil" => {
            if args.len() != 1 {
                return Err(TblfmError::Eval("ceil() expects 1 argument".to_string()));
            }
            Ok(eval_expr(&args[0], ctx)?.ceil())
        }
        "floor" => {
            if args.len() != 1 {
                return Err(TblfmError::Eval("floor() expects 1 argument".to_string()));
            }
            Ok(eval_expr(&args[0], ctx)?.floor())
        }
        _ => Err(TblfmError::RequiresEmacs(format!(
            "unsupported function: {name}"
        ))),
    }
}

/// Collect range values for aggregate functions.
fn collect_fn_range_values(args: &[Expr], ctx: &EvalContext<'_>) -> Result<Vec<f64>, TblfmError> {
    if args.len() != 1 {
        return Err(TblfmError::Eval(
            "aggregate function expects 1 range argument".to_string(),
        ));
    }
    match &args[0] {
        Expr::Range(range) => collect_range_values(range, ctx),
        // If a single cell ref is passed, return its value.
        other => {
            let val = eval_expr(other, ctx)?;
            Ok(vec![val])
        }
    }
}

// ---------------------------------------------------------------------------
// High-level: find and evaluate all TBLFM in a source string
// ---------------------------------------------------------------------------

/// Result of evaluating all TBLFM formulas in a source file.
pub struct CalcResult {
    /// Updated source content.
    pub content: String,
    /// Number of cells updated.
    pub cells_updated: usize,
    /// Errors encountered (non-fatal — per-formula).
    pub errors: Vec<TblfmError>,
    /// Whether any formula requires Emacs.
    pub requires_emacs: bool,
}

/// Find tables with `#+TBLFM:` lines in the source and evaluate formulas.
///
/// Returns the updated source content with formula results filled in.
pub fn calc_file(content: &str, constants: &HashMap<String, String>) -> CalcResult {
    let mut result_content = content.to_string();
    let mut total_updated = 0;
    let mut errors = Vec::new();
    let mut requires_emacs = false;

    // Find table + TBLFM regions.
    let regions = find_table_tblfm_regions(content);

    // Process in reverse order so byte offsets remain valid.
    for region in regions.into_iter().rev() {
        let table_lines: Vec<&str> = region.table_text.split('\n').collect();
        let mut eval_table = parse_eval_table(&table_lines);

        for tblfm_line in &region.tblfm_lines {
            match parse_tblfm_line(tblfm_line) {
                Ok(assignments) => {
                    for assignment in &assignments {
                        match evaluate(assignment, &eval_table, constants) {
                            Ok(updates) => {
                                for (row, col, value) in updates {
                                    apply_cell_update(&mut eval_table, row, col, &value);
                                    total_updated += 1;
                                }
                            }
                            Err(TblfmError::RequiresEmacs(msg)) => {
                                requires_emacs = true;
                                errors.push(TblfmError::RequiresEmacs(msg));
                            }
                            Err(e) => errors.push(e),
                        }
                    }
                }
                Err(e) => errors.push(e),
            }
        }

        // Rebuild table text from eval_table and replace in source.
        let new_table = render_eval_table(&eval_table);
        result_content.replace_range(region.table_start..region.table_end, &new_table);
    }

    CalcResult {
        content: result_content,
        cells_updated: total_updated,
        errors,
        requires_emacs,
    }
}

/// A table region with its TBLFM lines.
struct TableTblfmRegion {
    table_start: usize,
    table_end: usize,
    table_text: String,
    tblfm_lines: Vec<String>,
}

/// Find all table regions followed by `#+TBLFM:` lines.
fn find_table_tblfm_regions(content: &str) -> Vec<TableTblfmRegion> {
    let mut regions = Vec::new();
    let lines: Vec<&str> = content.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
        if raw.trim_start().starts_with('|') {
            let table_start_line = i;
            let mut table_lines = Vec::new();

            while i < lines.len() {
                let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
                if raw.trim_start().starts_with('|') {
                    table_lines.push(raw);
                    i += 1;
                } else {
                    break;
                }
            }

            // Check for TBLFM lines.
            let mut tblfm_lines = Vec::new();
            while i < lines.len() {
                let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
                let trimmed = raw.trim();
                if let Some(rest) = strip_tblfm_prefix(trimmed) {
                    tblfm_lines.push(rest.to_string());
                    i += 1;
                } else {
                    break;
                }
            }

            if !tblfm_lines.is_empty() {
                let table_start: usize =
                    lines[..table_start_line].iter().map(|l| l.len() + 1).sum();
                let table_end: usize = lines[..table_start_line + table_lines.len()]
                    .iter()
                    .map(|l| l.len() + 1)
                    .sum();
                let table_text = table_lines.join("\n");

                regions.push(TableTblfmRegion {
                    table_start,
                    table_end,
                    table_text,
                    tblfm_lines,
                });
            }
        } else {
            i += 1;
        }
    }

    regions
}

/// Strip `#+TBLFM:` prefix (case-insensitive).
fn strip_tblfm_prefix(line: &str) -> Option<&str> {
    let lower = line.to_lowercase();
    if lower.starts_with("#+tblfm:") {
        Some(&line[8..])
    } else {
        None
    }
}

/// Apply a cell update to the eval table.
fn apply_cell_update(table: &mut EvalTable, row: usize, col: usize, value: &str) {
    if let EvalRow::Data(cells) = &mut table.rows[row] {
        // Extend cells if needed.
        while cells.len() <= col {
            cells.push(CellValue::Empty);
        }
        if let Some(n) = parse_number(value) {
            cells[col] = CellValue::Number(n);
        } else {
            cells[col] = CellValue::Text(value.to_string());
        }
    }
}

/// Render the eval table back to org-mode table text.
fn render_eval_table(table: &EvalTable) -> String {
    let mut lines = Vec::new();

    // Compute column widths.
    let mut widths = vec![0usize; table.col_count];
    for row in &table.rows {
        if let EvalRow::Data(cells) = row {
            for (j, cell) in cells.iter().enumerate() {
                if j < widths.len() {
                    let w = cell_display(cell).len();
                    widths[j] = widths[j].max(w);
                }
            }
        }
    }
    // Minimum width of 1.
    for w in &mut widths {
        if *w == 0 {
            *w = 1;
        }
    }

    for row in &table.rows {
        match row {
            EvalRow::Separator => {
                let parts: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
                lines.push(format!("|{}|", parts.join("+")));
            }
            EvalRow::Data(cells) => {
                let mut parts = Vec::new();
                for (j, w) in widths.iter().enumerate() {
                    let text = if j < cells.len() {
                        cell_display(&cells[j])
                    } else {
                        String::new()
                    };
                    // Right-align numbers, left-align text.
                    let padded = if j < cells.len() && cells[j].as_number().is_some() {
                        format!("{:>width$}", text, width = *w)
                    } else {
                        format!("{:<width$}", text, width = *w)
                    };
                    parts.push(format!(" {} ", padded));
                }
                lines.push(format!("|{}|", parts.join("|")));
            }
        }
    }

    lines.join("\n")
}

/// Display a cell value as a string.
fn cell_display(cell: &CellValue) -> String {
    match cell {
        CellValue::Number(n) => format_result(*n),
        CellValue::Text(s) => s.clone(),
        CellValue::Empty => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_reference() {
        let (row, col) = parse_cell_ref("@2$3").unwrap();
        assert_eq!(row, Some(RowRef::Absolute(2)));
        assert_eq!(col, Some(ColRef::Absolute(3)));
    }

    #[test]
    fn parse_special_refs() {
        let (row, col) = parse_cell_ref("@>$>").unwrap();
        assert_eq!(row, Some(RowRef::Last));
        assert_eq!(col, Some(ColRef::Last));
    }

    #[test]
    fn parse_column_only() {
        let (row, col) = parse_cell_ref("$3").unwrap();
        assert_eq!(row, None);
        assert_eq!(col, Some(ColRef::Absolute(3)));
    }

    #[test]
    fn parse_hgroup_ref() {
        let (row, col) = parse_cell_ref("@I$2").unwrap();
        assert_eq!(row, Some(RowRef::HGroup(1)));
        assert_eq!(col, Some(ColRef::Absolute(2)));

        let (row, _) = parse_cell_ref("@II$1").unwrap();
        assert_eq!(row, Some(RowRef::HGroup(2)));
    }

    #[test]
    fn parse_named_col() {
        let (_, col) = parse_cell_ref("$total").unwrap();
        assert_eq!(col, Some(ColRef::Named("total".to_string())));
    }

    #[test]
    fn parse_roman() {
        assert_eq!(super::parse_roman("I"), 1);
        assert_eq!(super::parse_roman("II"), 2);
        assert_eq!(super::parse_roman("III"), 3);
        assert_eq!(super::parse_roman("IV"), 4);
        assert_eq!(super::parse_roman("V"), 5);
    }

    #[test]
    fn parse_simple_expr() {
        let expr = parse_expr("$1+$2").unwrap();
        match expr {
            Expr::BinOp(_, Op::Add, _) => {}
            other => panic!("expected BinOp Add, got {other:?}"),
        }
    }

    #[test]
    fn parse_multiplication() {
        let expr = parse_expr("$2*$3").unwrap();
        match expr {
            Expr::BinOp(_, Op::Mul, _) => {}
            other => panic!("expected BinOp Mul, got {other:?}"),
        }
    }

    #[test]
    fn parse_precedence() {
        // $1 + $2 * $3 should parse as $1 + ($2 * $3)
        let expr = parse_expr("$1+$2*$3").unwrap();
        match expr {
            Expr::BinOp(_, Op::Add, right) => {
                assert!(matches!(*right, Expr::BinOp(_, Op::Mul, _)));
            }
            other => panic!("expected Add at top, got {other:?}"),
        }
    }

    #[test]
    fn parse_function_call() {
        let expr = parse_expr("vsum($1..$3)").unwrap();
        match expr {
            Expr::FnCall(name, args) => {
                assert_eq!(name, "vsum");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected FnCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_elisp_detected() {
        let expr = parse_expr("lisp:(format \"%s\" val)").unwrap();
        assert!(matches!(expr, Expr::Elisp(_)));
    }

    #[test]
    fn parse_unsupported_fn() {
        let expr = parse_expr("sqrt($1)").unwrap();
        assert!(matches!(expr, Expr::Elisp(_)));
    }

    #[test]
    fn parse_tblfm_assignment() {
        let assignments = parse_tblfm_line("$4=$2*$3").unwrap();
        assert_eq!(assignments.len(), 1);
        assert!(matches!(
            assignments[0].target,
            CellTarget::Column(ColRef::Absolute(4))
        ));
    }

    #[test]
    fn parse_tblfm_multiple() {
        let assignments = parse_tblfm_line("$4=$2*$3::@>$4=vsum(@I$4..@II$4)").unwrap();
        assert_eq!(assignments.len(), 2);
    }

    #[test]
    fn parse_tblfm_with_format_spec() {
        let assignments = parse_tblfm_line("$4=$2*$3;%.2f").unwrap();
        assert_eq!(assignments.len(), 1);
    }

    #[test]
    fn eval_simple_table() {
        let table = parse_eval_table(&[
            "| Item  | Price | Qty | Total |",
            "|-------+-------+-----+-------|",
            "| Apple |  1.50 |   3 |       |",
            "| Pear  |  2.00 |   5 |       |",
        ]);

        let constants = HashMap::new();
        let assignments = parse_tblfm_line("$4=$2*$3").unwrap();

        let updates = evaluate(&assignments[0], &table, &constants).unwrap();
        // Should update rows 2 and 3 (the data rows after separator).
        assert_eq!(updates.len(), 2); // Two data rows below separator get $4.

        // First data row: 1.5 * 3 = 4.5
        let (_, col, val) = &updates[0];
        assert_eq!(*col, 3);
        assert_eq!(val, "4.5");

        // Second data row: 2.0 * 5 = 10
        let (_, col, val) = &updates[1];
        assert_eq!(*col, 3);
        assert_eq!(val, "10");
    }

    #[test]
    fn eval_vsum_range() {
        let table = parse_eval_table(&[
            "| A |  B |",
            "|---+----|",
            "| x | 10 |",
            "| y | 20 |",
            "| z | 30 |",
            "|---+----|",
            "|   |    |",
        ]);

        let constants = HashMap::new();
        let assignments = parse_tblfm_line("@>$2=vsum(@I$2..@II$2)").unwrap();

        let updates = evaluate(&assignments[0], &table, &constants).unwrap();
        assert_eq!(updates.len(), 1);
        let (_, _, val) = &updates[0];
        assert_eq!(val, "60");
    }

    #[test]
    fn eval_with_constant() {
        let table = parse_eval_table(&[
            "| Radius | Area |",
            "|--------+------|",
            "|      5 |      |",
        ]);

        let mut constants = HashMap::new();
        constants.insert("pi".to_string(), "3.14159".to_string());

        let assignments = parse_tblfm_line("$2=$1*$1*pi").unwrap();
        let updates = evaluate(&assignments[0], &table, &constants).unwrap();
        assert_eq!(updates.len(), 1);
        let (_, _, val) = &updates[0];
        // 5 * 5 * 3.14159 = 78.53975
        let n: f64 = val.parse().unwrap();
        assert!((n - 78.54).abs() < 0.01);
    }

    #[test]
    fn eval_elisp_returns_error() {
        let table = parse_eval_table(&["| A | B |", "|---+---|", "| 1 | 2 |"]);

        let constants = HashMap::new();
        let assignments = parse_tblfm_line("$2=lisp:(+ 1 2)").unwrap();
        let result = evaluate(&assignments[0], &table, &constants);
        assert!(matches!(result, Err(TblfmError::RequiresEmacs(_))));
    }

    #[test]
    fn eval_division_by_zero() {
        let table = parse_eval_table(&["| A | B |", "|---+---|", "| 1 | 0 |"]);

        let constants = HashMap::new();
        let assignments = parse_tblfm_line("$2=$1/$2").unwrap();
        let result = evaluate(&assignments[0], &table, &constants);
        assert!(matches!(result, Err(TblfmError::Eval(_))));
    }

    #[test]
    fn calc_file_end_to_end() {
        let content = "\
| Item  | Price | Qty | Total |
|-------+-------+-----+-------|
| Apple |  1.50 |   3 |       |
| Pear  |  2.00 |   5 |       |
#+TBLFM: $4=$2*$3
";
        let constants = HashMap::new();
        let result = calc_file(content, &constants);
        assert_eq!(result.cells_updated, 2);
        assert!(!result.requires_emacs);
        assert!(result.content.contains("4.5"));
        assert!(result.content.contains("10"));
    }

    #[test]
    fn calc_file_vsum_end_to_end() {
        let content = "\
| Item  | Price | Qty | Total |
|-------+-------+-----+-------|
| Apple |  1.50 |   3 |       |
| Pear  |  2.00 |   5 |       |
|-------+-------+-----+-------|
| Sum   |       |     |       |
#+TBLFM: $4=$2*$3::@>$4=vsum(@I$4..@II$4)
";
        let constants = HashMap::new();
        let result = calc_file(content, &constants);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(result.content.contains("4.5"));
        assert!(result.content.contains("10"));
        // Sum should be 14.5
        assert!(result.content.contains("14.5"));
    }

    #[test]
    fn format_result_integer() {
        assert_eq!(format_result(42.0), "42");
        assert_eq!(format_result(0.0), "0");
    }

    #[test]
    fn format_result_decimal() {
        assert_eq!(format_result(4.5), "4.5");
        assert_eq!(format_result(3.14), "3.14");
    }

    #[test]
    fn eval_vmean() {
        let table = parse_eval_table(&[
            "| A |", "|---|", "| 10 |", "| 20 |", "| 30 |", "|---|", "|   |",
        ]);
        let constants = HashMap::new();
        let assignments = parse_tblfm_line("@>$1=vmean(@I$1..@II$1)").unwrap();
        let updates = evaluate(&assignments[0], &table, &constants).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].2, "20");
    }

    #[test]
    fn eval_abs_and_round() {
        let table = parse_eval_table(&["| A | B |", "|---+---|", "| -3.7 |   |"]);
        let constants = HashMap::new();

        let assignments = parse_tblfm_line("$2=round(abs($1))").unwrap();
        let updates = evaluate(&assignments[0], &table, &constants).unwrap();
        assert_eq!(updates[0].2, "4");
    }
}
