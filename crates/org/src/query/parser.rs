// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Query language parser for org-mode entry matching.
//!
//! Supports a plain-text query syntax:
//! - `todo:TODO` — match specific TODO keyword
//! - `tags:work,urgent` — match entries with all listed tags
//! - `heading:meeting` — substring match on heading title
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "query parse error: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

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

// --- Tokenizer ---

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A `key:value` term or bare keyword.
    Term(String),
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

fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        if ch == '(' {
            tokens.push(Token::LParen);
            chars.next();
            continue;
        }
        if ch == ')' {
            tokens.push(Token::RParen);
            chars.next();
            continue;
        }

        // Read a word (until whitespace or paren).
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '(' || c == ')' {
                break;
            }
            word.push(c);
            chars.next();
        }

        match word.as_str() {
            "and" | "AND" => tokens.push(Token::And),
            "or" | "OR" => tokens.push(Token::Or),
            "not" | "NOT" => tokens.push(Token::Not),
            _ => tokens.push(Token::Term(word)),
        }
    }

    Ok(tokens)
}

// --- Recursive descent parser ---

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
            Some(Token::Term(_) | Token::Not | Token::LParen) => {
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
        Some(tok) => Err(ParseError(format!("unexpected token: {tok:?}"))),
        None => Err(ParseError("unexpected end of query".to_string())),
    }
}

/// Parse a single term like `todo:TODO`, `tags:work,urgent`, `done`, `clocked`.
fn parse_term(term: &str) -> Result<Predicate, ParseError> {
    // Bare keywords.
    match term {
        "done" | "DONE" => return Ok(Predicate::Done),
        "clocked" => return Ok(Predicate::Clocked),
        "todo" | "TODO" => return Ok(Predicate::Todo(None)),
        "blocked" => return Ok(Predicate::Blocked),
        "actionable" => return Ok(Predicate::Actionable),
        "waiting" | "WAITING" => return Ok(Predicate::Waiting),
        _ => {}
    }

    // key:value terms.
    if let Some(colon_pos) = term.find(':') {
        let key = &term[..colon_pos];
        let value = &term[colon_pos + 1..];

        match key {
            "todo" => {
                if value.is_empty() {
                    Ok(Predicate::Todo(None))
                } else {
                    Ok(Predicate::Todo(Some(value.to_string())))
                }
            }
            "tags" | "tag" => {
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
            "heading" | "h" => {
                if value.is_empty() {
                    Err(ParseError("empty heading value".to_string()))
                } else {
                    Ok(Predicate::Heading(value.to_string()))
                }
            }
            "property" | "prop" => parse_property_term(value),
            "priority" | "pri" => parse_priority_term(value),
            "level" | "lvl" => parse_level_term(value),
            "scheduled" | "sched" => {
                let dm = parse_date_match(value)?;
                Ok(Predicate::Scheduled(dm))
            }
            "deadline" | "dl" => {
                let dm = parse_date_match(value)?;
                Ok(Predicate::Deadline(dm))
            }
            "closed" => {
                let dm = parse_date_match(value)?;
                Ok(Predicate::Closed(dm))
            }
            _ => Err(ParseError(format!("unknown predicate: {key}"))),
        }
    } else {
        // Bare word — treat as heading substring search.
        Ok(Predicate::Heading(term.to_string()))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_todo() {
        let p = parse_query("todo").unwrap();
        assert_eq!(p, Predicate::Todo(None));
    }

    #[test]
    fn parse_todo_keyword() {
        let p = parse_query("todo:TODO").unwrap();
        assert_eq!(p, Predicate::Todo(Some("TODO".to_string())));
    }

    #[test]
    fn parse_done() {
        let p = parse_query("done").unwrap();
        assert_eq!(p, Predicate::Done);
    }

    #[test]
    fn parse_tags() {
        let p = parse_query("tags:work,urgent").unwrap();
        assert_eq!(
            p,
            Predicate::Tags(vec!["work".to_string(), "urgent".to_string()])
        );
    }

    #[test]
    fn parse_heading() {
        let p = parse_query("heading:meeting").unwrap();
        assert_eq!(p, Predicate::Heading("meeting".to_string()));
    }

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

    #[test]
    fn parse_priority_exact() {
        let p = parse_query("priority:A").unwrap();
        assert_eq!(p, Predicate::Priority(PriorityMatch::Exact('A')));
    }

    #[test]
    fn parse_priority_comparison() {
        let p = parse_query("priority:>=B").unwrap();
        assert_eq!(p, Predicate::Priority(PriorityMatch::Cmp(CmpOp::Gte, 'B')));
    }

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

    #[test]
    fn parse_scheduled() {
        let p = parse_query("scheduled:<=today").unwrap();
        assert_eq!(
            p,
            Predicate::Scheduled(DateMatch::Cmp(CmpOp::Lte, DateRef::Today))
        );
    }

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

    #[test]
    fn parse_clocked() {
        let p = parse_query("clocked").unwrap();
        assert_eq!(p, Predicate::Clocked);
    }

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

    #[test]
    fn parse_not() {
        let p = parse_query("not done").unwrap();
        assert_eq!(p, Predicate::Not(Box::new(Predicate::Done)));
    }

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

    #[test]
    fn parse_bare_word_as_heading() {
        let p = parse_query("meeting").unwrap();
        assert_eq!(p, Predicate::Heading("meeting".to_string()));
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_query("").is_err());
    }

    #[test]
    fn parse_deadline_past() {
        let p = parse_query("deadline:past").unwrap();
        assert_eq!(p, Predicate::Deadline(DateMatch::Past));
    }

    #[test]
    fn parse_waiting() {
        let p = parse_query("waiting").unwrap();
        assert_eq!(p, Predicate::Waiting);
    }

    #[test]
    fn parse_absolute_date() {
        let p = parse_query("scheduled:2024-01-15").unwrap();
        assert_eq!(
            p,
            Predicate::Scheduled(DateMatch::Cmp(CmpOp::Eq, DateRef::Absolute(2024, 1, 15)))
        );
    }
}
