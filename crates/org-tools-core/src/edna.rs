// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parser and evaluator for the org-edna task dependency mini-language.
//!
//! org-edna uses `:BLOCKER:` and `:TRIGGER:` properties in `PROPERTIES` drawers
//! to express task dependencies. The mini-language consists of **finders** (target
//! selectors), **actions** (state changes, suffixed with `!`), and **conditions**
//! (boolean checks, suffixed with `?`).
//!
//! This module provides:
//! - [`parse_edna`] — parse a property value into a list of [`EdnaExpr`] nodes
//! - [`is_blocked`] — evaluate whether an entry's `:BLOCKER:` dependencies are satisfied
//!
//! Ref: <https://www.nongnu.org/org-edna-el/>

use crate::document::{OrgDocument, OrgEntry};
use crate::locator::locator_for_entry;

// ---------------------------------------------------------------------------
// AST types
// ---------------------------------------------------------------------------

/// A single expression in the edna mini-language.
#[derive(Debug, Clone, PartialEq)]
pub enum EdnaExpr {
    /// A finder — selects target entries.
    Finder(Finder),
    /// An action — modifies target entries (only valid in `:TRIGGER:`).
    Action(Action),
    /// A condition — boolean test on target entries (only valid in `:BLOCKER:`).
    Condition(Condition),
    /// Conditional block: `if COND then EXPRS [else EXPRS] endif`.
    If {
        /// The condition expression.
        cond: Box<EdnaExpr>,
        /// Expressions to run when condition is true.
        then: Vec<EdnaExpr>,
        /// Expressions to run when condition is false.
        else_: Vec<EdnaExpr>,
    },
}

/// A finder selects target entries for subsequent actions or conditions.
#[derive(Debug, Clone, PartialEq)]
pub enum Finder {
    /// `ids(UUID ...)` — look up entries by `:ID:` property.
    Ids(Vec<String>),
    /// `self` — the current entry.
    Self_,
    /// `next-sibling` — the next sibling heading.
    NextSibling,
    /// `previous-sibling` — the previous sibling heading.
    PreviousSibling,
    /// `rest-of-siblings` — all subsequent siblings.
    RestOfSiblings,
    /// `parent` — the parent heading.
    Parent,
    /// `ancestors` — all ancestor headings.
    Ancestors,
    /// `children` — direct child headings.
    Children,
    /// `descendants` — all descendant headings.
    Descendants,
    /// `siblings` — all sibling headings.
    Siblings,
    /// `first-child` — the first child heading.
    FirstChild,
    /// `match("TAG_SPEC")` — entries matching a tag specification.
    Match(String),
    /// `olp("FILE" "HEADING" ...)` — outline path lookup.
    Olp(Vec<String>),
    /// `file("FILENAME")` — all entries in a file.
    File(String),
    /// `org-file("FILENAME")` — all entries in an org file (searched in org paths).
    OrgFile(String),
    /// `relatives(...)` — generic relative finder with options.
    Relatives(Vec<String>),
}

/// An action modifies target entries (suffix `!`).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `todo!("STATE")` — change TODO keyword.
    Todo(String),
    /// `scheduled!("TIMESPEC")` — set/modify SCHEDULED timestamp.
    Scheduled(String),
    /// `deadline!("TIMESPEC")` — set/modify DEADLINE timestamp.
    Deadline(String),
    /// `set-property!("KEY" "VALUE")` — set a property.
    SetProperty(String, String),
    /// `delete-property!("KEY")` — remove a property.
    DeleteProperty(String),
    /// `tag!("TAG")` — add or remove a tag.
    Tag(String),
    /// `set-priority!("PRIORITY")` — set priority cookie.
    SetPriority(String),
    /// `set-effort!("EFFORT")` — set effort property.
    SetEffort(String),
    /// `archive!` — archive the heading.
    Archive,
    /// `clock-in!` — start the clock.
    ClockIn,
    /// `clock-out!` — stop the clock.
    ClockOut,
    /// `chain!("PROPERTY")` — chain actions via a property.
    Chain(String),
}

/// A condition tests target entries (suffix `?`).
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// `todo-state?("KEYWORD")` — entry must be in specific TODO state.
    TodoState(String),
    /// `has-property?("KEY" "VALUE")` — entry must have property with value.
    HasProperty(String, String),
    /// `re-search?("REGEX")` — entry body matches regex.
    ReSearch(String),
    /// `variable-set?("VAR" "VALUE")` — Emacs variable check (not evaluable).
    VariableSet(String, String),
    /// `has-tags?("TAG" ...)` — entry must have all listed tags.
    HasTags(Vec<String>),
}

/// An error from parsing edna syntax.
#[derive(Debug, Clone, PartialEq)]
pub struct EdnaParseError {
    /// Human-readable error message.
    pub message: String,
    /// Byte offset into the source property value where the error occurred.
    pub offset: usize,
}

// ---------------------------------------------------------------------------
// Tokeniser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A bare word or keyword (e.g. `next-sibling`, `todo!`, `has-property?`).
    Word(String),
    /// A quoted string (content without quotes).
    Quoted(String),
    /// Opening parenthesis.
    LParen,
    /// Closing parenthesis.
    RParen,
}

struct Tokenizer<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn next_token(&mut self) -> Option<(Token, usize)> {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return None;
        }
        let start = self.pos;
        let ch = self.input.as_bytes()[self.pos];
        match ch {
            b'(' => {
                self.pos += 1;
                Some((Token::LParen, start))
            }
            b')' => {
                self.pos += 1;
                Some((Token::RParen, start))
            }
            b'"' => {
                self.pos += 1; // skip opening quote
                let content_start = self.pos;
                while self.pos < self.input.len() && self.input.as_bytes()[self.pos] != b'"' {
                    if self.input.as_bytes()[self.pos] == b'\\' {
                        self.pos += 1; // skip escaped char
                    }
                    self.pos += 1;
                }
                let content = &self.input[content_start..self.pos];
                if self.pos < self.input.len() {
                    self.pos += 1; // skip closing quote
                }
                Some((Token::Quoted(content.to_string()), start))
            }
            _ => {
                // Read a word: any non-whitespace, non-paren characters.
                while self.pos < self.input.len() {
                    let b = self.input.as_bytes()[self.pos];
                    if b.is_ascii_whitespace() || b == b'(' || b == b')' {
                        break;
                    }
                    self.pos += 1;
                }
                let word = &self.input[start..self.pos];
                Some((Token::Word(word.to_string()), start))
            }
        }
    }

    fn tokenize_all(&mut self) -> Vec<(Token, usize)> {
        let mut tokens = Vec::new();
        while let Some(tok) = self.next_token() {
            tokens.push(tok);
        }
        tokens
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<(Token, usize)>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<(Token, usize)>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn peek_offset(&self) -> usize {
        self.tokens.get(self.pos).map(|(_, o)| *o).unwrap_or(0)
    }

    fn advance(&mut self) -> Option<(Token, usize)> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn expect_lparen(&mut self) -> Result<(), EdnaParseError> {
        match self.advance() {
            Some((Token::LParen, _)) => Ok(()),
            Some((_, offset)) => Err(EdnaParseError {
                message: "expected '('".to_string(),
                offset,
            }),
            None => Err(EdnaParseError {
                message: "unexpected end of input, expected '('".to_string(),
                offset: 0,
            }),
        }
    }

    /// Read arguments (quoted strings or bare words) until `)`.
    fn read_args(&mut self) -> Result<Vec<String>, EdnaParseError> {
        self.expect_lparen()?;
        let mut args = Vec::new();
        loop {
            match self.peek() {
                Some(Token::RParen) => {
                    self.advance();
                    return Ok(args);
                }
                Some(Token::Quoted(_)) | Some(Token::Word(_)) => {
                    if let Some((tok, _)) = self.advance() {
                        match tok {
                            Token::Quoted(s) | Token::Word(s) => args.push(s),
                            _ => unreachable!(),
                        }
                    }
                }
                Some(Token::LParen) => {
                    return Err(EdnaParseError {
                        message: "unexpected '(' inside argument list".to_string(),
                        offset: self.peek_offset(),
                    });
                }
                None => {
                    return Err(EdnaParseError {
                        message: "unclosed parentheses in argument list".to_string(),
                        offset: self.peek_offset(),
                    });
                }
            }
        }
    }

    /// Check if next token is `(` (the keyword takes arguments).
    fn has_args(&self) -> bool {
        matches!(self.peek(), Some(Token::LParen))
    }

    /// Parse a single expression (finder, action, condition, or if-block).
    fn parse_expr(&mut self) -> Result<EdnaExpr, EdnaParseError> {
        let (tok, offset) = match self.advance() {
            Some(t) => t,
            None => {
                return Err(EdnaParseError {
                    message: "unexpected end of input".to_string(),
                    offset: 0,
                })
            }
        };

        let word = match tok {
            Token::Word(w) => w,
            Token::Quoted(s) => {
                return Err(EdnaParseError {
                    message: format!("unexpected quoted string \"{s}\", expected a keyword"),
                    offset,
                })
            }
            Token::LParen => {
                return Err(EdnaParseError {
                    message: "unexpected '('".to_string(),
                    offset,
                })
            }
            Token::RParen => {
                return Err(EdnaParseError {
                    message: "unexpected ')'".to_string(),
                    offset,
                })
            }
        };

        let lower = word.to_lowercase();

        // Handle `if` block.
        if lower == "if" {
            return self.parse_if_block(offset);
        }

        // Determine type by suffix.
        if lower.ends_with('!') {
            self.parse_action(&lower, offset)
        } else if lower.ends_with('?') {
            self.parse_condition(&lower, offset)
        } else {
            self.parse_finder(&lower, offset)
        }
    }

    fn parse_finder(&mut self, keyword: &str, offset: usize) -> Result<EdnaExpr, EdnaParseError> {
        let finder = match keyword {
            "self" => Finder::Self_,
            "next-sibling" => Finder::NextSibling,
            "previous-sibling" => Finder::PreviousSibling,
            "rest-of-siblings" | "rest-of-siblings-wrap" => Finder::RestOfSiblings,
            "parent" => Finder::Parent,
            "ancestors" => Finder::Ancestors,
            "children" => Finder::Children,
            "descendants" => Finder::Descendants,
            "siblings" => Finder::Siblings,
            "first-child" => Finder::FirstChild,
            "ids" => {
                let args = self.read_args()?;
                if args.is_empty() {
                    return Err(EdnaParseError {
                        message: "ids() requires at least one argument".to_string(),
                        offset,
                    });
                }
                Finder::Ids(args)
            }
            "match" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "match() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Finder::Match(args.into_iter().next().unwrap())
            }
            "olp" => {
                let args = self.read_args()?;
                if args.is_empty() {
                    return Err(EdnaParseError {
                        message: "olp() requires at least one argument".to_string(),
                        offset,
                    });
                }
                Finder::Olp(args)
            }
            "file" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "file() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Finder::File(args.into_iter().next().unwrap())
            }
            "org-file" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "org-file() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Finder::OrgFile(args.into_iter().next().unwrap())
            }
            "relatives" => {
                let args = self.read_args()?;
                Finder::Relatives(args)
            }
            _ => {
                // Allow finders with optional parenthesised args we don't recognise,
                // but report unknown keyword.
                if self.has_args() {
                    let _ = self.read_args()?;
                }
                return Err(EdnaParseError {
                    message: format!("unknown edna finder: {keyword}"),
                    offset,
                });
            }
        };
        Ok(EdnaExpr::Finder(finder))
    }

    fn parse_action(&mut self, keyword: &str, offset: usize) -> Result<EdnaExpr, EdnaParseError> {
        let action = match keyword {
            "todo!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "todo!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::Todo(args.into_iter().next().unwrap())
            }
            "scheduled!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "scheduled!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::Scheduled(args.into_iter().next().unwrap())
            }
            "deadline!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "deadline!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::Deadline(args.into_iter().next().unwrap())
            }
            "set-property!" => {
                let args = self.read_args()?;
                if args.len() != 2 {
                    return Err(EdnaParseError {
                        message: "set-property!() requires exactly two arguments".to_string(),
                        offset,
                    });
                }
                let mut it = args.into_iter();
                Action::SetProperty(it.next().unwrap(), it.next().unwrap())
            }
            "delete-property!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "delete-property!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::DeleteProperty(args.into_iter().next().unwrap())
            }
            "tag!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "tag!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::Tag(args.into_iter().next().unwrap())
            }
            "set-priority!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "set-priority!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::SetPriority(args.into_iter().next().unwrap())
            }
            "set-effort!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "set-effort!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::SetEffort(args.into_iter().next().unwrap())
            }
            "archive!" => Action::Archive,
            "clock-in!" => Action::ClockIn,
            "clock-out!" => Action::ClockOut,
            "chain!" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "chain!() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Action::Chain(args.into_iter().next().unwrap())
            }
            _ => {
                if self.has_args() {
                    let _ = self.read_args()?;
                }
                return Err(EdnaParseError {
                    message: format!("unknown edna action: {keyword}"),
                    offset,
                });
            }
        };
        Ok(EdnaExpr::Action(action))
    }

    fn parse_condition(
        &mut self,
        keyword: &str,
        offset: usize,
    ) -> Result<EdnaExpr, EdnaParseError> {
        let condition = match keyword {
            "todo-state?" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "todo-state?() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Condition::TodoState(args.into_iter().next().unwrap())
            }
            "has-property?" => {
                let args = self.read_args()?;
                if args.len() != 2 {
                    return Err(EdnaParseError {
                        message: "has-property?() requires exactly two arguments".to_string(),
                        offset,
                    });
                }
                let mut it = args.into_iter();
                Condition::HasProperty(it.next().unwrap(), it.next().unwrap())
            }
            "re-search?" => {
                let args = self.read_args()?;
                if args.len() != 1 {
                    return Err(EdnaParseError {
                        message: "re-search?() requires exactly one argument".to_string(),
                        offset,
                    });
                }
                Condition::ReSearch(args.into_iter().next().unwrap())
            }
            "variable-set?" => {
                let args = self.read_args()?;
                if args.len() != 2 {
                    return Err(EdnaParseError {
                        message: "variable-set?() requires exactly two arguments".to_string(),
                        offset,
                    });
                }
                let mut it = args.into_iter();
                Condition::VariableSet(it.next().unwrap(), it.next().unwrap())
            }
            "has-tags?" => {
                let args = self.read_args()?;
                if args.is_empty() {
                    return Err(EdnaParseError {
                        message: "has-tags?() requires at least one argument".to_string(),
                        offset,
                    });
                }
                Condition::HasTags(args)
            }
            _ => {
                if self.has_args() {
                    let _ = self.read_args()?;
                }
                return Err(EdnaParseError {
                    message: format!("unknown edna condition: {keyword}"),
                    offset,
                });
            }
        };
        Ok(EdnaExpr::Condition(condition))
    }

    fn parse_if_block(&mut self, _offset: usize) -> Result<EdnaExpr, EdnaParseError> {
        // Parse condition expression.
        let cond = Box::new(self.parse_expr()?);

        // Expect `then` keyword.
        match self.advance() {
            Some((Token::Word(w), _)) if w.to_lowercase() == "then" => {}
            Some((_, off)) => {
                return Err(EdnaParseError {
                    message: "expected 'then' after if condition".to_string(),
                    offset: off,
                })
            }
            None => {
                return Err(EdnaParseError {
                    message: "unexpected end of input, expected 'then'".to_string(),
                    offset: 0,
                })
            }
        }

        // Parse then-branch expressions until `else` or `endif`.
        let mut then = Vec::new();
        let mut else_ = Vec::new();
        let mut in_else = false;

        loop {
            match self.peek() {
                Some(Token::Word(w)) if w.to_lowercase() == "else" => {
                    self.advance();
                    in_else = true;
                }
                Some(Token::Word(w)) if w.to_lowercase() == "endif" => {
                    self.advance();
                    break;
                }
                None => {
                    return Err(EdnaParseError {
                        message: "unclosed if-block, expected 'endif'".to_string(),
                        offset: 0,
                    });
                }
                _ => {
                    let expr = self.parse_expr()?;
                    if in_else {
                        else_.push(expr);
                    } else {
                        then.push(expr);
                    }
                }
            }
        }

        Ok(EdnaExpr::If { cond, then, else_ })
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an edna property value into a list of expressions.
///
/// Returns all successfully parsed expressions plus any errors encountered.
/// Parsing is lenient: it continues past errors to find as many issues as possible.
pub fn parse_edna(input: &str) -> (Vec<EdnaExpr>, Vec<EdnaParseError>) {
    let mut tokenizer = Tokenizer::new(input);
    let tokens = tokenizer.tokenize_all();
    let mut parser = Parser::new(tokens);
    let mut exprs = Vec::new();
    let mut errors = Vec::new();

    while parser.pos < parser.tokens.len() {
        match parser.parse_expr() {
            Ok(expr) => exprs.push(expr),
            Err(e) => {
                errors.push(e);
                // Skip one token to avoid infinite loop on errors.
                parser.pos += 1;
            }
        }
    }

    (exprs, errors)
}

// ---------------------------------------------------------------------------
// Blocker evaluation
// ---------------------------------------------------------------------------

/// Context for resolving edna finders across multiple documents.
pub struct EdnaContext<'a> {
    /// All loaded documents (for cross-file ID resolution).
    pub all_docs: &'a [&'a OrgDocument],
    /// The document containing the entry being evaluated.
    pub doc: &'a OrgDocument,
    /// Index of the entry being evaluated within `doc`.
    pub entry_idx: usize,
}

/// Details about a single blocking dependency.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockerDetail {
    /// Title of the blocking entry.
    pub title: String,
    /// Current TODO keyword of the blocking entry.
    pub keyword: Option<String>,
    /// File path of the blocking entry.
    pub file: String,
    /// Line number of the blocking entry.
    pub line: usize,
    /// Locator string for the blocking entry.
    pub locator: String,
    /// Human-readable description of the unsatisfied condition.
    pub condition_desc: String,
}

/// Get structured details about why an entry is blocked.
///
/// Returns a list of [`BlockerDetail`] for each unsatisfied dependency.
/// Returns an empty vec if the entry is not blocked or has no `:BLOCKER:` property.
pub fn blocking_details(ctx: &EdnaContext<'_>) -> Vec<BlockerDetail> {
    let entry = &ctx.doc.entries[ctx.entry_idx];
    let blocker_value = match entry.properties.get("BLOCKER") {
        Some(v) => v,
        None => return Vec::new(),
    };

    let (exprs, _errors) = parse_edna(blocker_value);
    if exprs.is_empty() {
        return Vec::new();
    }

    let mut targets: Vec<ResolvedEntry<'_>> = Vec::new();
    let mut has_explicit_condition = false;
    let mut details = Vec::new();

    for expr in &exprs {
        match expr {
            EdnaExpr::Finder(finder) => {
                targets = resolve_finder(finder, ctx);
            }
            EdnaExpr::Condition(condition) => {
                has_explicit_condition = true;
                let desc = condition_description(condition);
                for t in &targets {
                    if !evaluate_condition_single(condition, t) {
                        details.push(make_blocker_detail(t, &desc));
                    }
                }
            }
            EdnaExpr::If { cond, then, else_ } => {
                let branch = if evaluate_if_condition(cond, &targets) {
                    then
                } else {
                    else_
                };
                for sub_expr in branch {
                    if let EdnaExpr::Condition(c) = sub_expr {
                        has_explicit_condition = true;
                        let desc = condition_description(c);
                        for t in &targets {
                            if !evaluate_condition_single(c, t) {
                                details.push(make_blocker_detail(t, &desc));
                            }
                        }
                    }
                }
            }
            EdnaExpr::Action(_) => {}
        }
    }

    if !has_explicit_condition {
        // Default condition: all targets must be done.
        for t in &targets {
            let is_done = t
                .entry()
                .keyword
                .as_deref()
                .is_some_and(|kw| t.doc.todo_keywords.is_done(kw));
            if !is_done {
                details.push(make_blocker_detail(t, "must be done"));
            }
        }
    }

    details
}

/// Create a [`BlockerDetail`] from a resolved entry.
fn make_blocker_detail(target: &ResolvedEntry<'_>, condition_desc: &str) -> BlockerDetail {
    let entry = target.entry();
    let locator = locator_for_entry(target.doc, target.entry_idx);
    BlockerDetail {
        title: entry.title.clone(),
        keyword: entry.keyword.clone(),
        file: target.doc.file.display().to_string(),
        line: entry.heading_line,
        locator: locator.to_string(),
        condition_desc: condition_desc.to_string(),
    }
}

/// Human-readable description of what a condition requires.
fn condition_description(condition: &Condition) -> String {
    match condition {
        Condition::TodoState(kw) => format!("must have state {kw}"),
        Condition::HasProperty(key, value) => format!("must have property {key}={value}"),
        Condition::ReSearch(re) => format!("must match regex {re}"),
        Condition::VariableSet(var, val) => format!("variable {var} must be {val}"),
        Condition::HasTags(tags) => format!("must have tags {}", tags.join(", ")),
    }
}

/// Evaluate a condition against a single resolved entry.
fn evaluate_condition_single(condition: &Condition, target: &ResolvedEntry<'_>) -> bool {
    match condition {
        Condition::TodoState(keyword) => target
            .entry()
            .keyword
            .as_deref()
            .is_some_and(|kw| kw.eq_ignore_ascii_case(keyword)),
        Condition::HasProperty(key, value) => target
            .entry()
            .properties
            .get(key.as_str())
            .is_some_and(|v| v == value),
        Condition::HasTags(required_tags) => {
            let inherited = target.doc.inherited_tags(target.entry_idx);
            required_tags
                .iter()
                .all(|req| inherited.iter().any(|tag| tag.eq_ignore_ascii_case(req)))
        }
        Condition::ReSearch(_) | Condition::VariableSet(_, _) => true,
    }
}

/// Check whether an entry is blocked by its `:BLOCKER:` property.
///
/// Returns `true` if the entry has a `:BLOCKER:` property and its dependencies
/// are NOT satisfied (i.e. the task cannot be worked on yet).
///
/// Returns `false` if there is no `:BLOCKER:` property or all dependencies are met.
pub fn is_blocked(ctx: &EdnaContext<'_>) -> bool {
    let entry = &ctx.doc.entries[ctx.entry_idx];
    let blocker_value = match entry.properties.get("BLOCKER") {
        Some(v) => v,
        None => return false,
    };

    let (exprs, _errors) = parse_edna(blocker_value);
    if exprs.is_empty() {
        return false;
    }

    // Evaluate: resolve finders to target entries, then check conditions.
    // The default condition (when no explicit condition is present) is that
    // all target entries must be in a done state.
    let mut targets: Vec<ResolvedEntry<'_>> = Vec::new();
    let mut has_explicit_condition = false;
    let mut all_conditions_met = true;

    for expr in &exprs {
        match expr {
            EdnaExpr::Finder(finder) => {
                targets = resolve_finder(finder, ctx);
            }
            EdnaExpr::Condition(condition) => {
                has_explicit_condition = true;
                if !evaluate_condition(condition, &targets) {
                    all_conditions_met = false;
                }
            }
            EdnaExpr::If { cond, then, else_ } => {
                // Simplified: evaluate the condition, then process the branch.
                let branch = if evaluate_if_condition(cond, &targets) {
                    then
                } else {
                    else_
                };
                for sub_expr in branch {
                    if let EdnaExpr::Condition(c) = sub_expr {
                        has_explicit_condition = true;
                        if !evaluate_condition(c, &targets) {
                            all_conditions_met = false;
                        }
                    }
                }
            }
            EdnaExpr::Action(_) => {
                // Actions are not evaluated in BLOCKER context.
            }
        }
    }

    if !has_explicit_condition {
        // Default: all targets must be done.
        !targets_all_done(&targets)
    } else {
        !all_conditions_met
    }
}

/// A resolved entry reference — either in the current document or cross-file.
struct ResolvedEntry<'a> {
    doc: &'a OrgDocument,
    entry_idx: usize,
}

impl<'a> ResolvedEntry<'a> {
    fn entry(&self) -> &'a OrgEntry {
        &self.doc.entries[self.entry_idx]
    }
}

/// Check if all resolved target entries have a done keyword.
fn targets_all_done(targets: &[ResolvedEntry<'_>]) -> bool {
    if targets.is_empty() {
        // No targets resolved — conservatively not blocked.
        return true;
    }
    targets.iter().all(|t| {
        t.entry()
            .keyword
            .as_deref()
            .is_some_and(|kw| t.doc.todo_keywords.is_done(kw))
    })
}

/// Resolve a finder to a list of entries.
fn resolve_finder<'a>(finder: &Finder, ctx: &EdnaContext<'a>) -> Vec<ResolvedEntry<'a>> {
    match finder {
        Finder::Self_ => vec![ResolvedEntry {
            doc: ctx.doc,
            entry_idx: ctx.entry_idx,
        }],
        Finder::Ids(ids) => {
            let mut results = Vec::new();
            for id in ids {
                // Search across all documents.
                for doc in ctx.all_docs {
                    if let Some(idx) = doc.find_by_id(id) {
                        results.push(ResolvedEntry {
                            doc,
                            entry_idx: idx,
                        });
                        break; // IDs are unique, stop at first match.
                    }
                }
            }
            results
        }
        Finder::NextSibling => {
            let entry = &ctx.doc.entries[ctx.entry_idx];
            if let Some(parent_idx) = entry.parent {
                let siblings = &ctx.doc.entries[parent_idx].children;
                let my_pos = siblings.iter().position(|&i| i == ctx.entry_idx);
                if let Some(pos) = my_pos {
                    if pos + 1 < siblings.len() {
                        return vec![ResolvedEntry {
                            doc: ctx.doc,
                            entry_idx: siblings[pos + 1],
                        }];
                    }
                }
            } else {
                // Top-level: find next top-level entry.
                let top_level: Vec<usize> = ctx
                    .doc
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.parent.is_none())
                    .map(|(i, _)| i)
                    .collect();
                let my_pos = top_level.iter().position(|&i| i == ctx.entry_idx);
                if let Some(pos) = my_pos {
                    if pos + 1 < top_level.len() {
                        return vec![ResolvedEntry {
                            doc: ctx.doc,
                            entry_idx: top_level[pos + 1],
                        }];
                    }
                }
            }
            Vec::new()
        }
        Finder::PreviousSibling => {
            let entry = &ctx.doc.entries[ctx.entry_idx];
            if let Some(parent_idx) = entry.parent {
                let siblings = &ctx.doc.entries[parent_idx].children;
                let my_pos = siblings.iter().position(|&i| i == ctx.entry_idx);
                if let Some(pos) = my_pos {
                    if pos > 0 {
                        return vec![ResolvedEntry {
                            doc: ctx.doc,
                            entry_idx: siblings[pos - 1],
                        }];
                    }
                }
            } else {
                let top_level: Vec<usize> = ctx
                    .doc
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.parent.is_none())
                    .map(|(i, _)| i)
                    .collect();
                let my_pos = top_level.iter().position(|&i| i == ctx.entry_idx);
                if let Some(pos) = my_pos {
                    if pos > 0 {
                        return vec![ResolvedEntry {
                            doc: ctx.doc,
                            entry_idx: top_level[pos - 1],
                        }];
                    }
                }
            }
            Vec::new()
        }
        Finder::RestOfSiblings => {
            let siblings = get_siblings(ctx);
            let my_pos = siblings.iter().position(|&i| i == ctx.entry_idx);
            match my_pos {
                Some(pos) => siblings[pos + 1..]
                    .iter()
                    .map(|&i| ResolvedEntry {
                        doc: ctx.doc,
                        entry_idx: i,
                    })
                    .collect(),
                None => Vec::new(),
            }
        }
        Finder::Parent => {
            if let Some(parent_idx) = ctx.doc.entries[ctx.entry_idx].parent {
                vec![ResolvedEntry {
                    doc: ctx.doc,
                    entry_idx: parent_idx,
                }]
            } else {
                Vec::new()
            }
        }
        Finder::Ancestors => {
            let mut results = Vec::new();
            let mut current = ctx.entry_idx;
            while let Some(parent_idx) = ctx.doc.entries[current].parent {
                results.push(ResolvedEntry {
                    doc: ctx.doc,
                    entry_idx: parent_idx,
                });
                current = parent_idx;
            }
            results
        }
        Finder::Children => ctx.doc.entries[ctx.entry_idx]
            .children
            .iter()
            .map(|&i| ResolvedEntry {
                doc: ctx.doc,
                entry_idx: i,
            })
            .collect(),
        Finder::Descendants => {
            let mut results = Vec::new();
            collect_descendants(ctx.doc, ctx.entry_idx, &mut results);
            results
        }
        Finder::Siblings => {
            let siblings = get_siblings(ctx);
            siblings
                .iter()
                .filter(|&&i| i != ctx.entry_idx)
                .map(|&i| ResolvedEntry {
                    doc: ctx.doc,
                    entry_idx: i,
                })
                .collect()
        }
        Finder::FirstChild => {
            let children = &ctx.doc.entries[ctx.entry_idx].children;
            if let Some(&first) = children.first() {
                vec![ResolvedEntry {
                    doc: ctx.doc,
                    entry_idx: first,
                }]
            } else {
                Vec::new()
            }
        }
        Finder::Match(tag_spec) => {
            // Match entries across all docs with the given tag.
            let mut results = Vec::new();
            for doc in ctx.all_docs {
                for (idx, _entry) in doc.entries.iter().enumerate() {
                    let inherited = doc.inherited_tags(idx);
                    if inherited.iter().any(|t| t.eq_ignore_ascii_case(tag_spec)) {
                        results.push(ResolvedEntry {
                            doc,
                            entry_idx: idx,
                        });
                    }
                }
            }
            results
        }
        // These finders require Emacs or filesystem context we don't have.
        Finder::Olp(_) | Finder::File(_) | Finder::OrgFile(_) | Finder::Relatives(_) => Vec::new(),
    }
}

/// Get sibling indices for the current entry.
fn get_siblings(ctx: &EdnaContext<'_>) -> Vec<usize> {
    let entry = &ctx.doc.entries[ctx.entry_idx];
    if let Some(parent_idx) = entry.parent {
        ctx.doc.entries[parent_idx].children.clone()
    } else {
        ctx.doc
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.parent.is_none())
            .map(|(i, _)| i)
            .collect()
    }
}

/// Recursively collect all descendant entry indices.
fn collect_descendants<'a>(
    doc: &'a OrgDocument,
    entry_idx: usize,
    results: &mut Vec<ResolvedEntry<'a>>,
) {
    for &child_idx in &doc.entries[entry_idx].children {
        results.push(ResolvedEntry {
            doc,
            entry_idx: child_idx,
        });
        collect_descendants(doc, child_idx, results);
    }
}

/// Evaluate a condition against resolved target entries.
fn evaluate_condition(condition: &Condition, targets: &[ResolvedEntry<'_>]) -> bool {
    match condition {
        Condition::TodoState(keyword) => targets.iter().all(|t| {
            t.entry()
                .keyword
                .as_deref()
                .is_some_and(|kw| kw.eq_ignore_ascii_case(keyword))
        }),
        Condition::HasProperty(key, value) => targets.iter().all(|t| {
            t.entry()
                .properties
                .get(key.as_str())
                .is_some_and(|v| v == value)
        }),
        Condition::HasTags(required_tags) => targets.iter().all(|t| {
            let inherited = t.doc.inherited_tags(t.entry_idx);
            required_tags
                .iter()
                .all(|req| inherited.iter().any(|tag| tag.eq_ignore_ascii_case(req)))
        }),
        // re-search? and variable-set? require Emacs — conservatively pass.
        Condition::ReSearch(_) | Condition::VariableSet(_, _) => true,
    }
}

/// Evaluate the condition part of an `if` block.
fn evaluate_if_condition(cond: &EdnaExpr, targets: &[ResolvedEntry<'_>]) -> bool {
    match cond {
        EdnaExpr::Condition(c) => evaluate_condition(c, targets),
        _ => true, // Non-condition in if-position — default to true.
    }
}

/// Extract dependency edges from edna expressions for graph building.
///
/// Returns `(finder_type, referenced_ids)` pairs that represent edges in a
/// dependency graph. Only extracts edges from finders that reference specific
/// entries (e.g. `ids()`), not structural finders.
pub fn extract_dependency_ids(input: &str) -> Vec<String> {
    let (exprs, _) = parse_edna(input);
    let mut ids = Vec::new();
    for expr in &exprs {
        if let EdnaExpr::Finder(Finder::Ids(ref found_ids)) = expr {
            ids.extend(found_ids.iter().cloned());
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_finder() {
        let (exprs, errors) = parse_edna("next-sibling");
        assert!(errors.is_empty());
        assert_eq!(exprs, vec![EdnaExpr::Finder(Finder::NextSibling)]);
    }

    #[test]
    fn parse_ids_finder() {
        let (exprs, errors) = parse_edna("ids(\"abc-123\" \"def-456\")");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Finder(Finder::Ids(vec![
                "abc-123".to_string(),
                "def-456".to_string()
            ]))]
        );
    }

    #[test]
    fn parse_ids_bare_words() {
        let (exprs, errors) = parse_edna("ids(abc-123 def-456)");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Finder(Finder::Ids(vec![
                "abc-123".to_string(),
                "def-456".to_string()
            ]))]
        );
    }

    #[test]
    fn parse_action() {
        let (exprs, errors) = parse_edna("todo!(\"DONE\")");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Action(Action::Todo("DONE".to_string()))]
        );
    }

    #[test]
    fn parse_condition() {
        let (exprs, errors) = parse_edna("todo-state?(\"DONE\")");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Condition(Condition::TodoState(
                "DONE".to_string()
            ))]
        );
    }

    #[test]
    fn parse_multiple_exprs() {
        let (exprs, errors) = parse_edna("ids(\"uuid-1\") todo!(\"DONE\")");
        assert!(errors.is_empty());
        assert_eq!(exprs.len(), 2);
        assert!(matches!(exprs[0], EdnaExpr::Finder(Finder::Ids(_))));
        assert!(matches!(exprs[1], EdnaExpr::Action(Action::Todo(_))));
    }

    #[test]
    fn parse_set_property_action() {
        let (exprs, errors) = parse_edna("set-property!(\"KEY\" \"VALUE\")");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Action(Action::SetProperty(
                "KEY".to_string(),
                "VALUE".to_string()
            ))]
        );
    }

    #[test]
    fn parse_match_finder() {
        let (exprs, errors) = parse_edna("match(\"work\")");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Finder(Finder::Match("work".to_string()))]
        );
    }

    #[test]
    fn parse_if_block() {
        let (exprs, errors) =
            parse_edna("if todo-state?(\"DONE\") then next-sibling todo!(\"TODO\") endif");
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(exprs.len(), 1);
        match &exprs[0] {
            EdnaExpr::If { cond, then, else_ } => {
                assert!(matches!(
                    cond.as_ref(),
                    EdnaExpr::Condition(Condition::TodoState(_))
                ));
                assert_eq!(then.len(), 2);
                assert!(else_.is_empty());
            }
            _ => panic!("expected If expression"),
        }
    }

    #[test]
    fn parse_if_else_block() {
        let (exprs, errors) = parse_edna(
            "if todo-state?(\"DONE\") then todo!(\"TODO\") else todo!(\"WAITING\") endif",
        );
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(exprs.len(), 1);
        match &exprs[0] {
            EdnaExpr::If {
                cond: _,
                then,
                else_,
            } => {
                assert_eq!(then.len(), 1);
                assert_eq!(else_.len(), 1);
            }
            _ => panic!("expected If expression"),
        }
    }

    #[test]
    fn parse_unknown_finder_reports_error() {
        let (_, errors) = parse_edna("nonexistent-finder");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unknown edna finder"));
    }

    #[test]
    fn parse_unknown_action_reports_error() {
        let (_, errors) = parse_edna("nonexistent!(\"arg\")");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unknown edna action"));
    }

    #[test]
    fn parse_unknown_condition_reports_error() {
        let (_, errors) = parse_edna("nonexistent?(\"arg\")");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unknown edna condition"));
    }

    #[test]
    fn parse_unclosed_parens_reports_error() {
        let (_, errors) = parse_edna("ids(\"abc\"");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unclosed"));
    }

    #[test]
    fn parse_ids_empty_reports_error() {
        let (_, errors) = parse_edna("ids()");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("at least one argument"));
    }

    #[test]
    fn parse_archive_action_no_args() {
        let (exprs, errors) = parse_edna("archive!");
        assert!(errors.is_empty());
        assert_eq!(exprs, vec![EdnaExpr::Action(Action::Archive)]);
    }

    #[test]
    fn parse_self_finder() {
        let (exprs, errors) = parse_edna("self");
        assert!(errors.is_empty());
        assert_eq!(exprs, vec![EdnaExpr::Finder(Finder::Self_)]);
    }

    #[test]
    fn parse_structural_finders() {
        for keyword in &[
            "parent",
            "ancestors",
            "children",
            "descendants",
            "siblings",
            "first-child",
        ] {
            let (exprs, errors) = parse_edna(keyword);
            assert!(errors.is_empty(), "error parsing {keyword}: {errors:?}");
            assert_eq!(exprs.len(), 1, "wrong count for {keyword}");
            assert!(
                matches!(exprs[0], EdnaExpr::Finder(_)),
                "not a finder for {keyword}"
            );
        }
    }

    #[test]
    fn parse_has_property_condition() {
        let (exprs, errors) = parse_edna("has-property?(\"COUNT\" \"3\")");
        assert!(errors.is_empty());
        assert_eq!(
            exprs,
            vec![EdnaExpr::Condition(Condition::HasProperty(
                "COUNT".to_string(),
                "3".to_string()
            ))]
        );
    }

    #[test]
    fn parse_complex_blocker() {
        let (exprs, errors) = parse_edna("ids(\"task-1\" \"task-2\") todo-state?(\"DONE\")");
        assert!(errors.is_empty());
        assert_eq!(exprs.len(), 2);
    }

    #[test]
    fn extract_ids_from_blocker() {
        let ids = extract_dependency_ids("ids(\"abc\" \"def\") todo-state?(\"DONE\")");
        assert_eq!(ids, vec!["abc", "def"]);
    }

    #[test]
    fn extract_ids_no_ids_finder() {
        let ids = extract_dependency_ids("next-sibling");
        assert!(ids.is_empty());
    }

    // -- Blocker evaluation tests --

    fn make_doc_with_entries(entries_data: &[(&str, Option<&str>, Option<usize>)]) -> OrgDocument {
        // Build a minimal OrgDocument from (title, keyword, parent) tuples.
        use std::collections::HashMap;
        let mut entries = Vec::new();
        for (i, (title, keyword, parent)) in entries_data.iter().enumerate() {
            entries.push(OrgEntry {
                level: if parent.is_some() { 2 } else { 1 },
                keyword: keyword.map(|s| s.to_string()),
                priority: None,
                title: title.to_string(),
                tags: Vec::new(),
                properties: HashMap::new(),
                planning: crate::document::Planning::default(),
                clocks: Vec::new(),
                heading_line: i + 1,
                heading_offset: 0,
                content_end_line: i + 2,
                parent: *parent,
                children: Vec::new(),
                raw_heading: format!("* {title}"),
            });
        }
        // Build children lists.
        for i in 0..entries.len() {
            if let Some(parent_idx) = entries[i].parent {
                // Must use index to avoid borrow issues.
                let child_idx = i;
                entries[parent_idx].children.push(child_idx);
            }
        }

        OrgDocument {
            file: std::path::PathBuf::from("test.org"),
            entries,
            file_properties: HashMap::new(),
            file_keywords: HashMap::new(),
            todo_keywords: Default::default(),
            priority_range: Default::default(),
            filetags: Vec::new(),
            default_properties: HashMap::new(),
            tag_spec: Default::default(),
            link_abbreviations: HashMap::new(),
            table_constants: HashMap::new(),
        }
    }

    #[test]
    fn not_blocked_without_blocker_property() {
        let doc = make_doc_with_entries(&[("Task A", Some("TODO"), None)]);
        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 0,
        };
        assert!(!is_blocked(&ctx));
    }

    #[test]
    fn blocked_by_undone_dependency() {
        let mut doc =
            make_doc_with_entries(&[("Dep", Some("TODO"), None), ("Blocked", Some("TODO"), None)]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "dep-1".to_string());
        doc.entries[1]
            .properties
            .insert("BLOCKER".to_string(), "ids(\"dep-1\")".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        assert!(is_blocked(&ctx));
    }

    #[test]
    fn not_blocked_when_dependency_done() {
        let mut doc =
            make_doc_with_entries(&[("Dep", Some("DONE"), None), ("Blocked", Some("TODO"), None)]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "dep-1".to_string());
        doc.entries[1]
            .properties
            .insert("BLOCKER".to_string(), "ids(\"dep-1\")".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        assert!(!is_blocked(&ctx));
    }

    #[test]
    fn blocked_by_previous_sibling() {
        let mut doc = make_doc_with_entries(&[
            ("First", Some("TODO"), None),
            ("Second", Some("TODO"), None),
        ]);
        doc.entries[1]
            .properties
            .insert("BLOCKER".to_string(), "previous-sibling".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        assert!(is_blocked(&ctx));
    }

    #[test]
    fn not_blocked_when_previous_sibling_done() {
        let mut doc = make_doc_with_entries(&[
            ("First", Some("DONE"), None),
            ("Second", Some("TODO"), None),
        ]);
        doc.entries[1]
            .properties
            .insert("BLOCKER".to_string(), "previous-sibling".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        assert!(!is_blocked(&ctx));
    }

    #[test]
    fn blocked_with_explicit_condition() {
        let mut doc = make_doc_with_entries(&[
            ("Dep", Some("WAITING"), None),
            ("Blocked", Some("TODO"), None),
        ]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "dep-1".to_string());
        doc.entries[1].properties.insert(
            "BLOCKER".to_string(),
            "ids(\"dep-1\") todo-state?(\"DONE\")".to_string(),
        );

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        // Dep is WAITING, condition requires DONE → blocked.
        assert!(is_blocked(&ctx));
    }

    #[test]
    fn cross_file_id_resolution() {
        let mut doc1 = make_doc_with_entries(&[("Dep in file 1", Some("DONE"), None)]);
        doc1.entries[0]
            .properties
            .insert("ID".to_string(), "cross-dep".to_string());

        let mut doc2 = make_doc_with_entries(&[("Task in file 2", Some("TODO"), None)]);
        doc2.entries[0]
            .properties
            .insert("BLOCKER".to_string(), "ids(\"cross-dep\")".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc1, &doc2];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc2,
            entry_idx: 0,
        };
        // Dep is DONE → not blocked.
        assert!(!is_blocked(&ctx));
    }

    // -- blocking_details() tests --

    #[test]
    fn details_empty_when_not_blocked() {
        let doc = make_doc_with_entries(&[("Task A", Some("TODO"), None)]);
        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 0,
        };
        assert!(blocking_details(&ctx).is_empty());
    }

    #[test]
    fn details_shows_undone_dependency() {
        let mut doc = make_doc_with_entries(&[
            ("Research", Some("TODO"), None),
            ("Write", Some("TODO"), None),
        ]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "dep-1".to_string());
        doc.entries[1]
            .properties
            .insert("BLOCKER".to_string(), "ids(\"dep-1\")".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        let details = blocking_details(&ctx);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].title, "Research");
        assert_eq!(details[0].keyword.as_deref(), Some("TODO"));
        assert_eq!(details[0].condition_desc, "must be done");
    }

    #[test]
    fn details_empty_when_dependency_done() {
        let mut doc = make_doc_with_entries(&[
            ("Research", Some("DONE"), None),
            ("Write", Some("TODO"), None),
        ]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "dep-1".to_string());
        doc.entries[1]
            .properties
            .insert("BLOCKER".to_string(), "ids(\"dep-1\")".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        assert!(blocking_details(&ctx).is_empty());
    }

    #[test]
    fn details_with_explicit_condition() {
        let mut doc = make_doc_with_entries(&[
            ("Dep", Some("WAITING"), None),
            ("Blocked", Some("TODO"), None),
        ]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "dep-1".to_string());
        doc.entries[1].properties.insert(
            "BLOCKER".to_string(),
            "ids(\"dep-1\") todo-state?(\"DONE\")".to_string(),
        );

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 1,
        };
        let details = blocking_details(&ctx);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].title, "Dep");
        assert_eq!(details[0].condition_desc, "must have state DONE");
    }

    #[test]
    fn details_multiple_blockers() {
        let mut doc = make_doc_with_entries(&[
            ("Task A", Some("TODO"), None),
            ("Task B", Some("TODO"), None),
            ("Blocked", Some("TODO"), None),
        ]);
        doc.entries[0]
            .properties
            .insert("ID".to_string(), "a".to_string());
        doc.entries[1]
            .properties
            .insert("ID".to_string(), "b".to_string());
        doc.entries[2]
            .properties
            .insert("BLOCKER".to_string(), "ids(\"a\" \"b\")".to_string());

        let docs: Vec<&OrgDocument> = vec![&doc];
        let ctx = EdnaContext {
            all_docs: &docs,
            doc: &doc,
            entry_idx: 2,
        };
        let details = blocking_details(&ctx);
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].title, "Task A");
        assert_eq!(details[1].title, "Task B");
    }
}
