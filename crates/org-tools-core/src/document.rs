// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=1820 reason="Cohesive OrgDocument and OrgEntry single-pass parser with comprehensive behavioural unit test suite"

//! Lightweight heading tree model for org-mode documents.
//!
//! [`OrgDocument`] is built from a [`SourceFile`] using a single-pass line scan.
//! It is not a full AST — just enough structure for querying, clocking,
//! plain-list checklists, `org-roam` v2 file-level nodes, and locator resolution.
//!
//! Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html),
//! [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html),
//! [Syntax: Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::entry::parse_checklist_item;
pub use crate::entry::{CheckboxState, ChecklistItem, ChecklistSummary};
use crate::locator::FILE_NODE_ENTRY_INDEX;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::heading::{
    parse_heading, parse_heading_with_keywords, priority_range_from_file, tag_spec_from_values,
    todo_keywords_from_file, HeadingParts, PriorityRange, TagSpec, TodoKeywords,
};
use crate::rules::timestamp::{parse_timestamp, OrgTimestamp};
use crate::source::SourceFile;

// region Public Types

/// Built-in org-mode link schemes recognized by [`OrgDocument::is_known_link_scheme`].
const BUILTIN_LINK_SCHEMES: &[&str] = &[
    "file", "id", "http", "https", "mailto", "ftp", "news", "shell", "elisp", "doi", "info",
    "docview", "bbdb", "gnus", "irc", "mhe", "rmail", "help",
];

/// A parsed org document with heading tree structure and file-level metadata.
#[derive(Debug)]
pub struct OrgDocument {
    /// Path to the source file.
    pub file: PathBuf,
    /// All heading entries in document order.
    pub entries: Vec<OrgEntry>,
    /// File-level properties (from a top-of-file `:PROPERTIES:` drawer before any heading or keyword).
    pub file_properties: HashMap<String, String>,
    /// 1-based line number of the top-of-file `:PROPERTIES:` drawer, if present.
    pub file_properties_line: Option<usize>,
    /// File-level keywords (`#+KEY: value` before any heading).
    pub file_keywords: HashMap<String, String>,
    /// TODO keyword configuration parsed from `#+TODO:` / `#+SEQ_TODO:` / `#+TYP_TODO:`.
    pub todo_keywords: TodoKeywords,
    /// Priority range parsed from `#+PRIORITIES:`.
    pub priority_range: PriorityRange,
    /// File-level tags from `#+FILETAGS:` (inherited by all entries).
    pub filetags: Vec<String>,
    /// File-wide default properties from `#+PROPERTY:` keyword lines (including `KEY+` continuations).
    ///
    /// Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
    pub default_properties: HashMap<String, String>,
    /// Tag specification parsed from `#+TAGS:` keyword lines.
    pub tag_spec: TagSpec,
    /// Custom link abbreviations from `#+LINK:` keyword lines.
    ///
    /// Maps prefix to URL template (e.g., `"github"` → `"https://github.com/"`).
    /// Multiple `#+LINK:` lines are additive.
    ///
    /// Spec: [§4.5 Using Links Outside Org](https://orgmode.org/manual/Using-Links-Outside-Org.html)
    pub link_abbreviations: HashMap<String, String>,
    /// Named constants from `#+CONSTANTS:` keyword lines.
    ///
    /// Maps constant name to value string (e.g., `"pi"` → `"3.14159"`).
    /// Multiple `#+CONSTANTS:` lines are additive. Used in table formulas
    /// as `$name` references.
    ///
    /// Spec: [§3.5.2 References](https://orgmode.org/manual/References.html)
    pub table_constants: HashMap<String, String>,
}

/// A single heading entry in an org document.
#[derive(Debug)]
pub struct OrgEntry {
    /// Heading level (1 = top-level `*`).
    pub level: usize,
    /// TODO keyword if present (e.g., `"TODO"`, `"DONE"`).
    pub keyword: Option<String>,
    /// Priority cookie character if present (e.g., `'A'`).
    pub priority: Option<char>,
    /// Heading title text (keyword, priority, and tags stripped).
    pub title: String,
    /// Local tags on this heading (not inherited).
    pub tags: Vec<String>,
    /// Properties from the `:PROPERTIES:` drawer.
    pub properties: HashMap<String, String>,
    /// Planning timestamps (SCHEDULED, DEADLINE, CLOSED).
    pub planning: Planning,
    /// Clock entries from `:LOGBOOK:` or standalone `CLOCK:` lines.
    pub clocks: Vec<ClockEntry>,
    /// Plain-list checklist items in this entry's own body (excluding child headings and protected blocks).
    ///
    /// Spec: [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html)
    pub checklist: Vec<ChecklistItem>,
    /// Line number of the heading (1-based).
    pub heading_line: usize,
    /// Byte offset of the heading line start.
    pub heading_offset: usize,
    /// Line number where this entry's content ends (exclusive, 1-based).
    pub content_end_line: usize,
    /// Index of parent entry in [`OrgDocument::entries`], or `None` for top-level.
    pub parent: Option<usize>,
    /// Indices of direct children in [`OrgDocument::entries`].
    pub children: Vec<usize>,
    /// The original heading line text (for `--format=org` output).
    pub raw_heading: String,
}

/// Planning timestamps attached to a heading.
#[derive(Debug, Default)]
pub struct Planning {
    /// `SCHEDULED:` timestamp.
    pub scheduled: Option<OrgTimestamp>,
    /// `DEADLINE:` timestamp.
    pub deadline: Option<OrgTimestamp>,
    /// `CLOSED:` timestamp.
    pub closed: Option<OrgTimestamp>,
}

/// A single `CLOCK:` entry.
#[derive(Debug)]
pub struct ClockEntry {
    /// Clock start timestamp.
    pub start: OrgTimestamp,
    /// Clock end timestamp (`None` for a running clock).
    pub end: Option<OrgTimestamp>,
    /// Computed duration in minutes (from `=> HH:MM`).
    pub duration_minutes: Option<i64>,
    /// Line number of this `CLOCK:` line (1-based).
    pub line: usize,
}

// endregion

// region Public API

impl OrgDocument {
    /// Build a document from a [`SourceFile`] using single-pass line-based parsing.
    pub fn from_source(source: &SourceFile) -> Self {
        let lines: Vec<&str> = source.content.split('\n').collect();
        let protected = protected_regions(&source.content);
        let mut preamble = PreambleAccumulator::default();
        let mut i = scan_preamble_section(&lines, &protected, &mut preamble);
        let built = preamble.take_and_finalize(&source.path);

        let scan_ctx = EntryScanContext {
            source,
            lines: &lines,
            protected: &protected,
            default_properties: &built.default_properties,
        };
        let kw_strs: Vec<String> = built
            .todo_keywords
            .all()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let kw_refs: Vec<&str> = kw_strs.iter().map(String::as_str).collect();

        let mut entries = Vec::new();
        let mut level_stack = Vec::new();
        while i < lines.len() {
            if !is_protected(i, &protected) {
                if let Some(heading) = parse_heading_with_keywords(lines[i], &kw_refs) {
                    let (entry, next_i) =
                        parse_heading_entry(&scan_ctx, i, heading, &mut entries, &mut level_stack);
                    entries.push(entry);
                    i = next_i;
                    continue;
                }
            }
            i += 1;
        }

        assign_content_end_lines(&mut entries, lines.len());
        built.into_document(source.path.clone(), entries)
    }

    /// Returns the top-of-file `:ID:` property value if present and non-empty.
    pub fn file_id(&self) -> Option<&str> {
        lookup_case_insensitive(&self.file_properties, "ID").filter(|s| !s.is_empty())
    }

    /// Returns the `#+TITLE:` keyword value if present and non-empty.
    pub fn file_title(&self) -> Option<&str> {
        self.file_keywords
            .get("TITLE")
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Returns the `#+TITLE:` keyword value, falling back to the UTF-8 file stem.
    pub fn file_title_or_stem(&self) -> String {
        if let Some(title) = self.file_title() {
            return title.to_string();
        }
        self.file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string()
    }

    /// Returns `true` when the top-of-file `:PROPERTIES:` drawer has `:ID:` matching `id`.
    pub fn has_file_id(&self, id: &str) -> bool {
        self.file_id() == Some(id)
    }

    /// Returns `true` when either the file-level `:ID:` or any heading `:ID:` matches `id`.
    pub fn contains_id(&self, id: &str) -> bool {
        self.has_file_id(id) || self.find_by_id(id).is_some()
    }

    /// Find a heading entry index by its `:ID:` property.
    pub fn find_by_id(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| lookup_case_insensitive(&e.properties, "ID") == Some(id))
    }

    /// Find a heading entry index by its `:CUSTOM_ID:` property.
    pub fn find_by_custom_id(&self, custom_id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| lookup_case_insensitive(&e.properties, "CUSTOM_ID") == Some(custom_id))
    }

    /// Get the outline path for an entry (ancestor titles from root to entry).
    pub fn outline_path(&self, entry_idx: usize) -> Vec<&str> {
        let mut path = Vec::new();
        let mut idx = Some(entry_idx);
        while let Some(i) = idx {
            let Some(entry) = self.entries.get(i) else {
                break;
            };
            path.push(entry.title.as_str());
            idx = entry.parent;
        }
        path.reverse();
        path
    }

    /// Get all inherited tags for an entry (local + ancestor + `#+FILETAGS:`).
    pub fn inherited_tags(&self, entry_idx: usize) -> Vec<&str> {
        let mut tags = Vec::new();
        let mut idx = Some(entry_idx);
        while let Some(i) = idx {
            let Some(entry) = self.entries.get(i) else {
                break;
            };
            for tag in &entry.tags {
                if !tags.contains(&tag.as_str()) {
                    tags.push(tag.as_str());
                }
            }
            idx = entry.parent;
        }
        for tag in &self.filetags {
            if !tags.contains(&tag.as_str()) {
                tags.push(tag.as_str());
            }
        }
        tags
    }

    /// Returns `true` if `scheme` is a known link scheme (built-in or custom via `#+LINK:`).
    pub fn is_known_link_scheme(&self, scheme: &str) -> bool {
        BUILTIN_LINK_SCHEMES
            .iter()
            .any(|b| b.eq_ignore_ascii_case(scheme))
            || self.link_abbreviations.contains_key(scheme)
    }

    /// Look up a property on an entry (or file node), falling back to `#+PROPERTY:` defaults.
    pub fn property(&self, entry_idx: usize, key: &str) -> Option<&str> {
        if let Some(entry) = self.entries.get(entry_idx) {
            return lookup_case_insensitive(&entry.properties, key)
                .or_else(|| lookup_case_insensitive(&self.default_properties, key));
        }
        if entry_idx == FILE_NODE_ENTRY_INDEX {
            return lookup_case_insensitive(&self.file_properties, key)
                .or_else(|| lookup_case_insensitive(&self.default_properties, key));
        }
        None
    }
}

impl OrgEntry {
    /// Computes aggregate counts across this entry's direct plain-list [`ChecklistItem`]s.
    ///
    /// Counts only checklist items in this entry's own body section before child headings.
    pub fn checklist_summary(&self) -> ChecklistSummary {
        ChecklistSummary::from_items(&self.checklist)
    }
}

// endregion

// region Preamble & Property Parsing

/// Mutable state collected from the preamble before the first heading.
#[derive(Default)]
struct PreambleAccumulator {
    /// Properties extracted from a valid top-of-file `:PROPERTIES:` drawer.
    file_properties: HashMap<String, String>,
    /// 1-based start line of the top-of-file `:PROPERTIES:` drawer.
    file_properties_line: Option<usize>,
    /// In-buffer `#+KEY: value` settings.
    file_keywords: HashMap<String, String>,
    /// Additive `#+TAGS:` values in document order.
    tags_lines: Vec<String>,
    /// Additive `#+PROPERTY:` values in document order.
    property_lines: Vec<String>,
    /// Additive `#+LINK:` prefix → URL template mappings.
    link_abbreviations: HashMap<String, String>,
    /// Additive `#+CONSTANTS:` name → value mappings.
    table_constants: HashMap<String, String>,
}

/// Derived preamble configuration ready to populate [`OrgDocument`].
struct FinalizedPreamble {
    /// File-level property drawer map.
    file_properties: HashMap<String, String>,
    /// 1-based line number of the file-level `:PROPERTIES:` drawer.
    file_properties_line: Option<usize>,
    /// File-level keyword map.
    file_keywords: HashMap<String, String>,
    /// Effective TODO keyword sequences.
    todo_keywords: TodoKeywords,
    /// Effective priority cookie bounds.
    priority_range: PriorityRange,
    /// Inherited file-level tags.
    filetags: Vec<String>,
    /// Merged file-wide `#+PROPERTY:` defaults.
    default_properties: HashMap<String, String>,
    /// Effective `#+TAGS:` specification.
    tag_spec: TagSpec,
    /// Effective `#+LINK:` abbreviations.
    link_abbreviations: HashMap<String, String>,
    /// Effective `#+CONSTANTS:` table definitions.
    table_constants: HashMap<String, String>,
}

impl FinalizedPreamble {
    /// Assembles an [`OrgDocument`] from finalized preamble metadata and parsed `entries`.
    fn into_document(self, file: PathBuf, entries: Vec<OrgEntry>) -> OrgDocument {
        OrgDocument {
            file,
            entries,
            file_properties: self.file_properties,
            file_properties_line: self.file_properties_line,
            file_keywords: self.file_keywords,
            todo_keywords: self.todo_keywords,
            priority_range: self.priority_range,
            filetags: self.filetags,
            default_properties: self.default_properties,
            tag_spec: self.tag_spec,
            link_abbreviations: self.link_abbreviations,
            table_constants: self.table_constants,
        }
    }
}

/// Scans preamble lines prior to the first unprotected heading and returns that heading's index.
fn scan_preamble_section(
    lines: &[&str],
    protected: &[(usize, usize)],
    preamble: &mut PreambleAccumulator,
) -> usize {
    let mut i = 0;
    while i < lines.len() {
        if is_protected(i, protected) {
            i += 1;
            continue;
        }
        if parse_heading(lines[i]).is_some() {
            break;
        }
        i = preamble.process_line(lines, i);
    }
    i
}

impl PreambleAccumulator {
    /// Processes a single preamble line at `index` and returns the next line index.
    fn process_line(&mut self, lines: &[&str], index: usize) -> usize {
        let line = lines[index];
        if let Some((key, val)) = parse_keyword_line(line) {
            self.record_keyword(key, val);
            return index + 1;
        }

        if line.trim().eq_ignore_ascii_case(":PROPERTIES:") {
            let empty_defaults = HashMap::new();
            if let Some((props, next_idx)) =
                parse_property_drawer(lines, index + 1, &empty_defaults)
            {
                if self.file_properties_line.is_none() && is_top_of_file_drawer(lines, index) {
                    self.file_properties = props;
                    self.file_properties_line = Some(index + 1);
                }
                return next_idx;
            }
        }

        index + 1
    }

    /// Records a single `#+KEY: value` line into the appropriate preamble collections.
    fn record_keyword(&mut self, key: String, val: String) {
        match key.as_str() {
            "TAGS" => self.tags_lines.push(val.clone()),
            "PROPERTY" => self.property_lines.push(val.clone()),
            "CONSTANTS" => parse_constants_into(&val, &mut self.table_constants, true),
            "LINK" => parse_link_abbreviation_into(&val, &mut self.link_abbreviations, true),
            _ => {}
        }
        self.file_keywords.insert(key, val);
    }

    /// Consumes accumulated preamble lines, merges `#+SETUPFILE:`, and derives document settings.
    fn take_and_finalize(&mut self, source_path: &Path) -> FinalizedPreamble {
        let mut taken = std::mem::take(self);
        merge_setupfile_settings(source_path, &mut taken);

        let todo_keywords = todo_keywords_from_file(&taken.file_keywords);
        let priority_range = priority_range_from_file(&taken.file_keywords);
        let filetags = parse_filetags(taken.file_keywords.get("FILETAGS"));
        let default_properties = parse_default_properties(&taken.property_lines);
        let tag_spec = tag_spec_from_values(&taken.tags_lines);

        FinalizedPreamble {
            file_properties: taken.file_properties,
            file_properties_line: taken.file_properties_line,
            file_keywords: taken.file_keywords,
            todo_keywords,
            priority_range,
            filetags,
            default_properties,
            tag_spec,
            link_abbreviations: taken.link_abbreviations,
            table_constants: taken.table_constants,
        }
    }
}

/// Returns `true` if `line` is a Worg-compliant org comment line (`#` followed by space, tab, or EOL).
///
/// Spec: [Syntax: Comment Lines](https://orgmode.org/worg/org-syntax.html#Comment_Lines)
fn is_org_comment_line(line: &str) -> bool {
    let trimmed = line.strip_suffix('\r').unwrap_or(line).trim_start();
    trimmed == "#" || trimmed.starts_with("# ") || trimmed.starts_with("#\t")
}

/// Returns `true` when `index` is at line 0 or preceded solely by `#` comment lines.
fn is_top_of_file_drawer(lines: &[&str], index: usize) -> bool {
    lines[..index].iter().copied().all(is_org_comment_line)
}

/// Looks up `key` in `map` by exact match first, falling back to ASCII case-insensitive match.
fn lookup_case_insensitive<'a>(map: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    if let Some(val) = map.get(key) {
        return Some(val.as_str());
    }
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Finds an existing map key matching `target_key` (exact first, then ASCII case-insensitive).
fn find_matching_key(props: &HashMap<String, String>, target_key: &str) -> Option<String> {
    if props.contains_key(target_key) {
        return Some(target_key.to_string());
    }
    props
        .keys()
        .find(|k| k.eq_ignore_ascii_case(target_key))
        .cloned()
}

/// Joins a base property value and an appended continuation value with a single space.
fn join_property_values(base: &str, addition: &str) -> String {
    if base.is_empty() {
        addition.to_string()
    } else if addition.is_empty() {
        base.to_string()
    } else {
        format!("{base} {addition}")
    }
}

/// Appends `value` onto `base_key` in `props`, seeding from `default_properties` when absent.
fn append_property_value(
    props: &mut HashMap<String, String>,
    default_properties: &HashMap<String, String>,
    base_key: &str,
    value: &str,
) {
    if base_key.is_empty() {
        return;
    }
    if let Some(existing_key) = find_matching_key(props, base_key) {
        if let Some(existing_val) = props.get_mut(&existing_key) {
            *existing_val = join_property_values(existing_val, value);
        }
        return;
    }

    let combined = match lookup_case_insensitive(default_properties, base_key) {
        Some(seed) => join_property_values(seed, value),
        None => value.to_string(),
    };
    props.insert(base_key.to_string(), combined);
}

/// Sets or overwrites `raw_key -> value` in `props`, replacing any differently-cased duplicate.
fn set_property_value(props: &mut HashMap<String, String>, raw_key: &str, value: &str) {
    if raw_key.is_empty() {
        return;
    }
    if let Some(old_k) = props
        .keys()
        .find(|k| k.as_str() != raw_key && k.eq_ignore_ascii_case(raw_key))
        .cloned()
    {
        props.remove(&old_k);
    }
    props.insert(raw_key.to_string(), value.to_string());
}

/// Inserts `raw_key -> value` or appends `" value"` when `raw_key` ends with `'+'` (`KEY+`).
///
/// Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
fn insert_or_append_property(
    props: &mut HashMap<String, String>,
    default_properties: &HashMap<String, String>,
    raw_key: &str,
    value: &str,
) {
    if let Some(base_key) = raw_key.strip_suffix('+') {
        append_property_value(props, default_properties, base_key, value);
    } else {
        set_property_value(props, raw_key, value);
    }
}

/// Parses a `:PROPERTIES:` drawer starting at `start` (the line after `:PROPERTIES:`).
///
/// Returns `Some((properties, next_line_index))` when closed by `:END:` before any
/// heading or EOF, or `None` when the drawer is unclosed.
fn parse_property_drawer(
    lines: &[&str],
    start: usize,
    default_properties: &HashMap<String, String>,
) -> Option<(HashMap<String, String>, usize)> {
    let mut props = HashMap::new();
    let mut i = start;

    while i < lines.len() {
        let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
        if parse_heading(raw).is_some() {
            return None;
        }
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            return Some((props, i + 1));
        }
        if let Some((key, val)) = parse_property_line(trimmed) {
            insert_or_append_property(&mut props, default_properties, &key, &val);
        }
        i += 1;
    }

    None
}

/// Parses a single `:KEY: value` property line inside a drawer.
fn parse_property_line(trimmed: &str) -> Option<(String, String)> {
    if !trimmed.starts_with(':') {
        return None;
    }
    let rest = &trimmed[1..];
    let colon_pos = rest.find(':')?;
    let key = rest[..colon_pos].to_string();
    if key.is_empty() {
        return None;
    }
    let val = rest[colon_pos + 1..].trim().to_string();
    Some((key, val))
}

/// Parses a `#+KEY: value` keyword line.
fn parse_keyword_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("#+") {
        return None;
    }
    let rest = &trimmed[2..];
    let colon_pos = rest.find(':')?;
    let key = rest[..colon_pos].trim().to_uppercase();
    let val = rest[colon_pos + 1..].trim().to_string();
    Some((key, val))
}

/// Parses a `#+FILETAGS:` value (`:tag1:tag2:`) into a list of tag strings.
fn parse_filetags(value: Option<&String>) -> Vec<String> {
    let Some(val) = value else {
        return Vec::new();
    };
    val.trim()
        .trim_matches(':')
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parses ordered `#+PROPERTY:` values (supporting multiple keys and `KEY+` continuations).
///
/// Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
fn parse_default_properties(property_lines: &[String]) -> HashMap<String, String> {
    let mut props = HashMap::new();
    let empty_defaults = HashMap::new();
    for raw in property_lines {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(space) = trimmed.find(|c: char| c.is_whitespace()) {
            let key = &trimmed[..space];
            let value = trimmed[space..].trim();
            insert_or_append_property(&mut props, &empty_defaults, key, value);
        } else {
            insert_or_append_property(&mut props, &empty_defaults, trimmed, "");
        }
    }
    props
}

/// Parses `name=value` pairs from a `#+CONSTANTS:` line into `target`.
fn parse_constants_into(val: &str, target: &mut HashMap<String, String>, overwrite: bool) {
    for pair in val.split_whitespace() {
        if let Some(eq) = pair.find('=') {
            let name = &pair[..eq];
            let value = &pair[eq + 1..];
            if !name.is_empty() && !value.is_empty() {
                if overwrite {
                    target.insert(name.to_string(), value.to_string());
                } else {
                    target
                        .entry(name.to_string())
                        .or_insert_with(|| value.to_string());
                }
            }
        }
    }
}

/// Parses a `<prefix> <template>` definition from a `#+LINK:` line into `target`.
fn parse_link_abbreviation_into(val: &str, target: &mut HashMap<String, String>, overwrite: bool) {
    if let Some(space) = val.find(|c: char| c.is_whitespace()) {
        let prefix = val[..space].to_string();
        let template = val[space..].trim().to_string();
        if !prefix.is_empty() && !template.is_empty() {
            if overwrite {
                target.insert(prefix, template);
            } else {
                target.entry(prefix).or_insert(template);
            }
        }
    }
}

/// Reads local `#+SETUPFILE:` content when configured and readable on disk.
fn read_local_setupfile(source_path: &Path, setupfile_val: &str) -> Option<String> {
    let trimmed = setupfile_val.trim();
    let path_str = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    if path_str.is_empty() || path_str.starts_with("http://") || path_str.starts_with("https://") {
        return None;
    }
    let base_dir = source_path.parent().unwrap_or(Path::new("."));
    std::fs::read_to_string(base_dir.join(path_str)).ok()
}

/// Loads preamble settings from a local `#+SETUPFILE:` as defaults beneath the main file.
fn merge_setupfile_settings(source_path: &Path, preamble: &mut PreambleAccumulator) {
    let Some(setup_val) = preamble.file_keywords.get("SETUPFILE").cloned() else {
        return;
    };
    let Some(content) = read_local_setupfile(source_path, &setup_val) else {
        return;
    };

    let mut setup_properties = Vec::new();
    for line in content.split('\n') {
        let raw = line.strip_suffix('\r').unwrap_or(line);
        if parse_heading(raw).is_some() {
            break;
        }
        let Some((key, val)) = parse_keyword_line(raw) else {
            continue;
        };
        match key.as_str() {
            "TAGS" => preamble.tags_lines.push(val),
            "PROPERTY" => setup_properties.push(val),
            "LINK" => parse_link_abbreviation_into(&val, &mut preamble.link_abbreviations, false),
            "CONSTANTS" => parse_constants_into(&val, &mut preamble.table_constants, false),
            _ => {
                preamble.file_keywords.entry(key).or_insert(val);
            }
        }
    }

    if !setup_properties.is_empty() {
        setup_properties.append(&mut preamble.property_lines);
        preamble.property_lines = setup_properties;
    }
}

// endregion

// region Entry & Checklist Parsing

/// Immutable document-level context shared across heading entry parses.
struct EntryScanContext<'a> {
    /// Underlying source file (for byte offsets).
    source: &'a SourceFile,
    /// Document lines split on `'\n'`.
    lines: &'a [&'a str],
    /// Protected block line intervals.
    protected: &'a [(usize, usize)],
    /// File-wide `#+PROPERTY:` defaults used to seed `:KEY+:` drawer continuations.
    default_properties: &'a HashMap<String, String>,
}

/// Links a newly parsed heading at `level` into `entries` and `level_stack`, returning its parent.
fn link_entry_into_outline(
    level: usize,
    entry_idx: usize,
    entries: &mut [OrgEntry],
    level_stack: &mut Vec<(usize, usize)>,
) -> Option<usize> {
    let parent = find_parent(level_stack, level);
    if let Some(parent_idx) = parent {
        entries[parent_idx].children.push(entry_idx);
    }
    while level_stack.last().is_some_and(|&(lvl, _)| lvl >= level) {
        level_stack.pop();
    }
    level_stack.push((level, entry_idx));
    parent
}

/// Parses a single heading, its optional planning/property lines, and its body section.
fn parse_heading_entry(
    ctx: &EntryScanContext<'_>,
    heading_idx: usize,
    heading: HeadingParts<'_>,
    entries: &mut [OrgEntry],
    level_stack: &mut Vec<(usize, usize)>,
) -> (OrgEntry, usize) {
    let entry_idx = entries.len();
    let heading_offset = if heading_idx < ctx.source.line_count() {
        ctx.source.line_start(heading_idx)
    } else {
        ctx.source.content.len()
    };
    let parent = link_entry_into_outline(heading.level, entry_idx, entries, level_stack);

    let mut entry = OrgEntry {
        level: heading.level,
        keyword: heading.keyword.map(str::to_string),
        priority: heading.priority,
        title: heading.title.to_string(),
        tags: heading.tags.iter().map(|s| s.to_string()).collect(),
        properties: HashMap::new(),
        planning: Planning::default(),
        clocks: Vec::new(),
        checklist: Vec::new(),
        heading_line: heading_idx + 1,
        heading_offset,
        content_end_line: ctx.lines.len(),
        parent,
        children: Vec::new(),
        raw_heading: ctx.lines[heading_idx].to_string(),
    };

    let mut i = heading_idx + 1;
    if i < ctx.lines.len() && is_planning_line(ctx.lines[i].trim()) {
        entry.planning = parse_planning(ctx.lines[i].trim());
        i += 1;
    }
    if i < ctx.lines.len() && ctx.lines[i].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        if let Some((props, end)) = parse_property_drawer(ctx.lines, i + 1, ctx.default_properties)
        {
            entry.properties = props;
            i = end;
        }
    }

    let next_i = scan_entry_body(ctx.lines, ctx.protected, i, &mut entry);
    (entry, next_i)
}

/// Assigns exclusive 1-based `content_end_line` boundaries across `entries`.
fn assign_content_end_lines(entries: &mut [OrgEntry], line_count: usize) {
    for idx in 0..entries.len() {
        let end = if idx + 1 < entries.len() {
            entries[idx + 1].heading_line
        } else {
            line_count + 1
        };
        entries[idx].content_end_line = end;
    }
}

/// Returns `true` if `trimmed` starts with `SCHEDULED:`, `DEADLINE:`, or `CLOSED:`.
fn is_planning_line(trimmed: &str) -> bool {
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Parses `SCHEDULED:`, `DEADLINE:`, and `CLOSED:` timestamps from a planning line.
fn parse_planning(line: &str) -> Planning {
    let mut planning = Planning::default();

    if let Some(pos) = line.find("SCHEDULED:") {
        let rest = line[pos + "SCHEDULED:".len()..].trim_start();
        if let Some((ts, _)) = parse_timestamp(rest, 0) {
            planning.scheduled = Some(ts);
        }
    }
    if let Some(pos) = line.find("DEADLINE:") {
        let rest = line[pos + "DEADLINE:".len()..].trim_start();
        if let Some((ts, _)) = parse_timestamp(rest, 0) {
            planning.deadline = Some(ts);
        }
    }
    if let Some(pos) = line.find("CLOSED:") {
        let rest = line[pos + "CLOSED:".len()..].trim_start();
        if let Some((ts, _)) = parse_timestamp(rest, 0) {
            planning.closed = Some(ts);
        }
    }

    planning
}

/// Finds the parent index for a heading at the given outline `level`.
fn find_parent(level_stack: &[(usize, usize)], level: usize) -> Option<usize> {
    level_stack
        .iter()
        .rev()
        .find(|&&(lvl, _)| lvl < level)
        .map(|&(_, idx)| idx)
}

/// Scans an entry's own body lines for `:LOGBOOK:` clocks, standalone `CLOCK:` lines, and checklists.
///
/// Stops before the next unprotected heading line and strictly skips lines inside
/// `protected` regions (`#+begin_src`, `#+begin_example`, etc.).
fn scan_entry_body(
    lines: &[&str],
    protected: &[(usize, usize)],
    start: usize,
    entry: &mut OrgEntry,
) -> usize {
    let mut i = start;

    while i < lines.len() {
        if is_protected(i, protected) {
            i += 1;
            continue;
        }

        let line = lines[i];
        if parse_heading(line).is_some() {
            return i;
        }

        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":LOGBOOK:") {
            i = scan_logbook_drawer(lines, i + 1, &mut entry.clocks);
            continue;
        }

        if trimmed.starts_with("CLOCK:") {
            if let Some(clock) = parse_clock_line(trimmed, i + 1) {
                entry.clocks.push(clock);
            }
        } else if let Some(item) = parse_checklist_item(line, i + 1) {
            entry.checklist.push(item);
        }

        i += 1;
    }

    i
}

/// Consumes a `:LOGBOOK: ... :END:` drawer starting at `start` and appends parsed clocks.
fn scan_logbook_drawer(lines: &[&str], start: usize, clocks: &mut Vec<ClockEntry>) -> usize {
    let mut i = start;
    while i < lines.len() {
        let inner = lines[i].trim();
        if inner.eq_ignore_ascii_case(":END:") {
            return i + 1;
        }
        if let Some(clock) = parse_clock_line(inner, i + 1) {
            clocks.push(clock);
        }
        i += 1;
    }
    i
}

/// Parses a `CLOCK:` line into a [`ClockEntry`].
fn parse_clock_line(trimmed: &str, line_number: usize) -> Option<ClockEntry> {
    let rest = trimmed.strip_prefix("CLOCK:")?.trim_start();
    let (start_ts, after_start) = parse_timestamp(rest, 0)?;

    let remaining = &rest[after_start..];
    if let Some(dash_pos) = remaining.find("--") {
        let after_dash = &remaining[dash_pos + 2..];
        if let Some((end_ts, after_end)) = parse_timestamp(after_dash, 0) {
            let duration = parse_duration(&after_dash[after_end..]);
            return Some(ClockEntry {
                start: start_ts,
                end: Some(end_ts),
                duration_minutes: duration,
                line: line_number,
            });
        }
    }

    Some(ClockEntry {
        start: start_ts,
        end: None,
        duration_minutes: None,
        line: line_number,
    })
}

/// Parses a duration string like ` =>  1:30` into total minutes.
fn parse_duration(s: &str) -> Option<i64> {
    let rest = s.trim().strip_prefix("=>")?.trim();
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hours: i64 = parts[0].trim().parse().ok()?;
    let minutes: i64 = parts[1].trim().parse().ok()?;
    Some(hours * 60 + minutes)
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an in-memory `SourceFile` named `test.org` with `content`.
    fn make_source(content: &str) -> SourceFile {
        SourceFile::new("test.org", content.to_string())
    }

    /// Verifies parsing an empty document yields zero entries.
    #[test]
    fn empty_document() {
        let source = make_source("");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.entries.is_empty());
    }

    /// Verifies parsing a single level-1 heading.
    #[test]
    fn single_heading() {
        let source = make_source("* Hello\nSome text.\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries.len(), 1);
        assert_eq!(doc.entries[0].level, 1);
        assert_eq!(doc.entries[0].title, "Hello");
        assert_eq!(doc.entries[0].heading_line, 1);
    }

    /// Verifies extracting TODO keyword, priority cookie, title, and tags from a heading.
    #[test]
    fn heading_with_todo_and_tags() {
        let source = make_source("* TODO [#A] Meeting notes :work:urgent:\n");
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.keyword.as_deref(), Some("TODO"));
        assert_eq!(e.priority, Some('A'));
        assert_eq!(e.title, "Meeting notes");
        assert_eq!(e.tags, vec!["work", "urgent"]);
    }

    /// Verifies parent and children indices across nested headings.
    #[test]
    fn parent_child_relationships() {
        let source = make_source("* Parent\n** Child 1\n** Child 2\n*** Grandchild\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries.len(), 4);

        assert!(doc.entries[0].parent.is_none());
        assert_eq!(doc.entries[0].children, vec![1, 2]);

        assert_eq!(doc.entries[1].parent, Some(0));
        assert!(doc.entries[1].children.is_empty());

        assert_eq!(doc.entries[2].parent, Some(0));
        assert_eq!(doc.entries[2].children, vec![3]);

        assert_eq!(doc.entries[3].parent, Some(2));
    }

    /// Verifies computing root-to-entry outline paths.
    #[test]
    fn outline_path() {
        let source = make_source("* A\n** B\n*** C\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.outline_path(2), vec!["A", "B", "C"]);
        assert_eq!(doc.outline_path(0), vec!["A"]);
    }

    /// Verifies parsing `SCHEDULED:` and `DEADLINE:` planning timestamps.
    #[test]
    fn planning_timestamps() {
        let source = make_source(
            "* TODO Task\nSCHEDULED: <2024-01-15 Mon 09:00> DEADLINE: <2024-02-01 Thu>\n",
        );
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert!(e.planning.scheduled.is_some());
        assert!(e.planning.deadline.is_some());
        assert_eq!(e.planning.scheduled.as_ref().unwrap().day, 15);
        assert_eq!(e.planning.deadline.as_ref().unwrap().day, 1);
    }

    /// Verifies parsing `:ID:` and `:CUSTOM_ID:` properties on a heading drawer.
    #[test]
    fn property_drawer() {
        let source =
            make_source("* Heading\n:PROPERTIES:\n:ID: abc-123\n:CUSTOM_ID: my-heading\n:END:\n");
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.properties.get("ID").unwrap(), "abc-123");
        assert_eq!(e.properties.get("CUSTOM_ID").unwrap(), "my-heading");
    }

    /// Verifies looking up heading entries by `:ID:`.
    #[test]
    fn find_by_id() {
        let source = make_source(
            "* A\n:PROPERTIES:\n:ID: uuid-1\n:END:\n* B\n:PROPERTIES:\n:ID: uuid-2\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.find_by_id("uuid-2"), Some(1));
        assert_eq!(doc.find_by_id("uuid-1"), Some(0));
        assert_eq!(doc.find_by_id("nonexistent"), None);
    }

    /// Verifies looking up heading entries by `:CUSTOM_ID:`.
    #[test]
    fn find_by_custom_id() {
        let source = make_source("* Heading\n:PROPERTIES:\n:CUSTOM_ID: my-section\n:END:\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.find_by_custom_id("my-section"), Some(0));
    }

    /// Verifies parsing completed `CLOCK:` lines inside a `:LOGBOOK:` drawer.
    #[test]
    fn clock_entries_in_logbook() {
        let source = make_source(
            "* Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  1:30\nCLOCK: [2024-01-14 Sun 14:00]--[2024-01-14 Sun 14:45] =>  0:45\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.clocks.len(), 2);
        assert_eq!(e.clocks[0].duration_minutes, Some(90));
        assert!(e.clocks[0].end.is_some());
        assert_eq!(e.clocks[1].duration_minutes, Some(45));
    }

    /// Verifies parsing a running `CLOCK:` line without an end timestamp.
    #[test]
    fn running_clock() {
        let source = make_source("* Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]\n:END:\n");
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.clocks.len(), 1);
        assert!(e.clocks[0].end.is_none());
        assert_eq!(e.clocks[0].duration_minutes, None);
    }

    /// Verifies parsing a standalone `CLOCK:` line outside a `:LOGBOOK:` drawer.
    #[test]
    fn standalone_clock_outside_logbook() {
        let source =
            make_source("* Task\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:00] =>  1:00\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].clocks.len(), 1);
    }

    /// Verifies extracting a line-1 file-level `:PROPERTIES:` drawer before `#+TITLE:`.
    #[test]
    fn file_level_properties() {
        let source =
            make_source(":PROPERTIES:\n:ID: file-id\n:END:\n#+TITLE: My Document\n* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties.get("ID").unwrap(), "file-id");
        assert_eq!(doc.file_keywords.get("TITLE").unwrap(), "My Document");
        assert_eq!(doc.entries.len(), 1);
    }

    /// Verifies collecting preamble `#+KEYWORD:` pairs into `file_keywords`.
    #[test]
    fn file_keywords() {
        let source = make_source("#+TITLE: Test\n#+AUTHOR: User\n* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_keywords.get("TITLE").unwrap(), "Test");
        assert_eq!(doc.file_keywords.get("AUTHOR").unwrap(), "User");
    }

    /// Verifies exclusive 1-based `content_end_line` computation across entries.
    #[test]
    fn content_end_lines() {
        let source = make_source("* A\ntext\n* B\nmore text\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].heading_line, 1);
        assert_eq!(doc.entries[0].content_end_line, 3);
        assert_eq!(doc.entries[1].heading_line, 3);
    }

    /// Verifies tag inheritance from parent headings to children.
    #[test]
    fn inherited_tags() {
        let source = make_source("* Parent :parent_tag:\n** Child :child_tag:\n");
        let doc = OrgDocument::from_source(&source);
        let tags = doc.inherited_tags(1);
        assert!(tags.contains(&"child_tag"));
        assert!(tags.contains(&"parent_tag"));
    }

    /// Verifies parsing a planning line followed immediately by a `:PROPERTIES:` drawer.
    #[test]
    fn planning_after_heading_then_properties() {
        let source = make_source(
            "* TODO Task\nSCHEDULED: <2024-01-15 Mon>\n:PROPERTIES:\n:ID: t1\n:END:\nBody text\n",
        );
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert!(e.planning.scheduled.is_some());
        assert_eq!(e.properties.get("ID").unwrap(), "t1");
    }

    /// Verifies sibling and parent links across alternating level-1 and level-2 headings.
    #[test]
    fn sibling_headings_at_different_levels() {
        let source = make_source("* A\n** B\n* C\n** D\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].children, vec![1]);
        assert_eq!(doc.entries[2].children, vec![3]);
        assert_eq!(doc.entries[1].parent, Some(0));
        assert_eq!(doc.entries[3].parent, Some(2));
    }

    /// Verifies `parse_duration` on valid and invalid `=> HH:MM` inputs.
    #[test]
    fn parse_duration_values() {
        assert_eq!(parse_duration("=>  1:30"), Some(90));
        assert_eq!(parse_duration("=> 0:45"), Some(45));
        assert_eq!(parse_duration("=> 10:00"), Some(600));
        assert_eq!(parse_duration("not a duration"), None);
    }

    /// Verifies `parse_property_line` on valid and malformed drawer lines.
    #[test]
    fn property_line_parsing() {
        assert_eq!(
            parse_property_line(":ID: abc"),
            Some(("ID".to_string(), "abc".to_string()))
        );
        assert_eq!(
            parse_property_line(":CUSTOM_ID: my-id"),
            Some(("CUSTOM_ID".to_string(), "my-id".to_string()))
        );
        assert_eq!(parse_property_line("not a property"), None);
        assert_eq!(parse_property_line("::"), None);
    }

    /// Verifies case-normalizing `parse_keyword_line` on `#+KEY: value` lines.
    #[test]
    fn keyword_line_parsing() {
        assert_eq!(
            parse_keyword_line("#+TITLE: My Doc"),
            Some(("TITLE".to_string(), "My Doc".to_string()))
        );
        assert_eq!(
            parse_keyword_line("#+author: Me"),
            Some(("AUTHOR".to_string(), "Me".to_string()))
        );
        assert_eq!(parse_keyword_line("not a keyword"), None);
    }

    /// Verifies custom `#+TODO:` active and done keyword sequences.
    #[test]
    fn custom_todo_keywords_recognized() {
        let source = make_source("#+TODO: OPEN | CLOSED WONTFIX\n* OPEN A bug\n* CLOSED Fixed\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.todo_keywords.todo, vec!["OPEN"]);
        assert_eq!(doc.todo_keywords.done, vec!["CLOSED", "WONTFIX"]);
        assert_eq!(doc.entries[0].keyword, Some("OPEN".to_string()));
        assert_eq!(doc.entries[1].keyword, Some("CLOSED".to_string()));
    }

    /// Verifies default `TODO` is not treated as a keyword when overridden by `#+TODO:`.
    #[test]
    fn custom_todo_default_not_recognized() {
        let source = make_source("#+TODO: OPEN | CLOSED\n* TODO This is a title\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].keyword, None);
        assert_eq!(doc.entries[0].title, "TODO This is a title");
    }

    /// Verifies default `TODO` and `DONE` keywords when no `#+TODO:` line is present.
    #[test]
    fn no_todo_setting_uses_defaults() {
        let source = make_source("* TODO Task\n* DONE Finished\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].keyword, Some("TODO".to_string()));
        assert_eq!(doc.entries[1].keyword, Some("DONE".to_string()));
    }

    /// Verifies `#+SEQ_TODO:` keyword sequence parsing.
    #[test]
    fn seq_todo_keyword() {
        let source = make_source("#+SEQ_TODO: DRAFT REVIEW | PUBLISHED\n* REVIEW Article\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].keyword, Some("REVIEW".to_string()));
        assert!(doc.todo_keywords.is_done("PUBLISHED"));
        assert!(!doc.todo_keywords.is_done("REVIEW"));
    }

    /// Verifies custom `#+PRIORITIES:` bounds.
    #[test]
    fn custom_priority_range() {
        let source = make_source("#+PRIORITIES: A E B\n* [#D] Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.priority_range.highest, 'A');
        assert_eq!(doc.priority_range.lowest, 'E');
        assert_eq!(doc.priority_range.default, 'B');
        assert!(doc.priority_range.is_valid('D'));
    }

    /// Verifies default `A..C` priority bounds when `#+PRIORITIES:` is absent.
    #[test]
    fn default_priority_range() {
        let source = make_source("* [#B] Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.priority_range, PriorityRange::default());
    }

    /// Verifies `#+FILETAGS:` inheritance on entries.
    #[test]
    fn filetags_inherited() {
        let source = make_source("#+FILETAGS: :project:work:\n* Heading :local:\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.filetags, vec!["project", "work"]);
        let tags = doc.inherited_tags(0);
        assert!(tags.contains(&"local"));
        assert!(tags.contains(&"project"));
        assert!(tags.contains(&"work"));
    }

    /// Verifies empty `filetags` when `#+FILETAGS:` is omitted.
    #[test]
    fn filetags_empty() {
        let source = make_source("* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.filetags.is_empty());
    }

    /// Verifies `#+FILETAGS:` parsing without surrounding colons.
    #[test]
    fn filetags_without_surrounding_colons() {
        let source = make_source("#+FILETAGS: tag1:tag2\n* H\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.filetags, vec!["tag1", "tag2"]);
    }

    /// Verifies `OrgDocument::property` falling back to `#+PROPERTY:` defaults.
    #[test]
    fn default_property_fallback() {
        let source = make_source("#+PROPERTY: CATEGORY work\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.property(0, "CATEGORY"), Some("work"));
    }

    /// Verifies entry drawer properties overriding `#+PROPERTY:` defaults.
    #[test]
    fn entry_property_overrides_default() {
        let source = make_source(
            "#+PROPERTY: CATEGORY work\n* Task\n:PROPERTIES:\n:CATEGORY: personal\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.property(0, "CATEGORY"), Some("personal"));
    }

    /// Verifies `OrgDocument::property` returning `None` when unset everywhere.
    #[test]
    fn no_default_property() {
        let source = make_source("* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.property(0, "CATEGORY"), None);
    }

    /// Verifies building `TagSpec` from a single `#+TAGS:` line.
    #[test]
    fn tag_spec_from_tags_keyword() {
        let source = make_source("#+TAGS: @work @home laptop\n* Task :@work:\n");
        let doc = OrgDocument::from_source(&source);
        assert!(!doc.tag_spec.allow_any);
        assert!(doc.tag_spec.matches_tag("@work"));
        assert!(doc.tag_spec.matches_tag("laptop"));
        assert!(!doc.tag_spec.matches_tag("unknown"));
    }

    /// Verifies additive merging across multiple `#+TAGS:` lines.
    #[test]
    fn tag_spec_multiple_tags_lines() {
        let source = make_source("#+TAGS: @work @home\n#+TAGS: laptop pc\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.tag_spec.matches_tag("@work"));
        assert!(doc.tag_spec.matches_tag("laptop"));
        assert!(doc.tag_spec.matches_tag("pc"));
    }

    /// Verifies `allow_any` tag matching when `#+TAGS:` is absent.
    #[test]
    fn tag_spec_allow_any_when_no_tags() {
        let source = make_source("* Task :anything:\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.tag_spec.allow_any);
    }

    /// Verifies exclusive tag groups in `#+TAGS:`.
    #[test]
    fn tag_spec_with_groups() {
        let source = make_source("#+TAGS: { @work @home } laptop\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.tag_spec.matches_tag("@work"));
        assert!(doc.tag_spec.matches_tag("laptop"));
        assert_eq!(doc.tag_spec.groups.len(), 1);
        assert!(doc.tag_spec.groups[0].exclusive);
    }

    /// Verifies parsing a custom `#+LINK:` abbreviation.
    #[test]
    fn link_abbreviation_parsed() {
        let source = make_source("#+LINK: gh https://github.com/\n* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(
            doc.link_abbreviations.get("gh"),
            Some(&"https://github.com/".to_string())
        );
    }

    /// Verifies additive merging across multiple `#+LINK:` lines.
    #[test]
    fn multiple_link_abbreviations() {
        let source = make_source(
            "#+LINK: gh https://github.com/\n#+LINK: wp https://en.wikipedia.org/wiki/\n* H\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.link_abbreviations.len(), 2);
        assert!(doc.link_abbreviations.contains_key("gh"));
        assert!(doc.link_abbreviations.contains_key("wp"));
    }

    /// Verifies empty `link_abbreviations` when no `#+LINK:` lines exist.
    #[test]
    fn no_link_abbreviations() {
        let source = make_source("* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.link_abbreviations.is_empty());
    }

    /// Verifies built-in link schemes in `is_known_link_scheme`.
    #[test]
    fn is_known_link_scheme_builtin() {
        let source = make_source("* H\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.is_known_link_scheme("file"));
        assert!(doc.is_known_link_scheme("https"));
        assert!(doc.is_known_link_scheme("mailto"));
        assert!(!doc.is_known_link_scheme("custom"));
    }

    /// Verifies custom `#+LINK:` prefixes in `is_known_link_scheme`.
    #[test]
    fn is_known_link_scheme_custom() {
        let source = make_source("#+LINK: gh https://github.com/\n* H\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.is_known_link_scheme("gh"));
        assert!(doc.is_known_link_scheme("https"));
        assert!(!doc.is_known_link_scheme("unknown"));
    }

    /// Verifies parsing `name=value` pairs on a `#+CONSTANTS:` line.
    #[test]
    fn constants_parsed() {
        let source = make_source("#+CONSTANTS: pi=3.14159 c=299792458\n* H\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.table_constants.get("pi"), Some(&"3.14159".to_string()));
        assert_eq!(doc.table_constants.get("c"), Some(&"299792458".to_string()));
    }

    /// Verifies additive merging across multiple `#+CONSTANTS:` lines.
    #[test]
    fn constants_multiple_lines() {
        let source = make_source("#+CONSTANTS: pi=3.14\n#+CONSTANTS: e=2.718\n* H\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.table_constants.len(), 2);
        assert!(doc.table_constants.contains_key("pi"));
        assert!(doc.table_constants.contains_key("e"));
    }

    /// Verifies empty `table_constants` when `#+CONSTANTS:` is omitted.
    #[test]
    fn constants_empty() {
        let source = make_source("* H\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.table_constants.is_empty());
    }

    /// Verifies scientific-notation constant values in `#+CONSTANTS:`.
    #[test]
    fn constants_scientific_notation() {
        let source = make_source("#+CONSTANTS: eps=2.4e-6\n* H\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.table_constants.get("eps"), Some(&"2.4e-6".to_string()));
    }

    /// Verifies inheriting `#+TODO:` and `#+FILETAGS:` from a local `#+SETUPFILE:`.
    #[test]
    fn setupfile_inherits_keywords() {
        let dir = tempfile::tempdir().unwrap();
        let setup_path = dir.path().join("setup.org");
        std::fs::write(&setup_path, "#+TODO: OPEN | CLOSED\n#+FILETAGS: :shared:\n").unwrap();

        let main_path = dir.path().join("main.org");
        let main_content = format!("#+SETUPFILE: {}\n* OPEN Task\n", setup_path.display());
        let source = SourceFile::new(main_path, main_content);
        let doc = OrgDocument::from_source(&source);

        assert_eq!(doc.todo_keywords.todo, vec!["OPEN"]);
        assert_eq!(doc.todo_keywords.done, vec!["CLOSED"]);
        assert_eq!(doc.filetags, vec!["shared"]);
    }

    /// Verifies main-file keywords overriding non-additive `#+SETUPFILE:` defaults.
    #[test]
    fn setupfile_main_file_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let setup_path = dir.path().join("setup.org");
        std::fs::write(&setup_path, "#+TODO: OPEN | CLOSED\n").unwrap();

        let main_path = dir.path().join("main.org");
        let main_content = format!(
            "#+SETUPFILE: {}\n#+TODO: DRAFT | PUBLISHED\n* DRAFT Task\n",
            setup_path.display()
        );
        let source = SourceFile::new(main_path, main_content);
        let doc = OrgDocument::from_source(&source);

        assert_eq!(doc.todo_keywords.todo, vec!["DRAFT"]);
        assert_eq!(doc.todo_keywords.done, vec!["PUBLISHED"]);
    }

    /// Verifies additive `#+TAGS:` merging across `#+SETUPFILE:` and the main file.
    #[test]
    fn setupfile_additive_tags() {
        let dir = tempfile::tempdir().unwrap();
        let setup_path = dir.path().join("setup.org");
        std::fs::write(&setup_path, "#+TAGS: @shared\n").unwrap();

        let main_path = dir.path().join("main.org");
        let main_content = format!(
            "#+SETUPFILE: {}\n#+TAGS: @local\n* Task\n",
            setup_path.display()
        );
        let source = SourceFile::new(main_path, main_content);
        let doc = OrgDocument::from_source(&source);

        assert!(doc.tag_spec.matches_tag("@shared"));
        assert!(doc.tag_spec.matches_tag("@local"));
    }

    /// Verifies skipping remote URL `#+SETUPFILE:` targets gracefully.
    #[test]
    fn setupfile_url_skipped() {
        let source = make_source("#+SETUPFILE: https://example.com/setup.org\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.todo_keywords, TodoKeywords::default());
    }

    /// Verifies falling back to defaults when a `#+SETUPFILE:` path is missing.
    #[test]
    fn setupfile_missing_uses_defaults() {
        let source = make_source("#+SETUPFILE: ./nonexistent.org\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.todo_keywords, TodoKeywords::default());
    }

    /// Verifies extraction of file-level `:ID:` and `#+title:` in a headingless org-roam v2 note.
    #[test]
    fn org_roam_v2_file_id_and_title_without_headings() {
        let source = make_source(
            ":PROPERTIES:\n:ID: 550e8400-e29b-41d4-a716-446655440000\n:END:\n#+title: Zettelkasten Note\n#+filetags: :roam:note:\nBody paragraph.\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties_line, Some(1));
        assert_eq!(doc.file_id(), Some("550e8400-e29b-41d4-a716-446655440000"));
        assert_eq!(doc.file_title(), Some("Zettelkasten Note"));
        assert_eq!(doc.file_title_or_stem(), "Zettelkasten Note");
        assert!(doc.has_file_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(doc.contains_id("550e8400-e29b-41d4-a716-446655440000"));
        assert_eq!(doc.find_by_id("550e8400-e29b-41d4-a716-446655440000"), None);
        assert_eq!(doc.filetags, vec!["roam", "note"]);
    }

    /// Verifies extraction of top-of-file `:PROPERTIES:` preceded only by `# comment` lines.
    #[test]
    fn org_roam_v2_file_properties_after_leading_comments() {
        let source = make_source(
            "# -*- mode: org; -*-\n# Roam header comment\n:PROPERTIES:\n:ID: roam-after-comments\n:END:\n#+TITLE: Commented Note\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties_line, Some(3));
        assert_eq!(doc.file_id(), Some("roam-after-comments"));
        assert_eq!(doc.file_title(), Some("Commented Note"));
    }

    /// Verifies that a `:PROPERTIES:` drawer appearing after `#+TITLE:` is ignored as misplaced.
    #[test]
    fn misplaced_drawer_after_title_not_extracted_as_file_properties() {
        let source = make_source(
            "#+TITLE: Title First\n:PROPERTIES:\n:ID: misplaced-id\n:END:\n* Heading\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties_line, None);
        assert_eq!(doc.file_id(), None);
        assert!(doc.file_properties.is_empty());
    }

    /// Verifies that `file_title_or_stem` falls back to the file stem when `#+TITLE:` is absent.
    #[test]
    fn file_title_or_stem_falls_back_to_filename_stem() {
        let source = SourceFile::new(
            "/notes/20261106-graph-model.org",
            ":PROPERTIES:\n:ID: f1\n:END:\nBody text.\n".to_string(),
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_title(), None);
        assert_eq!(doc.file_title_or_stem(), "20261106-graph-model");
    }

    /// Verifies that `#hashtag` (no space/tab after `#`) is not treated as an org comment line.
    #[test]
    fn hashtag_line_before_drawer_not_treated_as_comment() {
        let source = make_source("#hashtag\n:PROPERTIES:\n:ID: bad-id\n:END:\n* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties_line, None);
        assert_eq!(doc.file_id(), None);
        assert!(doc.file_properties.is_empty());
    }

    /// Verifies that an unclosed top-of-file `:PROPERTIES:` drawer does not swallow `#+TITLE:` or headings.
    #[test]
    fn unclosed_top_of_file_drawer_does_not_swallow_title_or_headings() {
        let source = make_source(
            ":PROPERTIES:\n:ID: unclosed-id\n#+TITLE: Preserved Title\n* Preserved Heading\nBody.\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties_line, None);
        assert_eq!(doc.file_id(), None);
        assert!(doc.file_properties.is_empty());
        assert_eq!(doc.file_title(), Some("Preserved Title"));
        assert_eq!(doc.entries.len(), 1);
        assert_eq!(doc.entries[0].title, "Preserved Heading");
    }

    /// Verifies that `:KEY+:` in a heading drawer appends onto a `#+PROPERTY:` file default.
    #[test]
    fn entry_property_plus_appends_onto_default_property() {
        let source = make_source(
            "#+PROPERTY: var foo=1\n* Customized\n:PROPERTIES:\n:var+: bar=2\n:END:\n* Inherited\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.property(0, "var"), Some("foo=1 bar=2"));
        assert_eq!(
            doc.entries[0].properties.get("var").map(String::as_str),
            Some("foo=1 bar=2")
        );
        assert_eq!(doc.property(1, "var"), Some("foo=1"));
    }

    /// Verifies standalone `:KEY+:` and consecutive `:KEY+:` continuations inside a heading drawer.
    #[test]
    fn entry_property_plus_standalone_in_drawer() {
        let source = make_source(
            "* Heading\n:PROPERTIES:\n:ROAM_ALIASES+: \"First\"\n:ROAM_ALIASES+: \"Second\"\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(
            doc.property(0, "ROAM_ALIASES"),
            Some("\"First\" \"Second\"")
        );
    }

    /// Verifies that multiple `#+PROPERTY:` lines preserve all distinct keys in `default_properties`.
    #[test]
    fn multiple_property_keywords_preserve_distinct_keys() {
        let source = make_source(
            "#+PROPERTY: CATEGORY work\n#+PROPERTY: EFFORT_ALL 0:15 0:30 1:00\n#+PROPERTY: header-args :results silent\n* Task\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(
            doc.default_properties.get("CATEGORY").map(String::as_str),
            Some("work")
        );
        assert_eq!(
            doc.default_properties.get("EFFORT_ALL").map(String::as_str),
            Some("0:15 0:30 1:00")
        );
        assert_eq!(
            doc.default_properties
                .get("header-args")
                .map(String::as_str),
            Some(":results silent")
        );
        assert_eq!(doc.property(0, "CATEGORY"), Some("work"));
        assert_eq!(doc.property(0, "EFFORT_ALL"), Some("0:15 0:30 1:00"));
        assert_eq!(doc.property(0, "header-args"), Some(":results silent"));
    }

    /// Verifies that `#+PROPERTY: KEY+ MORE` appends `" MORE"` to an existing key.
    #[test]
    fn property_plus_appends_to_existing_key() {
        let source = make_source(
            "#+PROPERTY: var foo=1\n#+PROPERTY: var+ bar=2\n#+PROPERTY: var+ baz=3\n* Task\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(
            doc.default_properties.get("var").map(String::as_str),
            Some("foo=1 bar=2 baz=3")
        );
    }

    /// Verifies that `#+PROPERTY: KEY+ MORE` inserts `KEY -> MORE` when `KEY` was not previously set.
    #[test]
    fn property_plus_inserts_when_key_not_previously_set() {
        let source = make_source("#+PROPERTY: header-args+ :exports code\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(
            doc.default_properties
                .get("header-args")
                .map(String::as_str),
            Some(":exports code")
        );
    }

    /// Verifies that `#+PROPERTY: KEY VAL` overwrites an earlier value before subsequent `KEY+` appends.
    #[test]
    fn property_without_plus_overwrites_previous_value() {
        let source = make_source(
            "#+PROPERTY: CATEGORY first\n#+PROPERTY: CATEGORY second\n#+PROPERTY: CATEGORY+ extra\n* Task\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(
            doc.default_properties.get("CATEGORY").map(String::as_str),
            Some("second extra")
        );
    }

    /// Builds a `ChecklistItem` test value concisely.
    fn checklist_item(line: usize, state: CheckboxState, text: &str) -> ChecklistItem {
        ChecklistItem {
            line,
            state,
            text: text.to_string(),
        }
    }

    /// Verifies parsing of unordered `-`, `+`, and indented `*` plain-list checkboxes across states.
    #[test]
    fn checklist_parses_unordered_bullets_and_states() {
        let source = make_source(
            "* TODO Release checklist\n- [ ] Write unit tests\n- [-] Implement parser\n- [X] Read specification\n+ [x] Check lowercase X\n  * [ ] Indented star bullet\n- Plain bullet without checkbox\n",
        );
        let doc = OrgDocument::from_source(&source);
        let entry = &doc.entries[0];
        assert_eq!(
            entry.checklist,
            vec![
                checklist_item(2, CheckboxState::Unchecked, "Write unit tests"),
                checklist_item(3, CheckboxState::Partial, "Implement parser"),
                checklist_item(4, CheckboxState::Checked, "Read specification"),
                checklist_item(5, CheckboxState::Checked, "Check lowercase X"),
                checklist_item(6, CheckboxState::Unchecked, "Indented star bullet"),
            ]
        );
    }

    /// Verifies parsing of ordered plain-list checkboxes (`1.`, `2)`, `a.`) and `checklist_summary()`.
    #[test]
    fn checklist_parses_ordered_bullets_and_summary() {
        let source = make_source(
            "* Ordered steps\n1. [X] First ordered step\n2) [-] Second ordered step\na. [ ] Alphabetical step\n3. [x] Final checked step\n",
        );
        let doc = OrgDocument::from_source(&source);
        let entry = &doc.entries[0];
        assert_eq!(
            entry.checklist,
            vec![
                checklist_item(2, CheckboxState::Checked, "First ordered step"),
                checklist_item(3, CheckboxState::Partial, "Second ordered step"),
                checklist_item(4, CheckboxState::Unchecked, "Alphabetical step"),
                checklist_item(5, CheckboxState::Checked, "Final checked step"),
            ]
        );
        assert_eq!(
            entry.checklist_summary(),
            ChecklistSummary {
                total: 4,
                checked: 2,
                partial: 1,
                unchecked: 1,
            }
        );
    }

    /// Verifies that checklist items belong strictly to each entry's own body before child headings.
    #[test]
    fn checklist_scoped_to_entry_own_body_before_child_headings() {
        let source = make_source(
            "* Parent\n- [ ] Parent item 1\n- [X] Parent item 2\n** Child\n- [ ] Child item 1\n- [-] Child item 2\n- [X] Child item 3\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].checklist.len(), 2);
        assert_eq!(doc.entries[0].checklist_summary().total, 2);
        assert_eq!(doc.entries[1].checklist.len(), 3);
        assert_eq!(doc.entries[1].checklist_summary().total, 3);
    }

    /// Verifies that checkboxes inside protected blocks (`#+begin_src`, `#+BEGIN_EXAMPLE`, etc.) are skipped.
    #[test]
    fn checklist_skips_protected_regions() {
        let source = make_source(
            "* Heading\n- [ ] Before blocks\n#+begin_src org\n- [ ] Inside src block\n* Fake heading in src\n- [X] Also inside src\n#+end_src\n#+BEGIN_EXAMPLE\n- [X] Inside example block\n#+END_EXAMPLE\n#+begin_quote\n- [-] Inside quote block\n#+end_quote\n- [X] After blocks\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries.len(), 1);
        assert_eq!(
            doc.entries[0].checklist,
            vec![
                ChecklistItem {
                    line: 2,
                    state: CheckboxState::Unchecked,
                    text: "Before blocks".to_string(),
                },
                ChecklistItem {
                    line: 14,
                    state: CheckboxState::Checked,
                    text: "After blocks".to_string(),
                },
            ]
        );
    }
}

// endregion
