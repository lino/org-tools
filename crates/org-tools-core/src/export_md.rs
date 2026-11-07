// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=1500 reason="Cohesive pure-Rust Org workspace slice extractor and GFM Markdown exporter covering blocks, tables, links, inline markup, and unit tests"

//! Pure-Rust Mermaid-aware GitHub Flavored Markdown exporter for Org-mode documents.
//!
//! Converts [`OrgDocument`] files and scoped Org slices into Zensical-ready GitHub
//! Flavored Markdown without invoking external tools such as `pandoc`.
//!
//! Heading mapping rules:
//! - `#+TITLE: Foo` renders as `# Foo` and shifts subsequent `*` headings by `+1`
//!   (`*` -> `##`, `**` -> `###`, clamped at `######`).
//! - Documents or slices without `#+TITLE:` map `*`..`******` directly to `#`..`######`.
//! - Top-of-file `:PROPERTIES:` drawers and heading `:PROPERTIES:` / `:LOGBOOK:`
//!   drawers are stripped so internal metadata never leaks into rendered prose.
//!
//! Spec: [Org Syntax](https://orgmode.org/worg/org-syntax.html),
//! [§13 Exporting](https://orgmode.org/manual/Exporting.html)

use std::path::{Component, Path, PathBuf};

use unicode_width::UnicodeWidthStr;

use crate::document::{ChecklistItem, OrgDocument};
use crate::entry::{parse_checklist_item, CheckboxState};
use crate::files::{canonicalize_clean, collect_org_files, strip_windows_verbatim};
use crate::locator::{resolve_in_document, OrgLocator, ResolvedEntry};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::heading::{parse_heading_with_keywords, HeadingParts};
use crate::source::SourceFile;

// region Constants & Types

/// Maximum heading depth supported by GitHub Flavored Markdown (`######`).
const MAX_GFM_HEADING_LEVEL: usize = 6;

/// Minimum column width for standard GFM table separator cells (`---`).
const MIN_GFM_TEXT_COL_WIDTH: usize = 3;

/// Minimum column width for right-aligned numeric GFM separator cells (`---:`).
const MIN_GFM_NUMERIC_COL_WIDTH: usize = 4;

/// Minimum backtick fence length for fenced code blocks.
const MIN_CODE_FENCE_LEN: usize = 3;

/// Environment variable allowing workspace root override without CLI flags.
pub const WORKSPACE_ENV_VAR: &str = "ORG_TOOLS_WORKSPACE";

/// Project marker files inspected when discovering an ancestor workspace root.
const WORKSPACE_MARKERS: &[&str] = &[".org-tools.toml", ".fritz.toml"];

/// Workspace context used while exporting an Org document or slice to Markdown.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownExportContext<'a> {
    /// Parsed workspace documents consulted when resolving cross-node `[[id:...]]` links.
    pub workspace_docs: &'a [OrgDocument],
    /// File path of the document currently being exported.
    pub current_file: &'a Path,
}

/// Exact raw Org slice and 1-based inclusive line bounds for a resolved node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractedOrgSlice<'a> {
    /// Borrowed raw Org slice from the underlying [`SourceFile`].
    pub raw: &'a str,
    /// One-based line number where the resolved node starts.
    pub start_line: usize,
    /// One-based inclusive line number where the extracted slice ends.
    pub end_line: usize,
}

/// Loaded document, resolved node, and file-scan counters from workspace resolution.
pub struct ResolvedWorkspaceNode {
    /// Resolved node metadata.
    pub resolved: ResolvedEntry,
    /// Raw source buffer for the target file.
    pub source: SourceFile,
    /// Parsed document for the target file.
    pub doc: OrgDocument,
    /// Parsed workspace documents when Markdown link resolution is requested.
    pub workspace_docs: Vec<OrgDocument>,
    /// Number of `.org` files scanned.
    pub files_scanned: usize,
    /// Number of in-memory document cache hits.
    pub cache_hits: usize,
    /// Number of disk document parses.
    pub cache_misses: usize,
}

/// Internal representation of a parsed Org pipe-table row.
enum MdTableRow {
    /// Horizontal rule row (`|---+---|`).
    Separator,
    /// Data cells already converted to inline Markdown.
    Data(Vec<String>),
}

// endregion

// region Public API

/// Exports a parsed [`OrgDocument`] and its [`SourceFile`] into Zensical-ready GFM.
pub fn export_document_to_markdown(
    source: &SourceFile,
    doc: &OrgDocument,
    workspace_docs: &[OrgDocument],
) -> String {
    let ctx = MarkdownExportContext {
        workspace_docs,
        current_file: &doc.file,
    };
    let raw = source.content.strip_suffix('\n').unwrap_or(&source.content);
    if source.content.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = raw
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    render_org_lines(&lines, doc, &ctx)
}

/// Exports an arbitrary Org slice associated with `current_file` into GFM.
pub fn export_slice_to_markdown(
    slice: &str,
    current_file: &Path,
    workspace_docs: &[OrgDocument],
) -> String {
    let temp_source = SourceFile::new(current_file.to_path_buf(), slice.to_string());
    let temp_doc = OrgDocument::from_source(&temp_source);
    export_document_to_markdown(&temp_source, &temp_doc, workspace_docs)
}

/// Converts rendered heading text into a lowercase GFM / Zensical anchor slug.
pub fn slugify_heading(text: &str) -> String {
    let plain = strip_markdown_inline_tokens(text);
    let mut slug = String::new();
    let mut prev_hyphen = false;

    for ch in plain.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' {
            slug.push(ch);
            prev_hyphen = false;
        } else if (ch.is_whitespace() || ch == '-') && !slug.is_empty() && !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }

    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Extracts the exact raw Org slice and 1-based line bounds for `resolved`.
pub fn extract_org_slice<'a>(
    source: &'a SourceFile,
    doc: &OrgDocument,
    resolved: &ResolvedEntry,
    no_children: bool,
) -> ExtractedOrgSlice<'a> {
    let total_lines = logical_line_count(&source.content);
    if resolved.is_file_node() {
        let start_line = resolved.line.max(1);
        if no_children {
            if let Some(first_heading) = doc.entries.first() {
                let end_byte = source.line_start(first_heading.heading_line - 1);
                let end_line = first_heading.heading_line.saturating_sub(1).max(start_line);
                return ExtractedOrgSlice {
                    raw: &source.content[..end_byte],
                    start_line,
                    end_line,
                };
            }
        }
        return ExtractedOrgSlice {
            raw: &source.content[..],
            start_line,
            end_line: total_lines.max(start_line),
        };
    }

    let entry_idx = resolved.entry_index;
    let entry = &doc.entries[entry_idx];
    let start_line = entry.heading_line;
    let start_byte = source.line_start(start_line - 1);
    let stop_entry = if no_children {
        doc.entries.get(entry_idx + 1)
    } else {
        doc.entries[entry_idx + 1..]
            .iter()
            .find(|e| e.level <= entry.level)
    };

    match stop_entry {
        Some(stop) => ExtractedOrgSlice {
            raw: &source.content[start_byte..source.line_start(stop.heading_line - 1)],
            start_line,
            end_line: stop.heading_line.saturating_sub(1).max(start_line),
        },
        None => ExtractedOrgSlice {
            raw: &source.content[start_byte..],
            start_line,
            end_line: total_lines.max(start_line),
        },
    }
}

/// Truncates `text` cleanly on a Unicode scalar boundary to at most `max_chars` characters.
pub fn truncate_unicode_chars(text: &str, max_chars: Option<usize>) -> (String, bool, usize) {
    let total = text.chars().count();
    match max_chars {
        Some(limit) if total > limit => (text.chars().take(limit).collect(), true, limit),
        _ => (text.to_string(), false, total),
    }
}

/// Scans file-level preamble lines before the first heading for plain-list checklist items.
pub fn scan_file_node_checklists(source: &SourceFile, doc: &OrgDocument) -> Vec<ChecklistItem> {
    let stop_idx = doc
        .entries
        .first()
        .map(|e| e.heading_line - 1)
        .unwrap_or_else(|| logical_line_count(&source.content));
    let protected = protected_regions(&source.content);
    source
        .lines()
        .take(stop_idx)
        .filter(|(idx, _)| !is_protected(*idx, &protected))
        .filter_map(|(idx, line)| parse_checklist_item(line, idx + 1))
        .collect()
}

/// Cleans `.` and `..` path components lexicographically without touching the filesystem.
fn clean_lexical_path(path: &Path) -> PathBuf {
    let clean_in = strip_windows_verbatim(path.to_path_buf());
    let mut out = PathBuf::new();
    for comp in clean_in.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !clean_in.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Absolutizes `path` against `cwd`, cleans `.`/`..`, and canonicalizes existing ancestors.
pub fn normalize_workspace_path(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        clean_lexical_path(path)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        clean_lexical_path(&cwd.join(path))
    };
    let mut cur: &Path = &abs;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    while !cur.exists() {
        let Some(name) = cur.file_name() else {
            return abs;
        };
        tail.push(name);
        let Some(parent) = cur.parent() else {
            return abs;
        };
        cur = parent;
    }
    let mut resolved = canonicalize_clean(cur).unwrap_or_else(|_| cur.to_path_buf());
    for seg in tail.into_iter().rev() {
        resolved.push(seg);
    }
    resolved
}

/// Discovers the effective workspace root from explicit path, env var, or ancestor markers.
pub fn discover_workspace_root(explicit: Option<&Path>, anchor_dir: &Path) -> PathBuf {
    if let Some(dir) = explicit {
        return normalize_workspace_path(dir);
    }
    if let Some(env_val) = std::env::var_os(WORKSPACE_ENV_VAR).filter(|v| !v.is_empty()) {
        return normalize_workspace_path(Path::new(&env_val));
    }
    let norm_anchor = normalize_workspace_path(anchor_dir);
    let start_dir = if norm_anchor.is_file() {
        norm_anchor.parent().unwrap_or(&norm_anchor)
    } else {
        &norm_anchor
    };
    let mut current = Some(start_dir);
    while let Some(dir) = current {
        if WORKSPACE_MARKERS.iter().any(|m| dir.join(m).is_file()) {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    start_dir.to_path_buf()
}

/// Resolves any [`OrgLocator`] variant across `explicit_ws` (or discovered workspace root).
pub fn resolve_workspace_locator(
    locator: &OrgLocator,
    explicit_ws: Option<&Path>,
    cwd: &Path,
    need_ws_docs: bool,
) -> Result<ResolvedWorkspaceNode, String> {
    match locator {
        OrgLocator::Id(uuid) => {
            let ws_root = discover_workspace_root(explicit_ws, cwd);
            if !ws_root.exists() {
                return Err(format!("workspace path not found: {}", ws_root.display()));
            }
            scan_workspace_for_id(uuid, &collect_org_files(&[ws_root]), need_ws_docs)
        }
        OrgLocator::CustomId { file, .. }
        | OrgLocator::OutlinePath { file, .. }
        | OrgLocator::LineRef { file, .. } => {
            resolve_file_scoped_node(file, locator, explicit_ws, need_ws_docs)
        }
    }
}

/// Scans `files` to locate `id:<uuid>`, optionally collecting all workspace documents.
fn scan_workspace_for_id(
    uuid: &str,
    files: &[PathBuf],
    need_ws_docs: bool,
) -> Result<ResolvedWorkspaceNode, String> {
    let id_loc = OrgLocator::Id(uuid.to_string());
    let mut matched: Option<(ResolvedEntry, SourceFile, OrgDocument)> = None;
    let mut ws_docs = Vec::new();
    let mut scanned = 0usize;

    for file_path in files {
        let source = SourceFile::from_path(file_path)
            .map_err(|e| format!("{}: {e}", file_path.display()))?;
        scanned += 1;
        let doc = OrgDocument::from_source(&source);
        if matched.is_none() {
            if let Ok(res) = resolve_in_document(&id_loc, &doc) {
                if !need_ws_docs {
                    return Ok(ResolvedWorkspaceNode {
                        resolved: res,
                        source,
                        doc,
                        workspace_docs: Vec::new(),
                        files_scanned: scanned,
                        cache_hits: 0,
                        cache_misses: scanned,
                    });
                }
                let cloned = SourceFile::new(file_path, source.content.clone());
                matched = Some((res, cloned, OrgDocument::from_source(&source)));
            }
        }
        if need_ws_docs {
            ws_docs.push(doc);
        }
    }

    let Some((resolved, source, doc)) = matched else {
        return Err(format!("entry not found: id:{uuid}"));
    };
    Ok(ResolvedWorkspaceNode {
        resolved,
        source,
        doc,
        workspace_docs: ws_docs,
        files_scanned: scanned,
        cache_hits: 1,
        cache_misses: scanned,
    })
}

/// Resolves a file-scoped locator, checking `explicit_ws` for relative paths when needed.
fn resolve_file_scoped_node(
    raw_file: &Path,
    locator: &OrgLocator,
    explicit_ws: Option<&Path>,
    need_ws_docs: bool,
) -> Result<ResolvedWorkspaceNode, String> {
    let target_path = if raw_file.is_file() {
        raw_file.to_path_buf()
    } else if let Some(ws) = explicit_ws.filter(|ws| ws.join(raw_file).is_file()) {
        ws.join(raw_file)
    } else {
        return Err(format!("file not found: {}", raw_file.display()));
    };

    let source = SourceFile::from_path(&target_path)
        .map_err(|e| format!("error reading {}: {e}", target_path.display()))?;
    let doc = OrgDocument::from_source(&source);
    let rebound = rebind_locator_file(locator, &target_path);
    let resolved = resolve_in_document(&rebound, &doc).map_err(|e| e.to_string())?;

    let (workspace_docs, files_scanned, cache_hits) = if need_ws_docs {
        load_peer_workspace_docs(&target_path, &source, explicit_ws)
    } else {
        (Vec::new(), 1, 0)
    };

    Ok(ResolvedWorkspaceNode {
        resolved,
        source,
        doc,
        workspace_docs,
        files_scanned,
        cache_hits,
        cache_misses: files_scanned,
    })
}

/// Loads peer `.org` files in the workspace surrounding `target_path`.
fn load_peer_workspace_docs(
    target_path: &Path,
    target_source: &SourceFile,
    explicit_ws: Option<&Path>,
) -> (Vec<OrgDocument>, usize, usize) {
    let target_norm = normalize_workspace_path(target_path);
    let anchor = target_norm.parent().unwrap_or(&target_norm);
    let files = collect_org_files(&[discover_workspace_root(explicit_ws, anchor)]);
    let mut ws_docs = Vec::with_capacity(files.len().max(1));
    let mut misses = 1usize;
    let mut hits = 0usize;

    for path in &files {
        if normalize_workspace_path(path) == target_norm {
            hits += 1;
            ws_docs.push(OrgDocument::from_source(target_source));
        } else if let Ok(other_src) = SourceFile::from_path(path) {
            misses += 1;
            ws_docs.push(OrgDocument::from_source(&other_src));
        }
    }
    if hits == 0 {
        ws_docs.push(OrgDocument::from_source(target_source));
    }
    (ws_docs, misses, hits)
}

/// Replaces the file path on a file-scoped [`OrgLocator`] while preserving its search target.
fn rebind_locator_file(locator: &OrgLocator, file: &Path) -> OrgLocator {
    match locator {
        OrgLocator::Id(id) => OrgLocator::Id(id.clone()),
        OrgLocator::CustomId { custom_id, .. } => OrgLocator::CustomId {
            file: file.to_path_buf(),
            custom_id: custom_id.clone(),
        },
        OrgLocator::OutlinePath { path, .. } => OrgLocator::OutlinePath {
            file: file.to_path_buf(),
            path: path.clone(),
        },
        OrgLocator::LineRef { line, .. } => OrgLocator::LineRef {
            file: file.to_path_buf(),
            line: *line,
        },
    }
}

/// Counts 1-based logical lines in `content` without counting a trailing newline twice.
fn logical_line_count(content: &str) -> usize {
    if content.is_empty() {
        1
    } else {
        content
            .strip_suffix('\n')
            .unwrap_or(content)
            .split('\n')
            .count()
            .max(1)
    }
}

// endregion

// region Block & Heading Conversion

/// Renders a slice of logical Org lines into normalized GitHub Flavored Markdown.
fn render_org_lines(lines: &[&str], doc: &OrgDocument, ctx: &MarkdownExportContext<'_>) -> String {
    let heading_shift = usize::from(doc.file_title().is_some());
    let kw_owned: Vec<String> = doc
        .todo_keywords
        .all()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let kw_refs: Vec<&str> = kw_owned.iter().map(String::as_str).collect();

    let mut out_blocks: Vec<String> = Vec::new();
    let mut idx = 0;

    while idx < lines.len() {
        let line = lines[idx];
        if let Some((rendered, next_idx)) = try_render_special_block(lines, idx, ctx) {
            push_rendered_chunk(&mut out_blocks, &rendered);
            idx = next_idx;
        } else if is_drawer_open(line) {
            idx = skip_drawer(lines, idx + 1);
        } else if line.trim_start().starts_with('|') {
            let (table_md, next_idx) = consume_and_render_table(lines, idx, ctx);
            push_rendered_chunk(&mut out_blocks, &table_md);
            idx = next_idx;
        } else {
            if let Some(single) = render_single_line(line, heading_shift, &kw_refs, ctx) {
                push_rendered_chunk(&mut out_blocks, &single);
            }
            idx += 1;
        }
    }

    finalize_markdown_lines(&out_blocks)
}

/// Appends `chunk` to `out_blocks`, collapsing consecutive blank lines.
fn push_rendered_chunk(out_blocks: &mut Vec<String>, chunk: &str) {
    if chunk.is_empty() {
        if out_blocks.last().is_some_and(|prev| !prev.is_empty()) {
            out_blocks.push(String::new());
        }
        return;
    }
    out_blocks.push(chunk.to_string());
}

/// Joins rendered chunks, trims trailing blank lines, and ensures a single trailing newline.
fn finalize_markdown_lines(chunks: &[String]) -> String {
    let mut end = chunks.len();
    while end > 0 && chunks[end - 1].is_empty() {
        end -= 1;
    }
    if end == 0 {
        return String::new();
    }
    let mut result = chunks[..end].join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Renders a single non-block, non-table Org line, returning `None` when omitted.
fn render_single_line(
    line: &str,
    heading_shift: usize,
    kw_refs: &[&str],
    ctx: &MarkdownExportContext<'_>,
) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Some(String::new());
    }
    if is_org_comment(trimmed) || is_planning_or_clock(trimmed) {
        return None;
    }
    if let Some((key, val)) = parse_keyword_directive(trimmed) {
        return render_keyword_directive(&key, &val, ctx);
    }
    if let Some(parts) = parse_heading_with_keywords(line, kw_refs) {
        let level = (parts.level + heading_shift).clamp(1, MAX_GFM_HEADING_LEVEL);
        return Some(format!(
            "{} {}",
            "#".repeat(level),
            format_heading_body(&parts, ctx)
        ));
    }
    if let Some(checklist_line) = try_render_checklist_line(line, ctx) {
        return Some(checklist_line);
    }
    Some(convert_inline(line, ctx))
}

/// Formats the text portion of a heading without column-77 tag padding spaces.
fn format_heading_body(parts: &HeadingParts<'_>, ctx: &MarkdownExportContext<'_>) -> String {
    let mut body = String::new();
    if let Some(kw) = parts.keyword {
        body.push_str(kw);
        body.push(' ');
    }
    if let Some(pri) = parts.priority {
        body.push_str(&format!("[#{pri}] "));
    }
    body.push_str(&convert_inline(parts.title.trim(), ctx));
    if !parts.tags.is_empty() {
        body.push_str(&format!(" :{}:", parts.tags.join(":")));
    }
    body
}

/// Renders `#+TITLE:` / `#+SUBTITLE:` directives and omits buffer-setting keywords.
fn render_keyword_directive(
    key: &str,
    val: &str,
    ctx: &MarkdownExportContext<'_>,
) -> Option<String> {
    match key {
        "TITLE" if !val.is_empty() => Some(
            format!("# {}\n", convert_inline(val, ctx))
                .trim_end()
                .to_string(),
        ),
        "SUBTITLE" if !val.is_empty() => Some(format!("*{}*", convert_inline(val, ctx))),
        _ => None,
    }
}

/// Parses a `#+KEY: value` directive line into `(UPPERCASE_KEY, trimmed_value)`.
fn parse_keyword_directive(trimmed: &str) -> Option<(String, String)> {
    let rest = trimmed.strip_prefix("#+")?;
    let colon = rest.find(':')?;
    let key = rest[..colon].trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((
        key.to_ascii_uppercase(),
        rest[colon + 1..].trim().to_string(),
    ))
}

/// Returns `true` when `trimmed` is an Org comment line (`#` followed by whitespace or EOL).
fn is_org_comment(trimmed: &str) -> bool {
    trimmed == "#" || trimmed.starts_with("# ") || trimmed.starts_with("#\t")
}

/// Returns `true` when `trimmed` is a planning (`SCHEDULED:`/`DEADLINE:`/`CLOSED:`) or `CLOCK:` line.
fn is_planning_or_clock(trimmed: &str) -> bool {
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
        || trimmed.starts_with("CLOCK:")
}

/// Returns `true` when `line` opens a `:PROPERTIES:` or `:LOGBOOK:` drawer.
fn is_drawer_open(line: &str) -> bool {
    let t = line.trim();
    t.eq_ignore_ascii_case(":PROPERTIES:") || t.eq_ignore_ascii_case(":LOGBOOK:")
}

/// Advances past `:END:` of a drawer, stopping early if an unclosed drawer hits a heading.
fn skip_drawer(lines: &[&str], mut idx: usize) -> usize {
    while idx < lines.len() {
        let t = lines[idx].trim();
        if t.eq_ignore_ascii_case(":END:") {
            return idx + 1;
        }
        if lines[idx].starts_with('*')
            && lines[idx]
                .trim_start_matches('*')
                .starts_with(char::is_whitespace)
        {
            return idx;
        }
        idx += 1;
    }
    idx
}

/// Attempts to parse and render an Org `#+begin_<kind> ... #+end_<kind>` block at `start`.
fn try_render_special_block(
    lines: &[&str],
    start: usize,
    ctx: &MarkdownExportContext<'_>,
) -> Option<(String, usize)> {
    let trimmed = lines[start].trim_start();
    let after = strip_prefix_ignore_case(trimmed, "#+begin_")?;
    let (kind_raw, args) = split_first_token(after);
    let kind = kind_raw.to_ascii_lowercase();
    let end_marker = format!("#+end_{kind}");

    let mut body = Vec::new();
    let mut idx = start + 1;
    while idx < lines.len() {
        if strip_prefix_ignore_case(lines[idx].trim_start(), &end_marker).is_some() {
            idx += 1;
            break;
        }
        body.push(lines[idx]);
        idx += 1;
    }

    Some((render_block_body(&kind, args, &body, ctx), idx))
}

/// Renders the collected body of a `#+begin_<kind>` block according to its Org block type.
fn render_block_body(
    kind: &str,
    args: &str,
    body: &[&str],
    ctx: &MarkdownExportContext<'_>,
) -> String {
    match kind {
        "src" => render_fenced_code_block(&extract_src_language(args), body),
        "example" => render_fenced_code_block("", body),
        "quote" => body
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    ">".to_string()
                } else {
                    format!("> {}", convert_inline(l, ctx))
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "comment" => String::new(),
        "export" => render_export_block(args, body),
        _ => body
            .iter()
            .map(|l| convert_inline(l, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Extracts the lowercase language identifier from `#+begin_src` arguments, stripping Babel flags.
fn extract_src_language(args: &str) -> String {
    let (lang, _) = split_first_token(args);
    if lang.starts_with(':') || lang.starts_with('-') {
        String::new()
    } else {
        lang.to_ascii_lowercase()
    }
}

/// Wraps verbatim lines in a backtick fence safe against nested backticks.
fn render_fenced_code_block(lang: &str, body: &[&str]) -> String {
    let max_ticks = body
        .iter()
        .map(|l| longest_char_run(l, '`'))
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(MIN_CODE_FENCE_LEN.max(max_ticks + 1));
    let mut out = format!("{fence}{lang}\n");
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&fence);
    out
}

/// Emits verbatim body for Markdown/HTML `#+begin_export` blocks and omits other backends.
fn render_export_block(args: &str, body: &[&str]) -> String {
    let (backend, _) = split_first_token(args);
    if backend.eq_ignore_ascii_case("markdown")
        || backend.eq_ignore_ascii_case("md")
        || backend.eq_ignore_ascii_case("html")
    {
        body.join("\n")
    } else {
        String::new()
    }
}

/// Converts a plain-list checkbox line into GFM task-list syntax (`[ ]`, `[x]`, `[-]`).
fn try_render_checklist_line(line: &str, ctx: &MarkdownExportContext<'_>) -> Option<String> {
    let item = parse_checklist_item(line, 1)?;
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_len];
    let trimmed = &line[indent_len..];
    let bullet_end = trimmed.find([' ', '\t'])?;
    let bullet = &trimmed[..bullet_end];
    let marker = match item.state {
        CheckboxState::Unchecked => "[ ]",
        CheckboxState::Checked => "[x]",
        CheckboxState::Partial => "[-]",
    };
    let text = convert_inline(&item.text, ctx);
    if text.is_empty() {
        Some(format!("{indent}{bullet} {marker}"))
    } else {
        Some(format!("{indent}{bullet} {marker} {text}"))
    }
}

// endregion

// region Table Formatting

/// Collects consecutive `|` pipe lines and formats them as an aligned GFM pipe table.
fn consume_and_render_table(
    lines: &[&str],
    start: usize,
    ctx: &MarkdownExportContext<'_>,
) -> (String, usize) {
    let mut end = start;
    while end < lines.len() && lines[end].trim_start().starts_with('|') {
        end += 1;
    }
    let rows = parse_md_table_rows(&lines[start..end], ctx);
    let Some((header, body, num_cols)) = extract_header_and_body(rows) else {
        return (lines[start..end].join("\n"), end);
    };

    let numeric = detect_numeric_columns(&body, num_cols);
    let widths = compute_gfm_col_widths(&header, &body, &numeric);
    let mut rendered = Vec::with_capacity(body.len() + 2);
    rendered.push(format_gfm_row(&header, &widths, &numeric, false));
    rendered.push(format_gfm_separator(&widths, &numeric));
    for row in &body {
        rendered.push(format_gfm_row(row, &widths, &numeric, true));
    }
    (rendered.join("\n"), end)
}

/// Parses raw Org table lines into separator markers and inline-converted data rows.
fn parse_md_table_rows(raw_lines: &[&str], ctx: &MarkdownExportContext<'_>) -> Vec<MdTableRow> {
    raw_lines
        .iter()
        .map(|line| {
            let trimmed = line.trim();
            if is_org_table_separator(trimmed) {
                MdTableRow::Separator
            } else {
                let inner = trimmed
                    .strip_prefix('|')
                    .unwrap_or(trimmed)
                    .strip_suffix('|')
                    .unwrap_or(trimmed);
                let cells = inner
                    .split('|')
                    .map(|c| convert_inline(c.trim(), ctx))
                    .collect();
                MdTableRow::Data(cells)
            }
        })
        .collect()
}

/// Returns `true` when `trimmed` is an Org horizontal table separator (`|---+---|`).
fn is_org_table_separator(trimmed: &str) -> bool {
    let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
    !inner.is_empty()
        && inner.contains('-')
        && inner
            .chars()
            .all(|c| matches!(c, '-' | '+' | '|' | ':' | ' '))
}

/// Splits parsed rows into normalized `(header, body_rows, num_cols)` and drops extra separators.
fn extract_header_and_body(
    rows: Vec<MdTableRow>,
) -> Option<(Vec<String>, Vec<Vec<String>>, usize)> {
    let mut data_rows: Vec<Vec<String>> = rows
        .into_iter()
        .filter_map(|r| match r {
            MdTableRow::Data(cells) => Some(cells),
            MdTableRow::Separator => None,
        })
        .collect();
    if data_rows.is_empty() {
        return None;
    }
    let num_cols = data_rows.iter().map(Vec::len).max().unwrap_or(0);
    if num_cols == 0 {
        return None;
    }
    for row in &mut data_rows {
        row.resize(num_cols, String::new());
    }
    let header = data_rows.remove(0);
    Some((header, data_rows, num_cols))
}

/// Detects columns whose non-empty body cells are exclusively numeric values.
fn detect_numeric_columns(body: &[Vec<String>], num_cols: usize) -> Vec<bool> {
    (0..num_cols)
        .map(|col| {
            let mut has_value = false;
            for row in body {
                let cell = row[col].trim();
                if cell.is_empty() {
                    continue;
                }
                if !is_numeric_cell(cell) {
                    return false;
                }
                has_value = true;
            }
            has_value
        })
        .collect()
}

/// Returns `true` when `cell` represents a formatted number, currency, or percentage.
fn is_numeric_cell(cell: &str) -> bool {
    let rest = cell.strip_prefix(['-', '+']).unwrap_or(cell);
    !rest.is_empty()
        && rest.chars().any(|c| c.is_ascii_digit())
        && rest
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '%' | '$' | '€'))
}

/// Computes padded display widths for every GFM table column.
fn compute_gfm_col_widths(header: &[String], body: &[Vec<String>], numeric: &[bool]) -> Vec<usize> {
    (0..header.len())
        .map(|col| {
            let min_w = if numeric[col] {
                MIN_GFM_NUMERIC_COL_WIDTH
            } else {
                MIN_GFM_TEXT_COL_WIDTH
            };
            let header_w = UnicodeWidthStr::width(header[col].as_str());
            let body_w = body
                .iter()
                .map(|r| UnicodeWidthStr::width(r[col].as_str()))
                .max()
                .unwrap_or(0);
            min_w.max(header_w).max(body_w)
        })
        .collect()
}

/// Formats a padded GFM table row, right-aligning numeric body cells when requested.
fn format_gfm_row(
    cells: &[String],
    widths: &[usize],
    numeric: &[bool],
    right_align_nums: bool,
) -> String {
    let mut out = String::from("|");
    for (col, cell) in cells.iter().enumerate() {
        let pad = widths[col].saturating_sub(UnicodeWidthStr::width(cell.as_str()));
        let spaces = " ".repeat(pad);
        if right_align_nums && numeric[col] {
            out.push_str(&format!(" {spaces}{cell} |"));
        } else {
            out.push_str(&format!(" {cell}{spaces} |"));
        }
    }
    out
}

/// Formats the GFM alignment separator row (`| --- | ---: |`).
fn format_gfm_separator(widths: &[usize], numeric: &[bool]) -> String {
    let mut out = String::from("|");
    for (col, &w) in widths.iter().enumerate() {
        if numeric[col] {
            out.push_str(&format!(" {}: |", "-".repeat(w - 1)));
        } else {
            out.push_str(&format!(" {} |", "-".repeat(w)));
        }
    }
    out
}

// endregion

// region Inline Markup & Link Resolution

/// Converts inline Org links, code spans (`=..=`/`~..~`/`` `..` ``), and emphasis (`*..*`/`/..`/`+..+`).
fn convert_inline(input: &str, ctx: &MarkdownExportContext<'_>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pos = 0;

    while pos < input.len() {
        let bt_run = count_backticks_at(input, pos);
        if bt_run > 0 {
            if let Some((bt_md, next_pos)) = try_scan_backtick_span(input, pos) {
                out.push_str(&bt_md);
                pos = next_pos;
            } else {
                out.push_str(&input[pos..pos + bt_run]);
                pos += bt_run;
            }
            continue;
        }
        if let Some((link_md, next_pos)) = try_scan_org_link(input, pos, ctx) {
            out.push_str(&link_md);
            pos = next_pos;
            continue;
        }
        if is_pre_border(prev_char(input, pos)) {
            if let Some((code_md, next_pos)) = try_scan_code_span(input, pos) {
                out.push_str(&code_md);
                pos = next_pos;
                continue;
            }
            if let Some((emph_md, next_pos)) = try_scan_emphasis(input, pos, ctx) {
                out.push_str(&emph_md);
                pos = next_pos;
                continue;
            }
        }
        let ch = input[pos..].chars().next().expect("pos within bounds");
        out.push(ch);
        pos += ch.len_utf8();
    }

    out
}

/// Counts contiguous ASCII backticks starting at byte offset `pos` in `s`.
fn count_backticks_at(s: &str, pos: usize) -> usize {
    s.get(pos..)
        .map(|tail| tail.bytes().take_while(|&b| b == b'`').count())
        .unwrap_or(0)
}

/// Scans an existing GFM backtick inline code span starting at `pos` verbatim.
fn try_scan_backtick_span(s: &str, pos: usize) -> Option<(String, usize)> {
    let open_len = count_backticks_at(s, pos);
    if open_len == 0 {
        return None;
    }
    let mut cursor = pos + open_len;
    while let Some(rel) = s.get(cursor..)?.find('`') {
        let idx = cursor + rel;
        let run_len = count_backticks_at(s, idx);
        if run_len == open_len {
            let end = idx + run_len;
            return Some((s[pos..end].to_string(), end));
        }
        cursor = idx + run_len;
    }
    None
}

/// Extracts an opening inline delimiter from `allowed` at `pos` when followed by non-space.
fn opening_span_delim(s: &str, pos: usize, allowed: &[char]) -> Option<char> {
    let delim = s[pos..].chars().next().filter(|c| allowed.contains(c))?;
    let first_inner = s.get(pos + 1..)?.chars().next()?;
    (!first_inner.is_whitespace() && first_inner != delim).then_some(delim)
}

/// Scans an Org inline code/verbatim span (`=...=` or `~...~`) starting at `pos`.
fn try_scan_code_span(s: &str, pos: usize) -> Option<(String, usize)> {
    let delim = opening_span_delim(s, pos, &['=', '~'])?;
    let inner_start = pos + 1;
    let close_idx = find_closing_code_marker(s, inner_start, delim, None)?;
    let inner = &s[inner_start..close_idx];
    let ticks = "`".repeat(longest_char_run(inner, '`') + 1);
    let pad = if inner.starts_with('`') || inner.ends_with('`') {
        " "
    } else {
        ""
    };
    Some((format!("{ticks}{pad}{inner}{pad}{ticks}"), close_idx + 1))
}

/// Scans an Org emphasis span (`*bold*`, `/italic/`, `+strike+`) starting at `pos`.
fn try_scan_emphasis(
    s: &str,
    pos: usize,
    ctx: &MarkdownExportContext<'_>,
) -> Option<(String, usize)> {
    let delim = opening_span_delim(s, pos, &['*', '/', '+'])?;
    let inner_start = pos + 1;
    let close_idx = find_closing_emphasis_marker(s, inner_start, delim)?;
    let converted = convert_inline(&s[inner_start..close_idx], ctx);
    let wrapper = match delim {
        '*' => "**",
        '/' => "*",
        _ => "~~",
    };
    Some((format!("{wrapper}{converted}{wrapper}"), close_idx + 1))
}

/// Finds a valid closing `=` or `~` code delimiter obeying whitespace and post-border rules.
fn find_closing_code_marker(
    s: &str,
    inner_start: usize,
    delim: char,
    outer_delim: Option<char>,
) -> Option<usize> {
    let mut offset = inner_start;
    while let Some(rel) = s[offset..].find(delim) {
        let idx = offset + rel;
        let before = prev_char(s, idx);
        let after = s[idx + 1..].chars().next();
        let valid_after = is_post_border(after) || (outer_delim.is_some() && after == outer_delim);
        if idx > inner_start && before.is_some_and(|c| !c.is_whitespace()) && valid_after {
            return Some(idx);
        }
        offset = idx + 1;
    }
    None
}

/// Returns the byte offset past a nested `[[...]]` link, backtick span, or `=...=`/`~...~` span.
fn skip_nested_verbatim_or_link(s: &str, pos: usize, outer_delim: char) -> Option<usize> {
    if let Some(after_open) = s.get(pos..)?.strip_prefix("[[") {
        let close_rel = after_open.find("]]")?;
        return Some(pos + 2 + close_rel + 2);
    }
    if count_backticks_at(s, pos) > 0 {
        return try_scan_backtick_span(s, pos).map(|(_, end)| end);
    }
    if is_pre_border(prev_char(s, pos)) {
        let code_delim = opening_span_delim(s, pos, &['=', '~'])?;
        let close = find_closing_code_marker(s, pos + 1, code_delim, Some(outer_delim))?;
        return Some(close + 1);
    }
    None
}

/// Finds a closing emphasis delimiter while skipping nested code spans and bracket links.
fn find_closing_emphasis_marker(s: &str, inner_start: usize, delim: char) -> Option<usize> {
    let mut pos = inner_start;
    while pos < s.len() {
        if let Some(next_pos) = skip_nested_verbatim_or_link(s, pos, delim) {
            pos = next_pos;
            continue;
        }
        let ch = s[pos..].chars().next()?;
        let next_idx = pos + ch.len_utf8();
        if ch == delim
            && pos > inner_start
            && prev_char(s, pos).is_some_and(|c| !c.is_whitespace())
            && is_post_border(s[next_idx..].chars().next())
        {
            return Some(pos);
        }
        pos = next_idx;
    }
    None
}

/// Scans an Org bracket link (`[[target]]` or `[[target][description]]`) starting at `pos`.
fn try_scan_org_link(
    s: &str,
    pos: usize,
    ctx: &MarkdownExportContext<'_>,
) -> Option<(String, usize)> {
    let after_open = s.get(pos..)?.strip_prefix("[[")?;
    let close_rel = after_open.find("]]")?;
    let inner = &after_open[..close_rel];
    let next_pos = pos + 2 + close_rel + 2;

    let (target, desc) = match inner.find("][") {
        Some(sep) => (&inner[..sep], Some(&inner[sep + 2..])),
        None => (inner, None),
    };
    Some((render_org_link(target.trim(), desc, ctx), next_pos))
}

/// Converts a parsed `(target, optional_description)` Org link into GFM `[label](href)`.
fn render_org_link(target: &str, desc: Option<&str>, ctx: &MarkdownExportContext<'_>) -> String {
    let converted_desc = desc.map(|d| convert_inline(d, ctx));
    if let Some(uuid) = strip_prefix_ignore_case(target, "id:") {
        return resolve_id_link(uuid.trim(), converted_desc.as_deref(), ctx);
    }
    if let Some(anchor) = target.strip_prefix('#') {
        let label = converted_desc.unwrap_or_else(|| anchor.to_string());
        return format!("[{label}](#{anchor})");
    }
    if let Some(heading) = target.strip_prefix('*') {
        let slug = slugify_heading(heading);
        let label = converted_desc.unwrap_or_else(|| heading.trim().to_string());
        return format!("[{label}](#{slug})");
    }
    let href = rewrite_link_href(target);
    let label = converted_desc.unwrap_or_else(|| href.clone());
    format!("[{label}]({href})")
}

/// Resolves `[[id:<uuid>]]` across `ctx.workspace_docs` with graceful `#id-<uuid>` fallback.
fn resolve_id_link(uuid: &str, desc: Option<&str>, ctx: &MarkdownExportContext<'_>) -> String {
    let locator = OrgLocator::Id(uuid.to_string());
    for doc in ctx.workspace_docs {
        if let Ok(resolved) = resolve_in_document(&locator, doc) {
            let label = desc
                .map(str::to_string)
                .unwrap_or_else(|| resolved.heading_text.clone());
            let same_file = paths_refer_to_same_file(ctx.current_file, &doc.file);
            if resolved.is_file_node() {
                let href = if same_file {
                    format!("#{}", slugify_heading(&resolved.heading_text))
                } else {
                    relative_md_path(ctx.current_file, &doc.file)
                };
                return format!("[{label}]({href})");
            }
            let slug = entry_anchor_slug(doc, resolved.entry_index, ctx);
            let href = if same_file {
                format!("#{slug}")
            } else {
                format!("{}#{slug}", relative_md_path(ctx.current_file, &doc.file))
            };
            return format!("[{label}]({href})");
        }
    }

    let fallback_label = desc
        .map(str::to_string)
        .unwrap_or_else(|| format!("id:{uuid}"));
    format!("[{fallback_label}](#id-{uuid})")
}

/// Computes the GFM heading anchor slug for `doc.entries[entry_idx]`.
fn entry_anchor_slug(
    doc: &OrgDocument,
    entry_idx: usize,
    ctx: &MarkdownExportContext<'_>,
) -> String {
    let entry = &doc.entries[entry_idx];
    let target_ctx = MarkdownExportContext {
        workspace_docs: ctx.workspace_docs,
        current_file: &doc.file,
    };
    let parts = HeadingParts {
        level: entry.level,
        keyword: entry.keyword.as_deref(),
        priority: entry.priority,
        title: &entry.title,
        tags: entry.tags.iter().map(String::as_str).collect(),
    };
    slugify_heading(&format_heading_body(&parts, &target_ctx))
}

/// Rewrites `file:foo.org` and local `.org` targets (with optional `::` anchors) to `.md`.
fn rewrite_link_href(target: &str) -> String {
    if ["http://", "https://", "mailto:", "ftp://"]
        .iter()
        .any(|scheme| target.starts_with(scheme))
    {
        return target.to_string();
    }
    let stripped = strip_prefix_ignore_case(target, "file:").unwrap_or(target);
    let (path_part, search) = match stripped.find("::") {
        Some(idx) => (&stripped[..idx], Some(&stripped[idx + 2..])),
        None => (stripped, None),
    };
    let md_path = replace_org_ext_with_md(path_part);
    match search {
        Some(s) if s.starts_with('#') => format!("{md_path}{s}"),
        Some(s) => {
            let clean = s.trim_start_matches("*/").trim_start_matches('*');
            let tail = clean.rsplit('/').next().unwrap_or(clean);
            format!("{md_path}#{}", slugify_heading(tail))
        }
        None => md_path,
    }
}

/// Computes a forward-slash relative `.md` path from `from_file`'s directory to `to_file`.
fn relative_md_path(from_file: &Path, to_file: &Path) -> String {
    let norm_from = normalize_workspace_path(from_file);
    let norm_to = normalize_workspace_path(to_file);
    let from_dir = norm_from.parent().unwrap_or(&norm_from);
    let from_comps: Vec<Component<'_>> = from_dir.components().collect();
    let to_comps: Vec<Component<'_>> = norm_to.components().collect();

    let common = from_comps
        .iter()
        .zip(to_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common == 0 {
        let fallback = norm_to
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        return replace_org_ext_with_md(fallback);
    }

    let mut rel = PathBuf::new();
    for _ in common..from_comps.len() {
        rel.push("..");
    }
    for comp in &to_comps[common..] {
        rel.push(comp);
    }
    let raw = rel.to_string_lossy().replace('\\', "/");
    replace_org_ext_with_md(&raw)
}

/// Returns `true` when two paths refer to the same source file.
fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    normalize_workspace_path(a) == normalize_workspace_path(b)
}

/// Replaces a trailing `.org` extension (case-insensitive) with `.md` safely across UTF-8.
fn replace_org_ext_with_md(path: &str) -> String {
    let split_at = path.len().saturating_sub(4);
    if path
        .get(split_at..)
        .is_some_and(|ext| ext.eq_ignore_ascii_case(".org"))
    {
        format!("{}.md", &path[..split_at])
    } else {
        path.to_string()
    }
}

/// Strips ASCII case-insensitive `prefix` from `s` without panicking on multi-byte UTF-8.
fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    {
        s.get(prefix.len()..)
    } else {
        None
    }
}

/// Splits `s` into `(first_token, remaining_trimmed)`.
fn split_first_token(s: &str) -> (&str, &str) {
    let trimmed = s.trim();
    match trimmed.find(char::is_whitespace) {
        Some(idx) => (&trimmed[..idx], trimmed[idx..].trim()),
        None => (trimmed, ""),
    }
}

/// Returns the character immediately preceding byte offset `pos` in `s`.
fn prev_char(s: &str, pos: usize) -> Option<char> {
    s[..pos].chars().next_back()
}

/// Returns `true` if `ch` is a valid Org emphasis pre-border character (or start of line).
fn is_pre_border(ch: Option<char>) -> bool {
    match ch {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '-' | '(' | '{' | '\'' | '"' | '[' | '|'),
    }
}

/// Returns `true` if `ch` is a valid Org emphasis post-border character (or end of line).
fn is_post_border(ch: Option<char>) -> bool {
    match ch {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '.' | ',' | ';' | ':' | '!' | '?' | '\'' | ')' | '}' | '"' | ']' | '|'
                )
        }
    }
}

/// Computes the longest contiguous run of `target` characters in `s`.
fn longest_char_run(s: &str, target: char) -> usize {
    let mut best = 0;
    let mut cur = 0;
    for ch in s.chars() {
        if ch == target {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

/// Strips inline Markdown link wrappers and emphasis markers before slugifying a heading.
fn strip_markdown_inline_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with('[') {
            if let Some(mid) = text[i + 1..].find("](") {
                let label_end = i + 1 + mid;
                if let Some(close) = text[label_end + 2..].find(')') {
                    out.push_str(&text[i + 1..label_end]);
                    i = label_end + 2 + close + 1;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().expect("index within str");
        if !matches!(ch, '`' | '*' | '~') {
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    out
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies `slugify_heading` normalizes mixed case, punctuation, and Markdown code spans.
    #[test]
    fn slugify_heading_normalizes_markdown_and_punctuation() {
        assert_eq!(slugify_heading("My Heading"), "my-heading");
        assert_eq!(
            slugify_heading("Lint & Format (`org fmt`)"),
            "lint-format-org-fmt"
        );
    }

    /// Verifies code fences automatically expand when literal content contains triple backticks.
    #[test]
    fn export_expands_fence_for_nested_backticks() {
        let md = export_slice_to_markdown(
            "#+begin_src markdown\n```rust\nfn main() {}\n```\n#+end_src\n",
            Path::new("nested.org"),
            &[],
        );
        assert!(md.starts_with("````markdown\n```rust\n"));
        assert!(md.ends_with("\n````\n"));
    }

    /// Verifies unlinked file paths are not corrupted by `/italic/` emphasis conversion.
    #[test]
    fn inline_scanner_preserves_unlinked_paths_and_urls() {
        let md = export_slice_to_markdown(
            "Inspect crates/org/src/main.rs and /real italic/ plus *bold*.\n",
            Path::new("doc.org"),
            &[],
        );
        assert_eq!(
            md,
            "Inspect crates/org/src/main.rs and *real italic* plus **bold**.\n"
        );
    }

    /// Verifies multi-byte UTF-8 prefixes and link targets never panic on byte-index checks.
    #[test]
    fn multibyte_utf8_links_and_lines_export_without_char_boundary_panic() {
        let md = export_slice_to_markdown(
            "🚀 line and [[🚀x][Rocket]] plus [[日本語.org][Doc]].\n",
            Path::new("utf8.org"),
            &[],
        );
        assert_eq!(md, "🚀 line and [Rocket](🚀x) plus [Doc](日本語.md).\n");
    }

    /// Verifies inline backtick spans stay verbatim and nested code/links do not break emphasis.
    #[test]
    fn preserve_inline_backtick_spans_and_nested_code_in_emphasis() {
        let backticks = export_slice_to_markdown(
            "Keep `foo *bar* /baz/ [[https://example.com]]` and ``x ` *y*`` verbatim.\n",
            Path::new("bt.org"),
            &[],
        );
        assert_eq!(
            backticks,
            "Keep `foo *bar* /baz/ [[https://example.com]]` and ``x ` *y*`` verbatim.\n"
        );

        let nested = export_slice_to_markdown(
            "*bold with =foo* bar= end*, *bold =foo*=*, and /italic with [[https://example.com/][site]] end/\n",
            Path::new("emph.org"),
            &[],
        );
        assert_eq!(
            nested,
            "**bold with `foo* bar` end**, **bold `foo*`**, and *italic with [site](https://example.com/) end*\n"
        );
    }

    /// Verifies uppercase `#+BEGIN_SRC MERMAID` normalizes the language token to `mermaid`.
    #[test]
    fn uppercase_begin_src_mermaid_lowercases_language_fence() {
        let md = export_slice_to_markdown(
            "#+BEGIN_SRC MERMAID :file diag.svg\ngraph TD\n  A --> B\n#+END_SRC\n",
            Path::new("diag.org"),
            &[],
        );
        assert_eq!(md, "```mermaid\ngraph TD\n  A --> B\n```\n");
    }

    /// Verifies `discover_workspace_root` resolves relative anchors without returning empty paths.
    #[test]
    fn discover_workspace_root_never_returns_empty_path_for_relative_anchor() {
        let root = discover_workspace_root(None, Path::new("specs"));
        assert!(!root.as_os_str().is_empty());
        assert!(root.is_absolute());
    }
}

// endregion
