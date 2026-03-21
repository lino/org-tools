// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

/// Parsed components of an org heading line.
///
/// Spec: [§2.2 Headlines](https://orgmode.org/manual/Headlines.html),
/// [Syntax: Headlines](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections)
///
/// A heading line has the form:
/// `STARS KEYWORD PRIORITY TITLE TAGS`
/// where STARS is `*+` at column 0, KEYWORD is an optional TODO state,
/// PRIORITY is `[#X]`, TITLE is the text, and TAGS is `:tag1:tag2:` at EOL.
#[derive(Debug, PartialEq)]
pub struct HeadingParts<'a> {
    /// Number of leading stars (1 = top-level).
    pub level: usize,
    /// TODO keyword if present (e.g., `"TODO"`, `"DONE"`).
    pub keyword: Option<&'a str>,
    /// Priority cookie character if present (e.g., `'A'`).
    pub priority: Option<char>,
    /// Heading title text with keyword, priority, and tags stripped.
    pub title: &'a str,
    /// Tag strings without the surrounding colons (e.g., `["work", "urgent"]`).
    pub tags: Vec<&'a str>,
}

/// Default TODO keywords recognized when no `#+TODO:` settings are present.
pub const DEFAULT_TODO_KEYWORDS: &[&str] = &[
    "TODO",
    "DONE",
    "NEXT",
    "WAITING",
    "HOLD",
    "CANCELLED",
    "CANCELED",
    "STARTED",
    "DELEGATED",
    "REVIEW",
    "DRAFT",
    "PUBLISHED",
];

/// Parsed TODO keyword configuration from `#+TODO:` / `#+SEQ_TODO:` / `#+TYP_TODO:`.
///
/// Keywords before the `|` separator are "not done" states; keywords after are
/// "done" states. If no `|` appears, the last keyword is treated as the done state
/// (matching Emacs behaviour).
///
/// Spec: [§5.2 TODO Extensions](https://orgmode.org/manual/TODO-Extensions.html)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoKeywords {
    /// Keywords representing "not done" states (e.g., `["TODO", "NEXT"]`).
    pub todo: Vec<String>,
    /// Keywords representing "done" states (e.g., `["DONE", "CANCELLED"]`).
    pub done: Vec<String>,
}

impl TodoKeywords {
    /// All keywords (todo + done) in a single list.
    pub fn all(&self) -> Vec<&str> {
        self.todo
            .iter()
            .chain(self.done.iter())
            .map(|s| s.as_str())
            .collect()
    }

    /// Returns true if `word` is a recognized TODO keyword (either state).
    pub fn contains(&self, word: &str) -> bool {
        self.todo.iter().any(|k| k == word) || self.done.iter().any(|k| k == word)
    }

    /// Returns true if `word` is a "done" keyword.
    pub fn is_done(&self, word: &str) -> bool {
        self.done.iter().any(|k| k == word)
    }
}

impl Default for TodoKeywords {
    fn default() -> Self {
        Self {
            todo: vec![
                "TODO".into(),
                "NEXT".into(),
                "WAITING".into(),
                "HOLD".into(),
                "STARTED".into(),
                "DELEGATED".into(),
                "REVIEW".into(),
                "DRAFT".into(),
            ],
            done: vec![
                "DONE".into(),
                "CANCELLED".into(),
                "CANCELED".into(),
                "PUBLISHED".into(),
            ],
        }
    }
}

/// Parse a `#+TODO:` / `#+SEQ_TODO:` / `#+TYP_TODO:` value into [`TodoKeywords`].
///
/// Format: `KW1 KW2 | KW3 KW4` where `|` separates not-done from done states.
/// Keywords may have fast-access chars in parens: `TODO(t)` — the `(t)` is stripped.
/// If no `|` is present, the last keyword is the done state.
pub fn parse_todo_spec(spec: &str) -> TodoKeywords {
    let spec = spec.trim();
    if spec.is_empty() {
        return TodoKeywords::default();
    }

    let (before_pipe, after_pipe) = if let Some(pos) = spec.find('|') {
        (&spec[..pos], Some(&spec[pos + 1..]))
    } else {
        (spec, None)
    };

    let strip_fast_key = |w: &str| -> String {
        // Strip "(x)" fast-access suffix: "TODO(t)" → "TODO".
        if let Some(paren) = w.find('(') {
            w[..paren].to_string()
        } else {
            w.to_string()
        }
    };

    let before: Vec<String> = before_pipe
        .split_whitespace()
        .map(strip_fast_key)
        .filter(|s| !s.is_empty())
        .collect();

    if let Some(after) = after_pipe {
        let done: Vec<String> = after
            .split_whitespace()
            .map(strip_fast_key)
            .filter(|s| !s.is_empty())
            .collect();
        TodoKeywords { todo: before, done }
    } else if before.len() > 1 {
        // No pipe: last keyword is the done state.
        let mut todo = before;
        let done = vec![todo.pop().unwrap()];
        TodoKeywords { todo, done }
    } else {
        // Single keyword, treat as todo.
        TodoKeywords {
            todo: before,
            done: Vec::new(),
        }
    }
}

/// Build a [`TodoKeywords`] from file keywords in an [`OrgDocument`].
///
/// Checks `#+TODO:`, `#+SEQ_TODO:`, and `#+TYP_TODO:` (in that order).
/// Returns the default set if none are found.
pub fn todo_keywords_from_file(
    file_keywords: &std::collections::HashMap<String, String>,
) -> TodoKeywords {
    for key in &["TODO", "SEQ_TODO", "TYP_TODO"] {
        if let Some(val) = file_keywords.get(*key) {
            return parse_todo_spec(val);
        }
    }
    TodoKeywords::default()
}

/// Priority range configuration from `#+PRIORITIES:`.
///
/// Format: `#+PRIORITIES: HIGHEST LOWEST [DEFAULT]`
/// where HIGHEST < LOWEST alphabetically (A < C means A is highest priority).
///
/// Spec: [§5.4 Priorities](https://orgmode.org/manual/Priorities.html)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorityRange {
    /// Highest priority letter (default: `'A'`).
    pub highest: char,
    /// Lowest priority letter (default: `'C'`).
    pub lowest: char,
    /// Default priority letter (default: `'B'`).
    pub default: char,
}

impl Default for PriorityRange {
    fn default() -> Self {
        Self {
            highest: 'A',
            lowest: 'C',
            default: 'B',
        }
    }
}

impl PriorityRange {
    /// Returns true if `ch` is within the valid priority range.
    pub fn is_valid(&self, ch: char) -> bool {
        let ch = ch.to_ascii_uppercase();
        ch >= self.highest && ch <= self.lowest
    }
}

/// Parse a `#+PRIORITIES:` value into a [`PriorityRange`].
///
/// Format: `HIGHEST LOWEST [DEFAULT]` (e.g., `A E B`).
pub fn parse_priority_spec(spec: &str) -> PriorityRange {
    let parts: Vec<&str> = spec.split_whitespace().collect();
    match parts.len() {
        2 => {
            let h = parts[0].chars().next().unwrap_or('A').to_ascii_uppercase();
            let l = parts[1].chars().next().unwrap_or('C').to_ascii_uppercase();
            PriorityRange {
                highest: h,
                lowest: l,
                default: h, // Emacs defaults to highest when no default given.
            }
        }
        3.. => {
            let h = parts[0].chars().next().unwrap_or('A').to_ascii_uppercase();
            let l = parts[1].chars().next().unwrap_or('C').to_ascii_uppercase();
            let d = parts[2].chars().next().unwrap_or('B').to_ascii_uppercase();
            PriorityRange {
                highest: h,
                lowest: l,
                default: d,
            }
        }
        _ => PriorityRange::default(),
    }
}

/// Build a [`PriorityRange`] from file keywords.
pub fn priority_range_from_file(
    file_keywords: &std::collections::HashMap<String, String>,
) -> PriorityRange {
    if let Some(val) = file_keywords.get("PRIORITIES") {
        parse_priority_spec(val)
    } else {
        PriorityRange::default()
    }
}

// ---------------------------------------------------------------------------
// Tag specification (#+TAGS:)
// ---------------------------------------------------------------------------

/// A single tag definition from `#+TAGS:`.
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDef {
    /// Tag name (e.g., `"@work"`, `"laptop"`).
    pub name: String,
    /// Optional single-character fast-access key (e.g., `'w'` from `@work(w)`).
    pub fast_key: Option<char>,
}

/// A member of a tag group — either a literal tag or a regex pattern.
///
/// Regex members use `{PATTERN}` syntax inside group brackets and match any
/// tag whose name satisfies the pattern.
#[derive(Debug, Clone)]
pub enum TagMember {
    /// A literal tag definition with optional fast-access key.
    Literal(TagDef),
    /// A regex pattern matching multiple tags (e.g., `{P@.+}`).
    Pattern(regex::Regex),
}

/// A group of tags, optionally mutually exclusive.
///
/// Curly braces `{ ... }` define mutually exclusive groups; square brackets
/// `[ Group : member1 member2 ]` define hierarchical groups (group tags).
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
#[derive(Debug, Clone)]
pub struct TagGroup {
    /// Group tag name (for hierarchy groups `[ Group : ... ]`). `None` for
    /// plain mutually exclusive groups `{ ... }`.
    pub group_tag: Option<TagDef>,
    /// Member tags or patterns within the group.
    pub members: Vec<TagMember>,
    /// `true` for `{ }` (mutually exclusive), `false` for `[ ]` (hierarchy).
    pub exclusive: bool,
}

/// Parsed `#+TAGS:` configuration for a file.
///
/// Built from one or more `#+TAGS:` lines (additive). When no `#+TAGS:` lines
/// are present, `allow_any` is `true` and no validation is performed.
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
#[derive(Debug, Clone)]
pub struct TagSpec {
    /// Tag groups (mutually exclusive or hierarchical).
    pub groups: Vec<TagGroup>,
    /// Standalone tags not belonging to any group.
    pub standalone: Vec<TagDef>,
    /// When `true`, any tag is valid (no `#+TAGS:` declared or value is empty).
    pub allow_any: bool,
}

impl Default for TagSpec {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            standalone: Vec::new(),
            allow_any: true,
        }
    }
}

impl TagSpec {
    /// Returns `true` if `tag` is declared (matches a literal name or a regex pattern).
    pub fn matches_tag(&self, tag: &str) -> bool {
        if self.allow_any {
            return true;
        }
        // Check standalone tags.
        if self.standalone.iter().any(|t| t.name == tag) {
            return true;
        }
        // Check group tags and members.
        for group in &self.groups {
            if let Some(ref gt) = group.group_tag {
                if gt.name == tag {
                    return true;
                }
            }
            for member in &group.members {
                match member {
                    TagMember::Literal(def) => {
                        if def.name == tag {
                            return true;
                        }
                    }
                    TagMember::Pattern(re) => {
                        if re.is_match(tag) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

/// Parse a tag token, stripping optional fast-access key suffix `(x)`.
fn parse_tag_def(token: &str) -> TagDef {
    if let Some(paren) = token.find('(') {
        let name = token[..paren].to_string();
        let fast_key = token[paren + 1..].chars().next().filter(|c| *c != ')');
        TagDef { name, fast_key }
    } else {
        TagDef {
            name: token.to_string(),
            fast_key: None,
        }
    }
}

/// Parse a member token inside a group. Tokens wrapped in `{...}` are regex
/// patterns; everything else is a literal tag.
fn parse_group_member(token: &str) -> TagMember {
    if token.starts_with('{') && token.ends_with('}') && token.len() > 2 {
        let pattern = &token[1..token.len() - 1];
        // Anchor the pattern to match the full tag name.
        let anchored = format!("^(?:{pattern})$");
        match regex::Regex::new(&anchored) {
            Ok(re) => TagMember::Pattern(re),
            // Fall back to literal if regex is invalid.
            Err(_) => TagMember::Literal(parse_tag_def(token)),
        }
    } else {
        TagMember::Literal(parse_tag_def(token))
    }
}

/// Parse one or more `#+TAGS:` values into a [`TagSpec`].
///
/// Multiple values are combined additively. An empty slice or a single empty
/// string results in `allow_any = true`.
///
/// Supported syntax:
/// - Simple tags: `@work @home laptop`
/// - Fast-access keys: `@work(w) @home(h)`
/// - Mutually exclusive groups: `{ @work @home }`
/// - Hierarchy groups: `[ Project : SubA SubB ]`
/// - Regex members: `[ Project : {P@.+} ]`
/// - Line-break markers `\\n` (ignored)
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
pub fn parse_tags_spec(values: &[&str]) -> TagSpec {
    if values.is_empty() {
        return TagSpec::default();
    }

    // If all values are empty/whitespace, allow any tag.
    let all_empty = values.iter().all(|v| v.trim().is_empty());
    if all_empty {
        return TagSpec {
            allow_any: true,
            ..TagSpec::default()
        };
    }

    // Concatenate all values into a single token stream.
    let combined: String = values.join(" ");
    let tokens: Vec<&str> = combined.split_whitespace().collect();

    let mut groups = Vec::new();
    let mut standalone = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i];

        // Skip line-break markers.
        if token == "\\n" {
            i += 1;
            continue;
        }

        // Mutually exclusive group: { tag1 tag2 ... }
        if token == "{" {
            i += 1;
            let mut members = Vec::new();
            while i < tokens.len() && tokens[i] != "}" {
                if tokens[i] == ":" || tokens[i] == "\\n" {
                    i += 1;
                    continue;
                }
                members.push(parse_group_member(tokens[i]));
                i += 1;
            }
            if i < tokens.len() {
                i += 1; // skip "}"
            }
            groups.push(TagGroup {
                group_tag: None,
                members,
                exclusive: true,
            });
            continue;
        }

        // Hierarchy group: [ GroupTag : member1 member2 ... ]
        if token == "[" {
            i += 1;
            // First token after "[" is the group tag.
            let group_tag = if i < tokens.len() && tokens[i] != ":" && tokens[i] != "]" {
                let gt = parse_tag_def(tokens[i]);
                i += 1;
                Some(gt)
            } else {
                None
            };
            // Skip the colon separator.
            if i < tokens.len() && tokens[i] == ":" {
                i += 1;
            }
            let mut members = Vec::new();
            while i < tokens.len() && tokens[i] != "]" {
                if tokens[i] == "\\n" {
                    i += 1;
                    continue;
                }
                members.push(parse_group_member(tokens[i]));
                i += 1;
            }
            if i < tokens.len() {
                i += 1; // skip "]"
            }
            groups.push(TagGroup {
                group_tag,
                members,
                exclusive: false,
            });
            continue;
        }

        // Standalone tag.
        standalone.push(parse_tag_def(token));
        i += 1;
    }

    TagSpec {
        groups,
        standalone,
        allow_any: false,
    }
}

/// Build a [`TagSpec`] from collected `#+TAGS:` values.
pub fn tag_spec_from_values(values: &[String]) -> TagSpec {
    if values.is_empty() {
        return TagSpec::default();
    }
    let refs: Vec<&str> = values.iter().map(|s| s.as_str()).collect();
    parse_tags_spec(&refs)
}

/// Returns the heading level (number of stars) if the line is a heading.
pub fn heading_level(line: &str) -> Option<usize> {
    if !line.starts_with('*') {
        return None;
    }
    let stars = line.len() - line.trim_start_matches('*').len();
    let after = &line[stars..];
    if after.is_empty() || after.starts_with(' ') {
        Some(stars)
    } else {
        None
    }
}

/// Returns true if the line is an org heading.
pub fn is_heading(line: &str) -> bool {
    heading_level(line).is_some()
}

/// Parses a heading line into its components using the default keyword set.
pub fn parse_heading(line: &str) -> Option<HeadingParts<'_>> {
    parse_heading_with_keywords(line, DEFAULT_TODO_KEYWORDS)
}

/// Parses a heading line using a custom set of TODO keywords.
pub fn parse_heading_with_keywords<'a>(
    line: &'a str,
    keywords: &[&str],
) -> Option<HeadingParts<'a>> {
    let level = heading_level(line)?;
    let rest = line[level..].trim_start();

    // Extract tags from the end of the line.
    let (rest_no_tags, tags) = extract_tags(rest);
    let rest = rest_no_tags.trim_end();

    // Extract TODO keyword.
    let (rest, keyword) = extract_keyword_from(rest, keywords);

    // Extract priority.
    let (rest, priority) = extract_priority(rest);

    let title = rest.trim();

    Some(HeadingParts {
        level,
        keyword,
        priority,
        title,
        tags,
    })
}

/// Extracts tags from the end of a heading text. Returns (text_without_tags, tags).
fn extract_tags(text: &str) -> (&str, Vec<&str>) {
    let trimmed = text.trim_end();
    if !trimmed.ends_with(':') {
        return (text, Vec::new());
    }

    // Find the start of the tag string (last ` :` sequence).
    if let Some(tag_start) = trimmed.rfind(" :") {
        let tag_str = &trimmed[tag_start + 1..];
        // Validate: tags are `:word:word:` — each segment between colons is alphanumeric/underscore/@.
        let inner = tag_str.trim_start_matches(':').trim_end_matches(':');
        if inner.is_empty() {
            return (text, Vec::new());
        }
        let parts: Vec<&str> = inner.split(':').collect();
        let all_valid = parts.iter().all(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '@')
        });
        if all_valid {
            return (&trimmed[..tag_start], parts);
        }
    }

    (text, Vec::new())
}

/// Extracts a TODO keyword from the start of heading text using a keyword set.
fn extract_keyword_from<'a>(text: &'a str, keywords: &[&str]) -> (&'a str, Option<&'a str>) {
    let first_word_end = text.find(' ').unwrap_or(text.len());
    let first_word = &text[..first_word_end];
    if keywords.contains(&first_word) {
        let rest = if first_word_end < text.len() {
            text[first_word_end..].trim_start()
        } else {
            ""
        };
        (rest, Some(first_word))
    } else {
        (text, None)
    }
}

/// Extracts a priority cookie `[#X]` from the start of text.
fn extract_priority(text: &str) -> (&str, Option<char>) {
    if text.len() >= 4 && text.starts_with("[#") && text.as_bytes()[3] == b']' {
        let ch = text.as_bytes()[2] as char;
        if ch.is_ascii_alphabetic() {
            let rest = if text.len() > 4 {
                text[4..].trim_start()
            } else {
                ""
            };
            return (rest, Some(ch));
        }
    }
    (text, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_heading() {
        let parts = parse_heading("* Hello").unwrap();
        assert_eq!(parts.level, 1);
        assert_eq!(parts.keyword, None);
        assert_eq!(parts.priority, None);
        assert_eq!(parts.title, "Hello");
        assert!(parts.tags.is_empty());
    }

    #[test]
    fn heading_with_todo() {
        let parts = parse_heading("** TODO Write tests").unwrap();
        assert_eq!(parts.level, 2);
        assert_eq!(parts.keyword, Some("TODO"));
        assert_eq!(parts.title, "Write tests");
    }

    #[test]
    fn heading_with_priority() {
        let parts = parse_heading("* TODO [#A] Urgent task").unwrap();
        assert_eq!(parts.keyword, Some("TODO"));
        assert_eq!(parts.priority, Some('A'));
        assert_eq!(parts.title, "Urgent task");
    }

    #[test]
    fn heading_with_tags() {
        let parts = parse_heading("* Heading :tag1:tag2:").unwrap();
        assert_eq!(parts.title, "Heading");
        assert_eq!(parts.tags, vec!["tag1", "tag2"]);
    }

    #[test]
    fn heading_with_everything() {
        let parts = parse_heading("*** DONE [#B] Complete task :work:done:").unwrap();
        assert_eq!(parts.level, 3);
        assert_eq!(parts.keyword, Some("DONE"));
        assert_eq!(parts.priority, Some('B'));
        assert_eq!(parts.title, "Complete task");
        assert_eq!(parts.tags, vec!["work", "done"]);
    }

    #[test]
    fn not_a_heading() {
        assert!(parse_heading("not a heading").is_none());
        assert!(parse_heading("*bold text*").is_none());
    }

    #[test]
    fn heading_level() {
        assert_eq!(super::heading_level("* H"), Some(1));
        assert_eq!(super::heading_level("*** H"), Some(3));
        assert_eq!(super::heading_level("*not"), None);
    }

    #[test]
    fn empty_heading() {
        let parts = parse_heading("*").unwrap();
        assert_eq!(parts.level, 1);
        assert_eq!(parts.title, "");
    }

    #[test]
    fn heading_with_priority_only() {
        let parts = parse_heading("* [#C]").unwrap();
        assert_eq!(parts.priority, Some('C'));
        assert_eq!(parts.title, "");
    }

    // --- parse_todo_spec ---

    #[test]
    fn todo_spec_with_pipe() {
        let kw = parse_todo_spec("TODO NEXT | DONE CANCELLED");
        assert_eq!(kw.todo, vec!["TODO", "NEXT"]);
        assert_eq!(kw.done, vec!["DONE", "CANCELLED"]);
    }

    #[test]
    fn todo_spec_without_pipe() {
        // Last keyword becomes the done state.
        let kw = parse_todo_spec("OPEN CLOSED");
        assert_eq!(kw.todo, vec!["OPEN"]);
        assert_eq!(kw.done, vec!["CLOSED"]);
    }

    #[test]
    fn todo_spec_with_fast_keys() {
        let kw = parse_todo_spec("TODO(t) NEXT(n) | DONE(d) CANCELLED(c)");
        assert_eq!(kw.todo, vec!["TODO", "NEXT"]);
        assert_eq!(kw.done, vec!["DONE", "CANCELLED"]);
    }

    #[test]
    fn todo_spec_single_keyword() {
        let kw = parse_todo_spec("WAITING");
        assert_eq!(kw.todo, vec!["WAITING"]);
        assert!(kw.done.is_empty());
    }

    #[test]
    fn todo_spec_empty() {
        let kw = parse_todo_spec("");
        assert_eq!(kw, TodoKeywords::default());
    }

    #[test]
    fn todo_keywords_contains_and_done() {
        let kw = parse_todo_spec("OPEN IN_PROGRESS | DONE WONTFIX");
        assert!(kw.contains("OPEN"));
        assert!(kw.contains("DONE"));
        assert!(kw.contains("WONTFIX"));
        assert!(!kw.contains("TODO"));
        assert!(!kw.is_done("OPEN"));
        assert!(kw.is_done("DONE"));
        assert!(kw.is_done("WONTFIX"));
    }

    // --- parse_heading_with_keywords ---

    #[test]
    fn custom_keyword_recognized() {
        let parts = parse_heading_with_keywords("* OPEN Fix the bug", &["OPEN", "CLOSED"]).unwrap();
        assert_eq!(parts.keyword, Some("OPEN"));
        assert_eq!(parts.title, "Fix the bug");
    }

    #[test]
    fn default_keyword_not_in_custom_set() {
        // "TODO" is not in the custom set, so it becomes part of the title.
        let parts = parse_heading_with_keywords("* TODO Fix the bug", &["OPEN", "CLOSED"]).unwrap();
        assert_eq!(parts.keyword, None);
        assert_eq!(parts.title, "TODO Fix the bug");
    }

    // --- todo_keywords_from_file ---

    #[test]
    fn from_file_keywords_todo() {
        let mut kw = std::collections::HashMap::new();
        kw.insert("TODO".to_string(), "OPEN | DONE WONTFIX".to_string());
        let result = todo_keywords_from_file(&kw);
        assert_eq!(result.todo, vec!["OPEN"]);
        assert_eq!(result.done, vec!["DONE", "WONTFIX"]);
    }

    #[test]
    fn from_file_keywords_seq_todo() {
        let mut kw = std::collections::HashMap::new();
        kw.insert(
            "SEQ_TODO".to_string(),
            "DRAFT REVIEW | PUBLISHED".to_string(),
        );
        let result = todo_keywords_from_file(&kw);
        assert_eq!(result.todo, vec!["DRAFT", "REVIEW"]);
        assert_eq!(result.done, vec!["PUBLISHED"]);
    }

    #[test]
    fn from_file_keywords_default() {
        let kw = std::collections::HashMap::new();
        let result = todo_keywords_from_file(&kw);
        assert_eq!(result, TodoKeywords::default());
    }

    // --- parse_priority_spec ---

    #[test]
    fn priority_spec_three_values() {
        let pr = parse_priority_spec("A E B");
        assert_eq!(pr.highest, 'A');
        assert_eq!(pr.lowest, 'E');
        assert_eq!(pr.default, 'B');
    }

    #[test]
    fn priority_spec_two_values() {
        let pr = parse_priority_spec("A D");
        assert_eq!(pr.highest, 'A');
        assert_eq!(pr.lowest, 'D');
    }

    #[test]
    fn priority_spec_default() {
        let pr = parse_priority_spec("");
        assert_eq!(pr, PriorityRange::default());
    }

    #[test]
    fn priority_range_valid() {
        let pr = parse_priority_spec("A E B");
        assert!(pr.is_valid('A'));
        assert!(pr.is_valid('C'));
        assert!(pr.is_valid('E'));
        assert!(!pr.is_valid('F'));
        assert!(!pr.is_valid('Z'));
    }

    #[test]
    fn priority_range_from_file_kw() {
        let mut kw = std::collections::HashMap::new();
        kw.insert("PRIORITIES".to_string(), "A F C".to_string());
        let pr = priority_range_from_file(&kw);
        assert_eq!(pr.highest, 'A');
        assert_eq!(pr.lowest, 'F');
        assert_eq!(pr.default, 'C');
    }

    // --- parse_tags_spec ---

    #[test]
    fn tags_spec_empty() {
        let spec = parse_tags_spec(&[]);
        assert!(spec.allow_any);
    }

    #[test]
    fn tags_spec_empty_value() {
        let spec = parse_tags_spec(&[""]);
        assert!(spec.allow_any);
    }

    #[test]
    fn tags_spec_simple_list() {
        let spec = parse_tags_spec(&["@work @home laptop"]);
        assert!(!spec.allow_any);
        assert_eq!(spec.standalone.len(), 3);
        assert_eq!(spec.standalone[0].name, "@work");
        assert_eq!(spec.standalone[1].name, "@home");
        assert_eq!(spec.standalone[2].name, "laptop");
    }

    #[test]
    fn tags_spec_fast_access_keys() {
        let spec = parse_tags_spec(&["@work(w) @home(h) laptop(l)"]);
        assert_eq!(spec.standalone[0].name, "@work");
        assert_eq!(spec.standalone[0].fast_key, Some('w'));
        assert_eq!(spec.standalone[1].fast_key, Some('h'));
        assert_eq!(spec.standalone[2].fast_key, Some('l'));
    }

    #[test]
    fn tags_spec_mutually_exclusive() {
        let spec = parse_tags_spec(&["{ @work @home } laptop"]);
        assert_eq!(spec.groups.len(), 1);
        assert!(spec.groups[0].exclusive);
        assert!(spec.groups[0].group_tag.is_none());
        assert_eq!(spec.groups[0].members.len(), 2);
        assert_eq!(spec.standalone.len(), 1);
        assert_eq!(spec.standalone[0].name, "laptop");
    }

    #[test]
    fn tags_spec_hierarchy_group() {
        let spec = parse_tags_spec(&["[ Project : SubA SubB ]"]);
        assert_eq!(spec.groups.len(), 1);
        assert!(!spec.groups[0].exclusive);
        let gt = spec.groups[0].group_tag.as_ref().unwrap();
        assert_eq!(gt.name, "Project");
        assert_eq!(spec.groups[0].members.len(), 2);
    }

    #[test]
    fn tags_spec_regex_member() {
        let spec = parse_tags_spec(&["[ Project : {P@.+} ]"]);
        assert_eq!(spec.groups.len(), 1);
        assert!(spec.matches_tag("Project"));
        assert!(spec.matches_tag("P@frontend"));
        assert!(spec.matches_tag("P@backend"));
        assert!(!spec.matches_tag("frontend"));
    }

    #[test]
    fn tags_spec_multiple_lines() {
        let spec = parse_tags_spec(&["@work(w) @home(h)", "laptop(l) pc(p)"]);
        assert_eq!(spec.standalone.len(), 4);
        assert!(spec.matches_tag("@work"));
        assert!(spec.matches_tag("pc"));
    }

    #[test]
    fn tags_spec_line_break_marker() {
        let spec = parse_tags_spec(&["@work \\n laptop"]);
        assert_eq!(spec.standalone.len(), 2);
        assert!(spec.matches_tag("@work"));
        assert!(spec.matches_tag("laptop"));
    }

    #[test]
    fn tags_spec_matches_standalone() {
        let spec = parse_tags_spec(&["@work @home laptop"]);
        assert!(spec.matches_tag("@work"));
        assert!(spec.matches_tag("laptop"));
        assert!(!spec.matches_tag("unknown"));
    }

    #[test]
    fn tags_spec_matches_group_members() {
        let spec = parse_tags_spec(&["{ @work @home } laptop"]);
        assert!(spec.matches_tag("@work"));
        assert!(spec.matches_tag("@home"));
        assert!(spec.matches_tag("laptop"));
        assert!(!spec.matches_tag("phone"));
    }

    #[test]
    fn tags_spec_matches_group_tag() {
        let spec = parse_tags_spec(&["[ Context : @Office @Remote ]"]);
        assert!(spec.matches_tag("Context"));
        assert!(spec.matches_tag("@Office"));
        assert!(spec.matches_tag("@Remote"));
        assert!(!spec.matches_tag("@Home"));
    }

    #[test]
    fn tags_spec_allow_any_matches_everything() {
        let spec = TagSpec::default();
        assert!(spec.matches_tag("anything"));
        assert!(spec.matches_tag("@work"));
    }

    #[test]
    fn tags_spec_case_sensitive() {
        let spec = parse_tags_spec(&["Work"]);
        assert!(spec.matches_tag("Work"));
        assert!(!spec.matches_tag("work"));
        assert!(!spec.matches_tag("WORK"));
    }

    #[test]
    fn tag_spec_from_values_helper() {
        let vals = vec!["@work @home".to_string(), "laptop".to_string()];
        let spec = tag_spec_from_values(&vals);
        assert!(!spec.allow_any);
        assert!(spec.matches_tag("@work"));
        assert!(spec.matches_tag("laptop"));
    }
}
