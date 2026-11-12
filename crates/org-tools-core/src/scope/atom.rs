// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Atomic scope constraints, token parsing, and glob matching.

use std::fmt;

use serde::{Deserialize, Serialize};

// region Data Types

/// Atomic scope constraint parsed from `:SCOPE:` or `- scope ::` metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScopeAtom {
    /// Universally accessible unless `strict_scope` is enabled.
    Global,
    /// Explicit project slug (e.g. `project:payments-api` or bare `payments-api`).
    Project(String),
    /// Project landscape or cluster slug (e.g. `landscape:platform`).
    Landscape(String),
    /// Repository forge path glob pattern (e.g. `forge:github.com/acme/*`).
    Forge(String),
    /// Effective tag match constraint (e.g. `tag:security`).
    Tag(String),
    /// Negated scope exclusion (e.g. `!project:legacy-billing`).
    Exclude(Box<ScopeAtom>),
}

impl fmt::Display for ScopeAtom {
    /// Formats the scope atom into its canonical prefixed string representation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => write!(f, "global"),
            Self::Project(s) => write!(f, "project:{s}"),
            Self::Landscape(s) => write!(f, "landscape:{s}"),
            Self::Forge(s) => write!(f, "forge:{s}"),
            Self::Tag(s) => write!(f, "tag:{s}"),
            Self::Exclude(inner) => write!(f, "!{inner}"),
        }
    }
}

// endregion

// region Token Parsing

/// Parses comma-separated values for plural prefix forms.
fn parse_comma_separated(
    rest: &str,
    constructor: fn(String) -> ScopeAtom,
    out: &mut Vec<ScopeAtom>,
) {
    for s in rest.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push(constructor(s.to_string()));
    }
}

/// Parses prefixed atoms (`projects:`, `landscapes:`, `project:`, `landscape:`, `forge:`, `tag:`).
fn parse_prefixed_token(lower: &str, raw: &str, out: &mut Vec<ScopeAtom>) -> bool {
    if lower == "global" {
        out.push(ScopeAtom::Global);
        return true;
    }
    if let Some(rest) = lower.strip_prefix("projects:") {
        parse_comma_separated(rest, ScopeAtom::Project, out);
        return true;
    }
    if let Some(rest) = lower.strip_prefix("project:") {
        let t = rest.trim();
        if !t.is_empty() {
            out.push(ScopeAtom::Project(t.to_string()));
        }
        return true;
    }
    if let Some(rest) = lower.strip_prefix("landscapes:") {
        parse_comma_separated(rest, ScopeAtom::Landscape, out);
        return true;
    }
    if let Some(rest) = lower.strip_prefix("landscape:") {
        let t = rest.trim();
        if !t.is_empty() {
            out.push(ScopeAtom::Landscape(t.to_string()));
        }
        return true;
    }
    if let Some(rest) = raw
        .strip_prefix("forge:")
        .or_else(|| raw.strip_prefix("FORGE:"))
    {
        let t = rest.trim();
        if !t.is_empty() {
            out.push(ScopeAtom::Forge(t.to_string()));
        }
        return true;
    }
    if let Some(rest) = lower.strip_prefix("tag:") {
        let t = rest.trim();
        if !t.is_empty() {
            out.push(ScopeAtom::Tag(t.to_string()));
        }
        return true;
    }
    false
}

/// Parses a single raw scope token into one or more canonical [`ScopeAtom`] values.
pub fn parse_atom_token(token: &str, out: &mut Vec<ScopeAtom>) {
    if let Some(inner_token) = token.strip_prefix('!') {
        let mut inner = Vec::new();
        parse_atom_token(inner_token, &mut inner);
        for a in inner {
            out.push(ScopeAtom::Exclude(Box::new(a)));
        }
        return;
    }
    let lower = token.to_ascii_lowercase();
    if !parse_prefixed_token(&lower, token, out) && !token.trim().is_empty() {
        out.push(ScopeAtom::Project(lower));
    }
}

/// Parses raw scope text into a list of [`ScopeAtom`]s, defaulting to `[Global]` when empty.
pub fn parse_scope(raw: &str) -> Vec<ScopeAtom> {
    let mut out = Vec::new();
    for part in raw.split('|') {
        for token in part.split_whitespace() {
            parse_atom_token(token, &mut out);
        }
    }
    if out.is_empty() {
        out.push(ScopeAtom::Global);
    }
    out
}

/// Resolves raw `:SCOPE:` drawer property or `- scope ::` vulpea-meta fallback text.
pub fn extract_scope_drawer_or_meta(drawer_val: Option<&str>, body: &str) -> String {
    if let Some(v) = drawer_val.map(str::trim).filter(|s| !s.is_empty()) {
        return v.to_string();
    }
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('-').map(str::trim_start) {
            if let Some((k, v)) = rest.split_once("::") {
                if k.trim().eq_ignore_ascii_case("scope") && !v.trim().is_empty() {
                    return v.trim().to_string();
                }
            }
        }
    }
    "global".to_string()
}

// endregion

// region Glob Matching

/// Evaluates case-insensitive glob patterns containing `*` and `**` against `path`.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let mut regex_str = String::from("(?i)^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&['*', '*', '/']) {
            regex_str.push_str("(?:.*/)?");
            i += 3;
        } else if chars[i..].starts_with(&['*', '*']) {
            regex_str.push_str(".*");
            i += 2;
        } else if chars[i] == '*' {
            regex_str.push_str("[^/]*");
            i += 1;
        } else {
            let c = chars[i];
            if matches!(
                c,
                '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' | '|'
            ) {
                regex_str.push('\\');
            }
            regex_str.push(c);
            i += 1;
        }
    }
    regex_str.push('$');
    regex::Regex::new(&regex_str).is_ok_and(|re| re.is_match(path))
}

// endregion
