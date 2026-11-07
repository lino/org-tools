// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=830 reason="Cohesive org-edna AST, tokenizer, recursive-descent parser, and canonical formatter"

//! AST definitions, tokenizer, parser, and formatter for the org-edna mini-language.
//!
//! Spec: <https://www.nongnu.org/org-edna-el/>

// region Public Types

/// A single expression in the org-edna mini-language.
#[derive(Debug, Clone, PartialEq)]
pub enum EdnaExpr {
    /// A finder — selects target entries into the active scope.
    Finder(Finder),
    /// An action — modifies target entries (only evaluated in `:TRIGGER:`).
    Action(Action),
    /// A condition — boolean check on target entries.
    Condition(Condition),
    /// Conditional block: `if COND then EXPRS [else EXPRS] endif`.
    If {
        /// Condition expression tested against the active scope.
        cond: Box<EdnaExpr>,
        /// Expressions executed when `cond` evaluates to `true`.
        then: Vec<EdnaExpr>,
        /// Expressions executed when `cond` evaluates to `false`.
        else_: Vec<EdnaExpr>,
    },
}

/// A finder selects target entries for subsequent actions or conditions.
#[derive(Debug, Clone, PartialEq)]
pub enum Finder {
    /// `ids(UUID ...)` — look up entries or level-0 file nodes by `:ID:` property.
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
    /// `siblings` — all sibling headings excluding `self`.
    Siblings,
    /// `first-child` — the first direct child heading.
    FirstChild,
    /// `match("TAG_SPEC")` — entries matching a tag specification.
    Match(String),
    /// `olp(["FILE",] "HEADING" ...)` — outline path lookup.
    Olp(Vec<String>),
    /// `file("FILENAME")` — entries in a relative or workspace file.
    File(String),
    /// `org-file("FILENAME")` — entries in a file searched across workspace paths.
    OrgFile(String),
    /// `relatives(...)` — generic relative finder with options.
    Relatives(Vec<String>),
}

/// An action modifies target entries (suffix `!`).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `todo!("STATE")` — change TODO keyword.
    Todo(String),
    /// `scheduled!("TIMESPEC")` — set or clear `SCHEDULED:` timestamp.
    Scheduled(String),
    /// `deadline!("TIMESPEC")` — set or clear `DEADLINE:` timestamp.
    Deadline(String),
    /// `set-property!("KEY" "VALUE")` — set a drawer property (`F005`-aligned).
    SetProperty(String, String),
    /// `delete-property!("KEY")` — delete a drawer property.
    DeleteProperty(String),
    /// `tag!("TAG")` — set heading tags (`F007`-aligned).
    Tag(String),
    /// `set-priority!("PRIORITY")` — set or clear priority cookie.
    SetPriority(String),
    /// `set-effort!("EFFORT")` — set `:EFFORT:` property.
    SetEffort(String),
    /// `archive!` — archive the target entry subtree.
    Archive,
    /// `clock-in!` — start the clock.
    ClockIn,
    /// `clock-out!` — stop the clock.
    ClockOut,
    /// `chain!("PROPERTY")` — copy `PROPERTY` from source entry to target entry.
    Chain(String),
}

/// A condition tests target entries (suffix `?`, optionally negated with leading `!`).
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// `done?` — target entry is in a done TODO state.
    Done,
    /// `!cond?` — boolean negation of an inner condition.
    Negated(Box<Condition>),
    /// `todo-state?("KEYWORD")` — entry has specific TODO keyword.
    TodoState(String),
    /// `has-property?("KEY" "VALUE")` — entry has property equal to value.
    HasProperty(String, String),
    /// `re-search?("REGEX")` — entry subtree text matches regex.
    ReSearch(String),
    /// `variable-set?("VAR" "VALUE")` — Emacs variable check (conservatively true).
    VariableSet(String, String),
    /// `has-tags?("TAG" ...)` — entry has all listed inherited/local tags.
    HasTags(Vec<String>),
}

/// Syntax error encountered while parsing an org-edna expression string.
#[derive(Debug, Clone, PartialEq)]
pub struct EdnaParseError {
    /// Human-readable error message.
    pub message: String,
    /// Byte offset into the source property value where the error occurred.
    pub offset: usize,
}

// endregion

// region Tokenizer

/// Lexical token produced while scanning an org-edna property value.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// Unquoted keyword or bare-word argument.
    Word(String),
    /// Double-quoted string literal with `\"` and `\\` unescaped.
    Quoted(String),
    /// Opening parenthesis `(`.
    LParen,
    /// Closing parenthesis `)`.
    RParen,
}

/// Byte-cursor lexical scanner for org-edna expressions.
struct Tokenizer<'a> {
    /// Raw property string slice.
    input: &'a str,
    /// Current byte cursor in `input`.
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    /// Creates a new tokenizer positioned at byte `0` of `input`.
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    /// Advances `pos` past ASCII whitespace bytes.
    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Scans a double-quoted string literal, unescaping `\\` and `\"` while preserving regex escapes.
    fn read_quoted(&mut self, start: usize) -> (Token, usize) {
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.input.len() {
            let rest = &self.input[self.pos..];
            let Some(ch) = rest.chars().next() else {
                break;
            };
            self.pos += ch.len_utf8();
            if ch == '"' {
                break;
            }
            if ch == '\\' && self.pos < self.input.len() {
                if let Some(next_ch) = self.input[self.pos..].chars().next() {
                    self.pos += next_ch.len_utf8();
                    if next_ch == '\\' || next_ch == '"' {
                        out.push(next_ch);
                    } else {
                        out.push('\\');
                        out.push(next_ch);
                    }
                    continue;
                }
            }
            out.push(ch);
        }
        (Token::Quoted(out), start)
    }

    /// Scans the next token along with its starting byte offset.
    fn next_token(&mut self) -> Option<(Token, usize)> {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return None;
        }
        let start = self.pos;
        match self.input.as_bytes()[self.pos] {
            b'(' => {
                self.pos += 1;
                Some((Token::LParen, start))
            }
            b')' => {
                self.pos += 1;
                Some((Token::RParen, start))
            }
            b'"' => Some(self.read_quoted(start)),
            _ => {
                while self.pos < self.input.len() {
                    let b = self.input.as_bytes()[self.pos];
                    if b.is_ascii_whitespace() || b == b'(' || b == b')' {
                        break;
                    }
                    self.pos += 1;
                }
                Some((Token::Word(self.input[start..self.pos].to_string()), start))
            }
        }
    }

    /// Tokenizes the entire input string into `(Token, byte_offset)` pairs.
    fn tokenize_all(&mut self) -> Vec<(Token, usize)> {
        let mut tokens = Vec::new();
        while let Some(tok) = self.next_token() {
            tokens.push(tok);
        }
        tokens
    }
}

// endregion

// region Recursive-Descent Parser

/// Token-stream parser producing [`EdnaExpr`] AST nodes.
struct Parser {
    /// Pre-tokenized input stream.
    tokens: Vec<(Token, usize)>,
    /// Current token index.
    pos: usize,
}

impl Parser {
    /// Constructs a parser over `tokens`.
    fn new(tokens: Vec<(Token, usize)>) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Peeks at the current token without consuming it.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    /// Returns the byte offset of the current token, or `0` at end of stream.
    fn peek_offset(&self) -> usize {
        self.tokens.get(self.pos).map_or(0, |(_, o)| *o)
    }

    /// Consumes and returns the current token and its offset.
    fn advance(&mut self) -> Option<(Token, usize)> {
        let tok = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        Some(tok)
    }

    /// Returns `true` when the next token is `(`.
    fn has_args(&self) -> bool {
        matches!(self.peek(), Some(Token::LParen))
    }

    /// Consumes `(` or returns a syntax error.
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

    /// Reads parenthesized string/word arguments up to the matching `)`.
    fn read_args(&mut self) -> Result<Vec<String>, EdnaParseError> {
        self.expect_lparen()?;
        let mut args = Vec::new();
        loop {
            match self.peek() {
                Some(Token::RParen) => {
                    self.advance();
                    return Ok(args);
                }
                Some(Token::Quoted(_) | Token::Word(_)) => {
                    if let Some((Token::Quoted(s) | Token::Word(s), _)) = self.advance() {
                        args.push(s);
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

    /// Reads exactly one argument for `name` or reports an arity error at `offset`.
    fn read_one_arg(&mut self, name: &str, offset: usize) -> Result<String, EdnaParseError> {
        let mut args = self.read_args()?;
        if args.len() != 1 {
            return Err(EdnaParseError {
                message: format!("{name}() requires exactly one argument"),
                offset,
            });
        }
        Ok(args.remove(0))
    }

    /// Reads exactly two arguments for `name` or reports an arity error at `offset`.
    fn read_two_args(
        &mut self,
        name: &str,
        offset: usize,
    ) -> Result<(String, String), EdnaParseError> {
        let mut args = self.read_args()?;
        if args.len() != 2 {
            return Err(EdnaParseError {
                message: format!("{name}() requires exactly two arguments"),
                offset,
            });
        }
        let second = args.pop().unwrap();
        let first = args.pop().unwrap();
        Ok((first, second))
    }

    /// Reads one or more arguments for `name` or reports an arity error at `offset`.
    fn read_non_empty_args(
        &mut self,
        name: &str,
        offset: usize,
    ) -> Result<Vec<String>, EdnaParseError> {
        let args = self.read_args()?;
        if args.is_empty() {
            return Err(EdnaParseError {
                message: format!("{name}() requires at least one argument"),
                offset,
            });
        }
        Ok(args)
    }

    /// Parses one top-level or nested expression.
    fn parse_expr(&mut self) -> Result<EdnaExpr, EdnaParseError> {
        let (tok, offset) = self.advance().ok_or_else(|| EdnaParseError {
            message: "unexpected end of input".to_string(),
            offset: 0,
        })?;
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
        if lower == "if" {
            self.parse_if_block()
        } else if lower.ends_with('!') {
            self.parse_action(&lower, offset)
        } else if lower.ends_with('?') {
            Ok(EdnaExpr::Condition(self.parse_condition(&lower, offset)?))
        } else {
            self.parse_finder(&lower, offset)
        }
    }

    /// Parses a zero-argument sibling or self finder if `keyword` matches one.
    fn parse_sibling_finder(keyword: &str) -> Option<Finder> {
        match keyword {
            "self" => Some(Finder::Self_),
            "next-sibling" => Some(Finder::NextSibling),
            "previous-sibling" => Some(Finder::PreviousSibling),
            "rest-of-siblings" | "rest-of-siblings-wrap" => Some(Finder::RestOfSiblings),
            "siblings" => Some(Finder::Siblings),
            _ => None,
        }
    }

    /// Parses a zero-argument hierarchy finder if `keyword` matches one.
    fn parse_hierarchy_finder(keyword: &str) -> Option<Finder> {
        match keyword {
            "parent" => Some(Finder::Parent),
            "ancestors" => Some(Finder::Ancestors),
            "children" => Some(Finder::Children),
            "descendants" => Some(Finder::Descendants),
            "first-child" => Some(Finder::FirstChild),
            _ => None,
        }
    }

    /// Parses a finder expression.
    fn parse_finder(&mut self, keyword: &str, offset: usize) -> Result<EdnaExpr, EdnaParseError> {
        if let Some(f) =
            Self::parse_sibling_finder(keyword).or_else(|| Self::parse_hierarchy_finder(keyword))
        {
            return Ok(EdnaExpr::Finder(f));
        }
        let finder = match keyword {
            "ids" => Finder::Ids(self.read_non_empty_args("ids", offset)?),
            "match" => Finder::Match(self.read_one_arg("match", offset)?),
            "olp" => Finder::Olp(self.read_non_empty_args("olp", offset)?),
            "file" => Finder::File(self.read_one_arg("file", offset)?),
            "org-file" => Finder::OrgFile(self.read_one_arg("org-file", offset)?),
            "relatives" => Finder::Relatives(self.read_args()?),
            _ => {
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

    /// Parses a zero-argument action (`archive!`, `clock-in!`, `clock-out!`).
    fn parse_zero_arg_action(keyword: &str) -> Option<Action> {
        match keyword {
            "archive!" => Some(Action::Archive),
            "clock-in!" => Some(Action::ClockIn),
            "clock-out!" => Some(Action::ClockOut),
            _ => None,
        }
    }

    /// Parses property/effort/chain actions (`set-property!`, `delete-property!`, `set-effort!`, `chain!`).
    fn parse_property_action(
        &mut self,
        keyword: &str,
        offset: usize,
    ) -> Result<Option<Action>, EdnaParseError> {
        let act = match keyword {
            "set-property!" => {
                let (k, v) = self.read_two_args("set-property!", offset)?;
                Action::SetProperty(k, v)
            }
            "delete-property!" => {
                Action::DeleteProperty(self.read_one_arg("delete-property!", offset)?)
            }
            "set-effort!" => Action::SetEffort(self.read_one_arg("set-effort!", offset)?),
            "chain!" => Action::Chain(self.read_one_arg("chain!", offset)?),
            _ => return Ok(None),
        };
        Ok(Some(act))
    }

    /// Parses an action expression suffixed with `!`.
    fn parse_action(&mut self, keyword: &str, offset: usize) -> Result<EdnaExpr, EdnaParseError> {
        if let Some(a) = Self::parse_zero_arg_action(keyword) {
            return Ok(EdnaExpr::Action(a));
        }
        if let Some(a) = self.parse_property_action(keyword, offset)? {
            return Ok(EdnaExpr::Action(a));
        }
        let action = match keyword {
            "todo!" => Action::Todo(self.read_one_arg("todo!", offset)?),
            "scheduled!" => Action::Scheduled(self.read_one_arg("scheduled!", offset)?),
            "deadline!" => Action::Deadline(self.read_one_arg("deadline!", offset)?),
            "tag!" => Action::Tag(self.read_one_arg("tag!", offset)?),
            "set-priority!" => Action::SetPriority(self.read_one_arg("set-priority!", offset)?),
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

    /// Parses a condition expression suffixed with `?`, supporting leading `!` negation.
    fn parse_condition(
        &mut self,
        keyword: &str,
        offset: usize,
    ) -> Result<Condition, EdnaParseError> {
        if let Some(stripped) = keyword.strip_prefix('!') {
            if stripped.is_empty() || stripped == "?" {
                return Err(EdnaParseError {
                    message: format!("unknown edna condition: {keyword}"),
                    offset,
                });
            }
            let inner = self.parse_condition(stripped, offset)?;
            return Ok(Condition::Negated(Box::new(inner)));
        }
        self.parse_positive_condition(keyword, offset)
    }

    /// Parses a non-negated condition keyword and its arguments.
    fn parse_positive_condition(
        &mut self,
        keyword: &str,
        offset: usize,
    ) -> Result<Condition, EdnaParseError> {
        match keyword {
            "done?" => {
                if self.has_args() && !self.read_args()?.is_empty() {
                    return Err(EdnaParseError {
                        message: "done?() takes no arguments".to_string(),
                        offset,
                    });
                }
                Ok(Condition::Done)
            }
            "todo-state?" => Ok(Condition::TodoState(
                self.read_one_arg("todo-state?", offset)?,
            )),
            "has-property?" => {
                let (k, v) = self.read_two_args("has-property?", offset)?;
                Ok(Condition::HasProperty(k, v))
            }
            "re-search?" => Ok(Condition::ReSearch(
                self.read_one_arg("re-search?", offset)?,
            )),
            "variable-set?" => {
                let (k, v) = self.read_two_args("variable-set?", offset)?;
                Ok(Condition::VariableSet(k, v))
            }
            "has-tags?" => Ok(Condition::HasTags(
                self.read_non_empty_args("has-tags?", offset)?,
            )),
            _ => {
                if self.has_args() {
                    let _ = self.read_args()?;
                }
                Err(EdnaParseError {
                    message: format!("unknown edna condition: {keyword}"),
                    offset,
                })
            }
        }
    }

    /// Consumes the mandatory `then` keyword after an `if` condition.
    fn expect_then_keyword(&mut self) -> Result<(), EdnaParseError> {
        match self.advance() {
            Some((Token::Word(w), _)) if w.eq_ignore_ascii_case("then") => Ok(()),
            Some((_, off)) => Err(EdnaParseError {
                message: "expected 'then' after if condition".to_string(),
                offset: off,
            }),
            None => Err(EdnaParseError {
                message: "unexpected end of input, expected 'then'".to_string(),
                offset: 0,
            }),
        }
    }

    /// Parses an `if COND then EXPRS [else EXPRS] endif` block.
    fn parse_if_block(&mut self) -> Result<EdnaExpr, EdnaParseError> {
        let cond = Box::new(self.parse_expr()?);
        self.expect_then_keyword()?;

        let mut then = Vec::new();
        let mut else_ = Vec::new();
        let mut in_else = false;
        loop {
            match self.peek() {
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("else") => {
                    self.advance();
                    in_else = true;
                }
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("endif") => {
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

// endregion

// region Public API

/// Parses an org-edna property value into AST expressions and syntax errors.
///
/// Parsing is lenient and continues past errors to surface all issues on a line.
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
                parser.pos += 1;
            }
        }
    }
    (exprs, errors)
}

/// Appends `ids(...)` arguments from `exprs` (including `if` branches) into `out`.
pub(crate) fn collect_ids_from_exprs(exprs: &[EdnaExpr], out: &mut Vec<String>) {
    for expr in exprs {
        match expr {
            EdnaExpr::Finder(Finder::Ids(found)) => out.extend(found.iter().cloned()),
            EdnaExpr::If { cond, then, else_ } => {
                collect_ids_from_exprs(std::slice::from_ref(cond), out);
                collect_ids_from_exprs(then, out);
                collect_ids_from_exprs(else_, out);
            }
            _ => {}
        }
    }
}

/// Extracts all referenced `:ID:` values from `ids(...)` finders in `input`.
pub fn extract_dependency_ids(input: &str) -> Vec<String> {
    let (exprs, _) = parse_edna(input);
    let mut ids = Vec::new();
    collect_ids_from_exprs(&exprs, &mut ids);
    ids
}

/// Escapes `\` and `"` inside a double-quoted edna argument.
fn quote_arg(raw: &str) -> String {
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Formats a list of arguments as `"a" "b"`.
fn quote_args(args: &[String]) -> String {
    args.iter()
        .map(|s| quote_arg(s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns the canonical keyword for a zero-argument structural [`Finder`], if applicable.
fn zero_arg_finder_keyword(finder: &Finder) -> Option<&'static str> {
    match finder {
        Finder::Self_ => Some("self"),
        Finder::NextSibling => Some("next-sibling"),
        Finder::PreviousSibling => Some("previous-sibling"),
        Finder::RestOfSiblings => Some("rest-of-siblings"),
        Finder::Parent => Some("parent"),
        Finder::Ancestors => Some("ancestors"),
        _ => zero_arg_child_finder_keyword(finder),
    }
}

/// Returns the canonical keyword for child/sibling structural [`Finder`] variants.
fn zero_arg_child_finder_keyword(finder: &Finder) -> Option<&'static str> {
    match finder {
        Finder::Children => Some("children"),
        Finder::Descendants => Some("descendants"),
        Finder::Siblings => Some("siblings"),
        Finder::FirstChild => Some("first-child"),
        _ => None,
    }
}

/// Formats a [`Finder`] back into canonical org-edna syntax.
fn format_finder(finder: &Finder) -> String {
    if let Some(kw) = zero_arg_finder_keyword(finder) {
        return kw.to_string();
    }
    match finder {
        Finder::Ids(ids) => format!("ids({})", quote_args(ids)),
        Finder::Match(m) => format!("match({})", quote_arg(m)),
        Finder::Olp(args) => format!("olp({})", quote_args(args)),
        Finder::File(f) => format!("file({})", quote_arg(f)),
        Finder::OrgFile(f) => format!("org-file({})", quote_arg(f)),
        Finder::Relatives(args) => format!("relatives({})", quote_args(args)),
        _ => String::new(),
    }
}

/// Returns the canonical keyword for a zero-argument [`Action`], if applicable.
fn zero_arg_action_keyword(action: &Action) -> Option<&'static str> {
    match action {
        Action::Archive => Some("archive!"),
        Action::ClockIn => Some("clock-in!"),
        Action::ClockOut => Some("clock-out!"),
        _ => None,
    }
}

/// Formats property, effort, or chain [`Action`] variants when applicable.
fn format_property_action(action: &Action) -> Option<String> {
    match action {
        Action::SetProperty(k, v) => {
            Some(format!("set-property!({} {})", quote_arg(k), quote_arg(v)))
        }
        Action::DeleteProperty(k) => Some(format!("delete-property!({})", quote_arg(k))),
        Action::SetEffort(e) => Some(format!("set-effort!({})", quote_arg(e))),
        Action::Chain(p) => Some(format!("chain!({})", quote_arg(p))),
        _ => None,
    }
}

/// Formats an [`Action`] back into canonical org-edna syntax.
fn format_action(action: &Action) -> String {
    if let Some(kw) = zero_arg_action_keyword(action) {
        return kw.to_string();
    }
    if let Some(prop_str) = format_property_action(action) {
        return prop_str;
    }
    match action {
        Action::Todo(s) => format!("todo!({})", quote_arg(s)),
        Action::Scheduled(s) => format!("scheduled!({})", quote_arg(s)),
        Action::Deadline(s) => format!("deadline!({})", quote_arg(s)),
        Action::Tag(t) => format!("tag!({})", quote_arg(t)),
        Action::SetPriority(p) => format!("set-priority!({})", quote_arg(p)),
        _ => String::new(),
    }
}

/// Formats a [`Condition`] back into canonical org-edna syntax.
fn format_condition(cond: &Condition) -> String {
    match cond {
        Condition::Done => "done?".to_string(),
        Condition::Negated(inner) => format!("!{}", format_condition(inner)),
        Condition::TodoState(s) => format!("todo-state?({})", quote_arg(s)),
        Condition::HasProperty(k, v) => {
            format!("has-property?({} {})", quote_arg(k), quote_arg(v))
        }
        Condition::ReSearch(r) => format!("re-search?({})", quote_arg(r)),
        Condition::VariableSet(k, v) => {
            format!("variable-set?({} {})", quote_arg(k), quote_arg(v))
        }
        Condition::HasTags(tags) => format!("has-tags?({})", quote_args(tags)),
    }
}

/// Formats a slice of [`EdnaExpr`] nodes into space-separated org-edna syntax.
pub fn format_edna_exprs(exprs: &[EdnaExpr]) -> String {
    exprs
        .iter()
        .map(|e| match e {
            EdnaExpr::Finder(f) => format_finder(f),
            EdnaExpr::Action(a) => format_action(a),
            EdnaExpr::Condition(c) => format_condition(c),
            EdnaExpr::If { cond, then, else_ } => {
                let c_str = format_edna_exprs(std::slice::from_ref(cond));
                let t_str = format_edna_exprs(then);
                if else_.is_empty() {
                    format!("if {c_str} then {t_str} endif")
                } else {
                    format!(
                        "if {c_str} then {t_str} else {} endif",
                        format_edna_exprs(else_)
                    )
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// endregion
