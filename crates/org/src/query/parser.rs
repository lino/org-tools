// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=870 reason="Pre-existing cohesive query language AST, tokenizer, recursive-descent parser, and behavioural unit tests"

//! Query language parser for org-mode entry matching.
//!
//! Supports a plain-text query syntax:
//! - `todo:TODO` — match specific TODO keyword
//! - `tags:work,urgent` — match entries with all listed tags
//! - `heading:meeting` — substring match on heading title
//! - `body:"Herzmaschine"` — full-text BM25 phrase/term match across title and body
//! - `property:KEY=VALUE` — property match
//! - `priority:A` or `priority:>=B` — priority match
//! - `level:2` or `level:<=3` — heading level match
//! - `scheduled:<=today` — date predicates
//! - `deadline:past`, `deadline:<=+7d`
//! - `clocked` — has any clock entries
//! - `done` — DONE keyword
//! - Boolean: `and`, `or`, `not`, parentheses
//! - Implicit AND between terms

use std::fmt;

// region Public Types

/// A parsed query predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// Match entries with any TODO keyword, or a specific one.
    Todo(Option<String>),
    /// Match entries with a DONE keyword.
    Done,
    /// Match entries with all specified tags.
    Tags(Vec<String>),
    /// Substring match on heading title (case-insensitive).
    Heading(String),
    /// Full-text BM25 phrase/term match across heading title and body.
    Body(String),
    /// Match entries with a property key/value pair.
    Property {
        /// Property key.
        key: String,
        /// Property value to match.
        value: String,
    },
    /// Match entries by priority.
    Priority(PriorityMatch),
    /// Match entries by heading level.
    Level(Comparison),
    /// Match entries with a SCHEDULED timestamp.
    Scheduled(DateMatch),
    /// Match entries with a DEADLINE timestamp.
    Deadline(DateMatch),
    /// Match entries with a CLOSED timestamp.
    Closed(DateMatch),
    /// Match entries that have any clock entries.
    Clocked,
    /// Match entries blocked by org-edna `:BLOCKER:` dependencies.
    Blocked,
    /// Match actionable entries: TODO keyword and not blocked by edna.
    Actionable,
    /// Match entries in a waiting state (keyword contains "WAIT" or has `:WAITING_FOR:` property).
    Waiting,
    /// Logical AND of predicates.
    And(Vec<Predicate>),
    /// Logical OR of predicates.
    Or(Vec<Predicate>),
    /// Logical NOT.
    Not(Box<Predicate>),
}

/// Priority matching with optional comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum PriorityMatch {
    /// Exact match: `priority:A`.
    Exact(char),
    /// Comparison: `priority:>=B`.
    Cmp(CmpOp, char),
}

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    /// `<`.
    Lt,
    /// `<=`.
    Lte,
    /// `>`.
    Gt,
    /// `>=`.
    Gte,
    /// `=`.
    Eq,
}

/// Numeric comparison for levels.
#[derive(Debug, Clone, PartialEq)]
pub enum Comparison {
    /// Exact match: `level:2`.
    Eq(usize),
    /// Less than: `level:<3`.
    Lt(usize),
    /// Less than or equal: `level:<=3`.
    Lte(usize),
    /// Greater than: `level:>1`.
    Gt(usize),
    /// Greater than or equal: `level:>=2`.
    Gte(usize),
}

/// Date matching for planning timestamps.
#[derive(Debug, Clone, PartialEq)]
pub enum DateMatch {
    /// Any timestamp present.
    Any,
    /// `today`.
    Today,
    /// `past` — before today.
    Past,
    /// `future` — after today.
    Future,
    /// `<=today`, `>=today`, etc.
    Cmp(CmpOp, DateRef),
}

/// A date reference (absolute or relative).
#[derive(Debug, Clone, PartialEq)]
pub enum DateRef {
    /// `today`.
    Today,
    /// Relative: `+7d`, `-3d`, `+2w`.
    Relative(i64, DateUnit),
    /// Absolute: `2024-01-15`.
    Absolute(u16, u8, u8),
}

/// Unit for relative date references.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DateUnit {
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
}

/// Parse error.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    /// Formats this query parse error for human display.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "query parse error: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

impl Predicate {
    /// Returns `true` when `Predicate::Body` appears anywhere inside this AST.
    pub fn uses_body(&self) -> bool {
        match self {
            Self::Body(_) => true,
            Self::And(ps) | Self::Or(ps) => ps.iter().any(Self::uses_body),
            Self::Not(inner) => inner.uses_body(),
            _ => false,
        }
    }

    /// Returns `true` when `Blocked` or `Actionable` appears anywhere inside this AST.
    pub fn uses_edna(&self) -> bool {
        match self {
            Self::Blocked | Self::Actionable => true,
            Self::And(ps) | Self::Or(ps) => ps.iter().any(Self::uses_edna),
            Self::Not(inner) => inner.uses_edna(),
            _ => false,
        }
    }

    /// Collects positive `Body` phrases that MUST hold on every matching entry (`Body` / `And`).
    pub fn required_body_phrases(&self) -> Vec<String> {
        match self {
            Self::Body(s) => vec![s.clone()],
            Self::And(ps) => ps.iter().flat_map(Self::required_body_phrases).collect(),
            _ => Vec::new(),
        }
    }

    /// Collects all positive (non-negated) `Body` phrases used for BM25 relevance scoring.
    pub fn positive_body_phrases(&self) -> Vec<String> {
        match self {
            Self::Body(s) => vec![s.clone()],
            Self::And(ps) | Self::Or(ps) => {
                ps.iter().flat_map(Self::positive_body_phrases).collect()
            }
            _ => Vec::new(),
        }
    }
}

// endregion

// region Tokenizer

/// Parse a query string into a [`Predicate`].
pub fn parse_query(input: &str) -> Result<Predicate, ParseError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(ParseError("empty query".to_string()));
    }
    let (pred, rest) = parse_or(&tokens)?;
    if !rest.is_empty() {
        return Err(ParseError(format!("unexpected token: {:?}", rest[0])));
    }
    Ok(pred)
}

/// Lexical token emitted by the query tokenizer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A `key:value` term or unquoted bare keyword.
    Term(String),
    /// A top-level double-quoted bare heading phrase (`"..."`).
    QuotedHeading(String),
    /// `and`.
    And,
    /// `or`.
    Or,
    /// `not`.
    Not,
    /// `(`.
    LParen,
    /// `)`.
    RParen,
}

/// Consumes a double-quoted segment (`"..."`) after the opening `"`, handling `\"` and `\\`.
fn read_quoted_segment(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
) -> Result<(), ParseError> {
    while let Some(ch) = chars.next() {
        if ch == '"' {
            return Ok(());
        }
        if ch == '\\' {
            if let Some(esc) = chars.next() {
                out.push(esc);
            } else {
                return Err(ParseError("unterminated quoted string".to_string()));
            }
        } else {
            out.push(ch);
        }
    }
    Err(ParseError("unterminated quoted string".to_string()))
}

/// Consumes one bare word or `key:"quoted"` term from `chars`.
fn read_word_token(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<Token, ParseError> {
    let mut word = String::new();
    let mut had_quotes = false;
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || matches!(c, '(' | ')') {
            break;
        }
        chars.next();
        if c == '"' {
            had_quotes = true;
            read_quoted_segment(chars, &mut word)?;
        } else {
            word.push(c);
        }
    }
    Ok(match (had_quotes, word.as_str()) {
        (false, "and" | "AND") => Token::And,
        (false, "or" | "OR") => Token::Or,
        (false, "not" | "NOT") => Token::Not,
        _ => Token::Term(word),
    })
}

/// Tokenizes `input` into [`Token`] values while preserving double-quoted phrases.
fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else if ch == '(' {
            chars.next();
            tokens.push(Token::LParen);
        } else if ch == ')' {
            chars.next();
            tokens.push(Token::RParen);
        } else if ch == '"' {
            chars.next();
            let mut phrase = String::new();
            read_quoted_segment(&mut chars, &mut phrase)?;
            tokens.push(Token::QuotedHeading(phrase));
        } else {
            tokens.push(read_word_token(&mut chars)?);
        }
    }
    Ok(tokens)
}

// endregion

// region Recursive-Descent Parser

/// Parse OR expressions: `A or B or C`.
fn parse_or(tokens: &[Token]) -> Result<(Predicate, &[Token]), ParseError> {
    let (mut left, mut rest) = parse_and(tokens)?;

    while rest.first() == Some(&Token::Or) {
        let (right, new_rest) = parse_and(&rest[1..])?;
        left = match left {
            Predicate::Or(mut v) => {
                v.push(right);
                Predicate::Or(v)
            }
            _ => Predicate::Or(vec![left, right]),
        };
        rest = new_rest;
    }

    Ok((left, rest))
}

/// Parse AND expressions: `A and B` or implicit `A B`.
fn parse_and(tokens: &[Token]) -> Result<(Predicate, &[Token]), ParseError> {
    let (mut left, mut rest) = parse_not(tokens)?;

    loop {
        // Explicit `and`.
        if rest.first() == Some(&Token::And) {
            let (right, new_rest) = parse_not(&rest[1..])?;
            left = match left {
                Predicate::And(mut v) => {
                    v.push(right);
                    Predicate::And(v)
                }
                _ => Predicate::And(vec![left, right]),
            };
            rest = new_rest;
            continue;
        }

        // Implicit AND: next token is a term, not, or lparen (not or/rparen/end).
        match rest.first() {
            Some(Token::Term(_) | Token::QuotedHeading(_) | Token::Not | Token::LParen) => {
                let (right, new_rest) = parse_not(rest)?;
                left = match left {
                    Predicate::And(mut v) => {
                        v.push(right);
                        Predicate::And(v)
                    }
                    _ => Predicate::And(vec![left, right]),
                };
                rest = new_rest;
            }
            _ => break,
        }
    }

    Ok((left, rest))
}

/// Parse NOT: `not X`.
fn parse_not(tokens: &[Token]) -> Result<(Predicate, &[Token]), ParseError> {
    if tokens.first() == Some(&Token::Not) {
        let (inner, rest) = parse_atom(&tokens[1..])?;
        return Ok((Predicate::Not(Box::new(inner)), rest));
    }
    parse_atom(tokens)
}

/// Parse an atom: a parenthesized expression or a term.
fn parse_atom(tokens: &[Token]) -> Result<(Predicate, &[Token]), ParseError> {
    match tokens.first() {
        Some(Token::LParen) => {
            let (inner, rest) = parse_or(&tokens[1..])?;
            if rest.first() != Some(&Token::RParen) {
                return Err(ParseError("missing closing parenthesis".to_string()));
            }
            Ok((inner, &rest[1..]))
        }
        Some(Token::Term(term)) => {
            let pred = parse_term(term)?;
            Ok((pred, &tokens[1..]))
        }
        Some(Token::QuotedHeading(phrase)) => {
            if phrase.is_empty() {
                return Err(ParseError("empty heading value".to_string()));
            }
            Ok((Predicate::Heading(phrase.clone()), &tokens[1..]))
        }
        Some(tok) => Err(ParseError(format!("unexpected token: {tok:?}"))),
        None => Err(ParseError("unexpected end of query".to_string())),
    }
}

// endregion

// region Term Parsers

/// Parses a comma-separated `tags:a,b` value into [`Predicate::Tags`].
fn parse_tags_term(value: &str) -> Result<Predicate, ParseError> {
    let tags: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if tags.is_empty() {
        Err(ParseError("empty tags value".to_string()))
    } else {
        Ok(Predicate::Tags(tags))
    }
}

/// Parses a `scheduled:`, `deadline:`, or `closed:` planning predicate term.
fn parse_planning_term(key: &str, value: &str) -> Result<Predicate, ParseError> {
    match key {
        "scheduled" | "sched" => Ok(Predicate::Scheduled(parse_date_match(value)?)),
        "deadline" | "dl" => Ok(Predicate::Deadline(parse_date_match(value)?)),
        "closed" => Ok(Predicate::Closed(parse_date_match(value)?)),
        _ => Err(ParseError(format!("unknown predicate: {key}"))),
    }
}

/// Validates a required text argument (`heading:` or `body:`) and wraps it via `ctor`.
fn parse_non_empty_text(
    value: &str,
    label: &str,
    ctor: fn(String) -> Predicate,
) -> Result<Predicate, ParseError> {
    if value.is_empty() {
        Err(ParseError(format!("empty {label} value")))
    } else {
        Ok(ctor(value.to_string()))
    }
}

/// Parses a `key:value` predicate term after splitting on the first `:` separator.
fn parse_key_value_term(key: &str, value: &str) -> Result<Predicate, ParseError> {
    match key {
        "todo" => Ok(Predicate::Todo(
            (!value.is_empty()).then(|| value.to_string()),
        )),
        "tags" | "tag" => parse_tags_term(value),
        "heading" | "h" => parse_non_empty_text(value, "heading", Predicate::Heading),
        "body" | "b" => parse_non_empty_text(value, "body", Predicate::Body),
        "property" | "prop" => parse_property_term(value),
        "priority" | "pri" => parse_priority_term(value),
        "level" | "lvl" => parse_level_term(value),
        other => parse_planning_term(other, value),
    }
}

/// Parses a single term like `todo:TODO`, `body:"Herzmaschine"`, `done`, or `clocked`.
fn parse_term(term: &str) -> Result<Predicate, ParseError> {
    match term {
        "done" | "DONE" => return Ok(Predicate::Done),
        "clocked" => return Ok(Predicate::Clocked),
        "todo" | "TODO" => return Ok(Predicate::Todo(None)),
        "blocked" => return Ok(Predicate::Blocked),
        "actionable" => return Ok(Predicate::Actionable),
        "waiting" | "WAITING" => return Ok(Predicate::Waiting),
        _ => {}
    }

    if let Some((key, value)) = term.split_once(':') {
        parse_key_value_term(key, value)
    } else {
        Ok(Predicate::Heading(term.to_string()))
    }
}

/// Parses `property:KEY` or `property:KEY=VALUE`.
fn parse_property_term(value: &str) -> Result<Predicate, ParseError> {
    if let Some(eq_pos) = value.find('=') {
        let key = value[..eq_pos].to_string();
        let val = value[eq_pos + 1..].to_string();
        if key.is_empty() {
            return Err(ParseError("empty property key".to_string()));
        }
        Ok(Predicate::Property { key, value: val })
    } else {
        // Property existence check (value = any).
        Ok(Predicate::Property {
            key: value.to_string(),
            value: String::new(),
        })
    }
}

/// Parses `priority:A` or `priority:>=B`.
fn parse_priority_term(value: &str) -> Result<Predicate, ParseError> {
    if value.len() == 1 && value.chars().next().unwrap().is_ascii_alphabetic() {
        return Ok(Predicate::Priority(PriorityMatch::Exact(
            value.chars().next().unwrap().to_ascii_uppercase(),
        )));
    }

    // Comparison: >=B, <=A, >C, <B, =A
    let (op, rest) = parse_cmp_op(value)?;
    if rest.len() == 1 && rest.chars().next().unwrap().is_ascii_alphabetic() {
        Ok(Predicate::Priority(PriorityMatch::Cmp(
            op,
            rest.chars().next().unwrap().to_ascii_uppercase(),
        )))
    } else {
        Err(ParseError(format!("invalid priority: {value}")))
    }
}

/// Parses `level:2` or `level:<=3`.
fn parse_level_term(value: &str) -> Result<Predicate, ParseError> {
    // Try exact number first.
    if let Ok(n) = value.parse::<usize>() {
        return Ok(Predicate::Level(Comparison::Eq(n)));
    }

    let (op, rest) = parse_cmp_op(value)?;
    let n: usize = rest
        .parse()
        .map_err(|_| ParseError(format!("invalid level number: {rest}")))?;
    Ok(Predicate::Level(match op {
        CmpOp::Lt => Comparison::Lt(n),
        CmpOp::Lte => Comparison::Lte(n),
        CmpOp::Gt => Comparison::Gt(n),
        CmpOp::Gte => Comparison::Gte(n),
        CmpOp::Eq => Comparison::Eq(n),
    }))
}

/// Parses a `scheduled:`, `deadline:`, or `closed:` date value.
fn parse_date_match(value: &str) -> Result<DateMatch, ParseError> {
    match value {
        "" => Ok(DateMatch::Any),
        "today" => Ok(DateMatch::Today),
        "past" => Ok(DateMatch::Past),
        "future" => Ok(DateMatch::Future),
        _ => {
            // Try comparison: <=today, >=+7d, etc.
            if let Ok((op, rest)) = parse_cmp_op(value) {
                let date_ref = parse_date_ref(rest)?;
                return Ok(DateMatch::Cmp(op, date_ref));
            }
            // Try bare date ref: +7d, 2024-01-15.
            let date_ref = parse_date_ref(value)?;
            Ok(DateMatch::Cmp(CmpOp::Eq, date_ref))
        }
    }
}

/// Parses a relative (`+7d`) or absolute (`YYYY-MM-DD`) date reference.
fn parse_date_ref(value: &str) -> Result<DateRef, ParseError> {
    if value == "today" {
        return Ok(DateRef::Today);
    }

    // Relative: +7d, -3d, +2w, +1m.
    if value.starts_with('+') || value.starts_with('-') {
        let sign: i64 = if value.starts_with('-') { -1 } else { 1 };
        let rest = &value[1..];
        if rest.is_empty() {
            return Err(ParseError(format!("invalid date ref: {value}")));
        }
        let unit_char = rest.as_bytes()[rest.len() - 1];
        let number: &str = &rest[..rest.len() - 1];
        let n: i64 = number
            .parse()
            .map_err(|_| ParseError(format!("invalid date number: {number}")))?;
        let unit = match unit_char {
            b'd' => DateUnit::Day,
            b'w' => DateUnit::Week,
            b'm' => DateUnit::Month,
            _ => return Err(ParseError(format!("invalid date unit: {value}"))),
        };
        return Ok(DateRef::Relative(sign * n, unit));
    }

    // Absolute: YYYY-MM-DD.
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() == 3 {
        let year: u16 = parts[0]
            .parse()
            .map_err(|_| ParseError(format!("invalid year: {}", parts[0])))?;
        let month: u8 = parts[1]
            .parse()
            .map_err(|_| ParseError(format!("invalid month: {}", parts[1])))?;
        let day: u8 = parts[2]
            .parse()
            .map_err(|_| ParseError(format!("invalid day: {}", parts[2])))?;
        return Ok(DateRef::Absolute(year, month, day));
    }

    Err(ParseError(format!("invalid date reference: {value}")))
}

/// Splits a leading comparison operator (`<=`, `>=`, `<`, `>`, `=`) from `value`.
fn parse_cmp_op(value: &str) -> Result<(CmpOp, &str), ParseError> {
    if let Some(rest) = value.strip_prefix("<=") {
        Ok((CmpOp::Lte, rest))
    } else if let Some(rest) = value.strip_prefix(">=") {
        Ok((CmpOp::Gte, rest))
    } else if let Some(rest) = value.strip_prefix('<') {
        Ok((CmpOp::Lt, rest))
    } else if let Some(rest) = value.strip_prefix('>') {
        Ok((CmpOp::Gt, rest))
    } else if let Some(rest) = value.strip_prefix('=') {
        Ok((CmpOp::Eq, rest))
    } else {
        Err(ParseError(format!(
            "expected comparison operator in: {value}"
        )))
    }
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies bare `todo` parses as `Predicate::Todo(None)`.
    #[test]
    fn parse_bare_todo() {
        let p = parse_query("todo").unwrap();
        assert_eq!(p, Predicate::Todo(None));
    }

    /// Verifies `todo:TODO` parses as a specific keyword predicate.
    #[test]
    fn parse_todo_keyword() {
        let p = parse_query("todo:TODO").unwrap();
        assert_eq!(p, Predicate::Todo(Some("TODO".to_string())));
    }

    /// Verifies bare `done` parses as `Predicate::Done`.
    #[test]
    fn parse_done() {
        let p = parse_query("done").unwrap();
        assert_eq!(p, Predicate::Done);
    }

    /// Verifies comma-separated `tags:` parses into `Predicate::Tags`.
    #[test]
    fn parse_tags() {
        let p = parse_query("tags:work,urgent").unwrap();
        assert_eq!(
            p,
            Predicate::Tags(vec!["work".to_string(), "urgent".to_string()])
        );
    }

    /// Verifies `heading:` parses into `Predicate::Heading`.
    #[test]
    fn parse_heading() {
        let p = parse_query("heading:meeting").unwrap();
        assert_eq!(p, Predicate::Heading("meeting".to_string()));
    }

    /// Verifies `property:KEY=VALUE` parses into `Predicate::Property`.
    #[test]
    fn parse_property() {
        let p = parse_query("property:CATEGORY=project").unwrap();
        assert_eq!(
            p,
            Predicate::Property {
                key: "CATEGORY".to_string(),
                value: "project".to_string(),
            }
        );
    }

    /// Verifies `priority:A` parses as an exact priority predicate.
    #[test]
    fn parse_priority_exact() {
        let p = parse_query("priority:A").unwrap();
        assert_eq!(p, Predicate::Priority(PriorityMatch::Exact('A')));
    }

    /// Verifies `priority:>=B` parses as a relational priority predicate.
    #[test]
    fn parse_priority_comparison() {
        let p = parse_query("priority:>=B").unwrap();
        assert_eq!(p, Predicate::Priority(PriorityMatch::Cmp(CmpOp::Gte, 'B')));
    }

    /// Verifies exact and relational `level:` predicates.
    #[test]
    fn parse_level() {
        assert_eq!(
            parse_query("level:2").unwrap(),
            Predicate::Level(Comparison::Eq(2))
        );
        assert_eq!(
            parse_query("level:<=3").unwrap(),
            Predicate::Level(Comparison::Lte(3))
        );
    }

    /// Verifies `scheduled:<=today` parses into `Predicate::Scheduled`.
    #[test]
    fn parse_scheduled() {
        let p = parse_query("scheduled:<=today").unwrap();
        assert_eq!(
            p,
            Predicate::Scheduled(DateMatch::Cmp(CmpOp::Lte, DateRef::Today))
        );
    }

    /// Verifies relative `+7d` deadline expressions parse cleanly.
    #[test]
    fn parse_deadline_relative() {
        let p = parse_query("deadline:<=+7d").unwrap();
        assert_eq!(
            p,
            Predicate::Deadline(DateMatch::Cmp(
                CmpOp::Lte,
                DateRef::Relative(7, DateUnit::Day)
            ))
        );
    }

    /// Verifies bare `clocked` parses as `Predicate::Clocked`.
    #[test]
    fn parse_clocked() {
        let p = parse_query("clocked").unwrap();
        assert_eq!(p, Predicate::Clocked);
    }

    /// Verifies adjacent terms combine via implicit logical `And`.
    #[test]
    fn parse_implicit_and() {
        let p = parse_query("todo:TODO tags:work").unwrap();
        assert_eq!(
            p,
            Predicate::And(vec![
                Predicate::Todo(Some("TODO".to_string())),
                Predicate::Tags(vec!["work".to_string()]),
            ])
        );
    }

    /// Verifies explicit `or` combines predicates via `Predicate::Or`.
    #[test]
    fn parse_explicit_or() {
        let p = parse_query("todo:TODO or todo:NEXT").unwrap();
        assert_eq!(
            p,
            Predicate::Or(vec![
                Predicate::Todo(Some("TODO".to_string())),
                Predicate::Todo(Some("NEXT".to_string())),
            ])
        );
    }

    /// Verifies `not` negates the following predicate atom.
    #[test]
    fn parse_not() {
        let p = parse_query("not done").unwrap();
        assert_eq!(p, Predicate::Not(Box::new(Predicate::Done)));
    }

    /// Verifies parenthesized `or` nested inside `and` preserves operator precedence.
    #[test]
    fn parse_complex_query() {
        let p = parse_query("(todo:TODO or todo:NEXT) and tags:work").unwrap();
        assert_eq!(
            p,
            Predicate::And(vec![
                Predicate::Or(vec![
                    Predicate::Todo(Some("TODO".to_string())),
                    Predicate::Todo(Some("NEXT".to_string())),
                ]),
                Predicate::Tags(vec!["work".to_string()]),
            ])
        );
    }

    /// Verifies unrecognized bare words fall back to `Predicate::Heading`.
    #[test]
    fn parse_bare_word_as_heading() {
        let p = parse_query("meeting").unwrap();
        assert_eq!(p, Predicate::Heading("meeting".to_string()));
    }

    /// Verifies empty queries return a [`ParseError`].
    #[test]
    fn parse_empty_fails() {
        assert!(parse_query("").is_err());
    }

    /// Verifies `deadline:past` parses as `DateMatch::Past`.
    #[test]
    fn parse_deadline_past() {
        let p = parse_query("deadline:past").unwrap();
        assert_eq!(p, Predicate::Deadline(DateMatch::Past));
    }

    /// Verifies bare `waiting` parses as `Predicate::Waiting`.
    #[test]
    fn parse_waiting() {
        let p = parse_query("waiting").unwrap();
        assert_eq!(p, Predicate::Waiting);
    }

    /// Verifies ISO-8601 `YYYY-MM-DD` dates parse as `DateRef::Absolute`.
    #[test]
    fn parse_absolute_date() {
        let p = parse_query("scheduled:2024-01-15").unwrap();
        assert_eq!(
            p,
            Predicate::Scheduled(DateMatch::Cmp(CmpOp::Eq, DateRef::Absolute(2024, 1, 15)))
        );
    }

    /// Verifies `body:<term>` and `body:"<phrase>"` parse into `Predicate::Body`.
    #[test]
    fn parse_body_term_and_quoted_phrase() {
        assert_eq!(
            parse_query("body:Herzmaschine").unwrap(),
            Predicate::Body("Herzmaschine".to_string())
        );
        assert_eq!(
            parse_query("body:\"Herzmaschine\"").unwrap(),
            Predicate::Body("Herzmaschine".to_string())
        );
        assert_eq!(
            parse_query("body:\"multi word (v2)\"").unwrap(),
            Predicate::Body("multi word (v2)".to_string())
        );
        assert_eq!(
            parse_query("property:CATEGORY=\"project plan\"").unwrap(),
            Predicate::Property {
                key: "CATEGORY".to_string(),
                value: "project plan".to_string(),
            }
        );
        assert!(parse_query("body:\"").is_err());
        assert!(parse_query("body:\"\"").is_err());
    }
}

// endregion
