// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-file `#+INCLUDE:` directive parser, workspace-confined resolver, and recursive expander.
//!
//! Spec: [§12.4 Include Files](https://orgmode.org/manual/Include-Files.html).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::export_md::normalize_workspace_path;
use crate::files::canonicalize_clean;
use crate::locator::{resolve_in_document, OrgLocator};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::source::SourceFile;

// region Error & Directive Types

/// Errors produced when parsing, validating, or expanding an `#+INCLUDE:` directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeError {
    /// Target file `(path, workspace_root)` resolves outside the confined workspace root.
    OutsideWorkspace(PathBuf, PathBuf),
    /// Target file `(raw_path, resolved_path)` does not exist on disk.
    MissingFile(PathBuf, PathBuf),
    /// Recursive `#+INCLUDE:` cycle detected across the active include stack.
    CycleDetected(Vec<String>),
    /// Subtree target `(file, target)` was not found in the included file.
    UnresolvedTarget(PathBuf, String),
    /// Malformed `#+INCLUDE:` directive syntax or parameter value.
    InvalidDirective(String),
    /// Filesystem I/O error `(path, message)` reading an included file.
    IoError(PathBuf, String),
}

impl fmt::Display for IncludeError {
    /// Formats a human-readable diagnostic message for the include error.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideWorkspace(p, w) => {
                write!(f, "#+INCLUDE: '{}' outside '{}'", p.display(), w.display())
            }
            Self::MissingFile(p, _) => write!(f, "#+INCLUDE: '{}' does not exist", p.display()),
            Self::CycleDetected(c) => {
                write!(f, "#+INCLUDE: recursive include cycle: {}", c.join(" -> "))
            }
            Self::UnresolvedTarget(p, t) => {
                write!(f, "#+INCLUDE: unresolved target '{t}' in '{}'", p.display())
            }
            Self::InvalidDirective(msg) => write!(f, "#+INCLUDE: {msg}"),
            Self::IoError(p, msg) => write!(f, "#+INCLUDE: error reading '{}': {msg}", p.display()),
        }
    }
}

impl std::error::Error for IncludeError {}

/// Parsed `#+INCLUDE:` directive and its optional extraction parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeDirective {
    /// Relative file path before any `::` search suffix.
    pub raw_path: String,
    /// Optional search option following `::` (e.g. `*Section`, `#custom-id`, `id:...`).
    pub search_option: Option<String>,
    /// Optional wrapper block kind and arguments (e.g. `("src", Some("rust"))`).
    pub block_name: Option<(String, Option<String>)>,
    /// Optional minimum heading level (`:minlevel N`).
    pub minlevel: Option<usize>,
    /// Optional 1-based inclusive `(start, end)` line range (`:lines "A-B"`).
    pub lines: Option<(Option<usize>, Option<usize>)>,
    /// Whether `:only-contents t` was specified to omit the target heading line itself.
    pub only_contents: bool,
}

/// Formats a compact `(file, search_option)` label for cycle diagnostics.
fn frame_label((file, search): &(PathBuf, Option<String>)) -> String {
    let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("?");
    search
        .as_ref()
        .map_or_else(|| name.to_string(), |t| format!("{name}::{t}"))
}

// endregion

// region Directive Parsing

/// Parses a single line as an `#+INCLUDE:` directive if it starts with `#+INCLUDE:` (case-insensitive).
pub fn parse_include_directive(line: &str) -> Option<Result<IncludeDirective, IncludeError>> {
    let t = line.trim_start();
    (t.len() >= 10 && t.starts_with("#+") && t[2..10].eq_ignore_ascii_case("INCLUDE:"))
        .then(|| parse_include_body(t[10..].trim()))
}

/// Parses the arguments following `#+INCLUDE:`.
fn parse_include_body(rest: &str) -> Result<IncludeDirective, IncludeError> {
    let tokens = tokenize_include_args(rest)?;
    let spec = tokens
        .first()
        .ok_or_else(|| IncludeError::InvalidDirective("missing file path".into()))?;
    let (raw_path, search_option) = match spec.split_once("::") {
        Some((_, s)) if s.trim().is_empty() => {
            return Err(IncludeError::InvalidDirective(
                "empty target after '::'".into(),
            ));
        }
        Some((p, s)) => (p.trim().to_string(), Some(s.trim().to_string())),
        None => (spec.trim().to_string(), None),
    };
    if raw_path.is_empty() {
        return Err(IncludeError::InvalidDirective("missing file path".into()));
    }
    parse_include_options(raw_path, search_option, &tokens[1..])
}

/// Tokenizes the arguments of an `#+INCLUDE:` line, preserving quoted strings.
fn tokenize_include_args(tail: &str) -> Result<Vec<String>, IncludeError> {
    let (mut tokens, mut rest) = (Vec::new(), tail.trim_start());
    while !rest.is_empty() {
        if let Some(after_q) = rest.strip_prefix('"') {
            let (tok, rem) = after_q
                .split_once('"')
                .ok_or_else(|| IncludeError::InvalidDirective("unclosed quote".into()))?;
            tokens.push(tok.to_string());
            rest = rem.trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            tokens.push(rest[..end].to_string());
            rest = rest[end..].trim_start();
        }
    }
    Ok(tokens)
}

/// Parses optional block name and `:minlevel`, `:lines`, `:only-contents` keyword arguments.
fn parse_include_options(
    raw_path: String,
    search_option: Option<String>,
    tokens: &[String],
) -> Result<IncludeDirective, IncludeError> {
    let (mut idx, mut block_name) = (0, None);
    if tokens.first().is_some_and(|t| !t.starts_with(':')) {
        let kind = tokens[0].to_ascii_lowercase();
        idx = 1;
        let arg = ((kind == "src" || kind == "export")
            && tokens.get(1).is_some_and(|t| !t.starts_with(':')))
        .then(|| {
            idx = 2;
            tokens[1].clone()
        });
        block_name = Some((kind, arg));
    }
    let (mut minlevel, mut lines, mut only_contents) = (None, None, false);
    while idx < tokens.len() {
        let val = tokens.get(idx + 1).map(String::as_str);
        match tokens[idx].to_ascii_lowercase().as_str() {
            ":minlevel" => {
                let v =
                    val.ok_or_else(|| IncludeError::InvalidDirective("missing :minlevel".into()))?;
                minlevel = Some(parse_positive_usize(v, ":minlevel")?);
                idx += 2;
            }
            ":lines" => {
                let v =
                    val.ok_or_else(|| IncludeError::InvalidDirective("missing :lines".into()))?;
                lines = Some(parse_lines_range(v)?);
                idx += 2;
            }
            ":only-contents" => {
                if let Some(next) = val.filter(|t| !t.starts_with(':')) {
                    only_contents = !next.eq_ignore_ascii_case("nil") && next != "false";
                    idx += 2;
                } else {
                    only_contents = true;
                    idx += 1;
                }
            }
            _ => idx += 1,
        }
    }
    Ok(IncludeDirective {
        raw_path,
        search_option,
        block_name,
        minlevel,
        lines,
        only_contents,
    })
}

/// Parses a positive integer (`>= 1`) for an `#+INCLUDE:` parameter.
fn parse_positive_usize(raw: &str, flag: &str) -> Result<usize, IncludeError> {
    raw.parse::<usize>()
        .ok()
        .filter(|&n| n >= 1)
        .ok_or_else(|| IncludeError::InvalidDirective(format!("invalid {flag} value '{raw}'")))
}

/// Parses `:lines "A-B"` into a 1-based `(start, end)` pair.
fn parse_lines_range(raw: &str) -> Result<(Option<usize>, Option<usize>), IncludeError> {
    let (l, r) = raw
        .trim()
        .trim_matches('"')
        .split_once('-')
        .ok_or_else(|| IncludeError::InvalidDirective(format!("invalid :lines '{raw}'")))?;
    let start = (!l.trim().is_empty())
        .then(|| parse_positive_usize(l.trim(), ":lines"))
        .transpose()?;
    let end = (!r.trim().is_empty())
        .then(|| parse_positive_usize(r.trim(), ":lines"))
        .transpose()?;
    Ok((start, end))
}

// endregion

// region Target Extraction & Level Shifting

/// Splits content into logical lines without a trailing empty line after the final `\n`.
fn split_logical_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

/// Extracts the subtree lines for `search` from `src`.
fn extract_search_target_lines(
    src: &SourceFile,
    all: &[String],
    search: &str,
    only_contents: bool,
) -> Result<Vec<String>, IncludeError> {
    let doc = OrgDocument::from_source(src);
    let idx = find_target_entry_index(&doc, search)
        .ok_or_else(|| IncludeError::UnresolvedTarget(src.path.clone(), search.to_string()))?;
    let entry = &doc.entries[idx];
    let start = entry.heading_line + usize::from(only_contents);
    let end = entry.subtree_end_line.saturating_sub(1).min(all.len());
    let slice = all.get(start.saturating_sub(1)..end).filter(|_| start > 0);
    Ok(slice.map(<[String]>::to_vec).unwrap_or_default())
}

/// Finds the matching entry index in `doc` for `*Heading`, `*/Outline`, `#custom-id`, or `id:<uuid>`.
fn find_target_entry_index(doc: &OrgDocument, search: &str) -> Option<usize> {
    if let Some(cid) = search.strip_prefix('#') {
        return doc.find_by_custom_id(cid.trim());
    }
    if let Some(uuid) = search.strip_prefix("id:") {
        return doc.find_by_id(uuid.trim());
    }
    if let Some(outline) = search.strip_prefix("*/") {
        let path = outline
            .split('/')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let loc = OrgLocator::OutlinePath {
            file: doc.file.clone(),
            path,
        };
        if let Ok(res) = resolve_in_document(&loc, doc) {
            return doc.entries.get(res.entry_index).map(|_| res.entry_index);
        }
    }
    let wanted = search.trim_start_matches('*').trim();
    doc.entries
        .iter()
        .position(|e| e.title.eq_ignore_ascii_case(wanted))
}

/// Shifts heading stars in `lines` so the minimum non-protected heading level becomes `minlevel`.
fn apply_minlevel_shift(mut lines: Vec<String>, minlevel: usize) -> Vec<String> {
    let protected = protected_regions(&lines.join("\n"));
    let base = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !is_protected(*i, &protected))
        .filter_map(|(_, l)| heading_star_count(l))
        .min();
    if let Some(base) = base {
        for (i, line) in lines.iter_mut().enumerate() {
            if let Some(s) = heading_star_count(line).filter(|_| !is_protected(i, &protected)) {
                let stars = "*".repeat(minlevel + s.saturating_sub(base));
                *line = format!("{stars}{}", &line[s..]);
            }
        }
    }
    lines
}

/// Returns the number of leading `*` characters if `line` is an Org heading line.
fn heading_star_count(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    (stars > 0 && line[stars..].starts_with([' ', '\t'])).then_some(stars)
}

// endregion

// region Workspace Confinement & Recursive Expansion

/// Resolves and confines `raw` relative to `from` within `ws`.
fn resolve_confined_target(raw: &str, from: &Path, ws: &Path) -> Result<PathBuf, IncludeError> {
    let outside = || IncludeError::OutsideWorkspace(raw.into(), ws.to_path_buf());
    let cand = from.parent().unwrap_or_else(|| Path::new(".")).join(raw);
    let norm = normalize_workspace_path(&cand);
    if !norm.starts_with(ws) {
        return Err(outside());
    }
    if !cand.is_file() {
        return Err(IncludeError::MissingFile(raw.into(), cand));
    }
    let canon = canonicalize_clean(&cand).unwrap_or(norm);
    canon.starts_with(ws).then_some(canon).ok_or_else(outside)
}

/// Expands all non-protected `#+INCLUDE:` directives in `content` recursively.
pub fn expand_includes(
    content: &str,
    including_file: &Path,
    workspace_root: Option<&Path>,
) -> Result<String, IncludeError> {
    let default_ws = including_file.parent().unwrap_or_else(|| Path::new("."));
    let ws = normalize_workspace_path(workspace_root.unwrap_or(default_ws));
    let mut stack = vec![(normalize_workspace_path(including_file), None)];
    let lines = split_logical_lines(content);
    let expanded = expand_lines_recursive(&lines, including_file, &ws, &mut stack)?;
    let mut out = expanded.join("\n");
    if content.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// Validates all non-protected `#+INCLUDE:` directives in `source`, returning `(line, col, error)`.
pub fn validate_file_includes(
    source: &SourceFile,
    workspace_root: Option<&Path>,
) -> Vec<(usize, usize, IncludeError)> {
    let default_ws = source.path.parent().unwrap_or_else(|| Path::new("."));
    let ws = normalize_workspace_path(workspace_root.unwrap_or(default_ws));
    let top_file = normalize_workspace_path(&source.path);
    let protected = protected_regions(&source.content);
    let (mut errors, mut offset) = (Vec::new(), 0usize);

    for (idx, raw_line) in source.content.split('\n').enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if !is_protected(idx, &protected) {
            if let Some(parsed) = parse_include_directive(line) {
                let (ln, col) = source.line_col(offset + line.len() - line.trim_start().len());
                let mut stack = vec![(top_file.clone(), None)];
                if let Err(e) =
                    parsed.and_then(|d| expand_single(&d, &source.path, &ws, &mut stack))
                {
                    errors.push((ln, col, e));
                }
            }
        }
        offset += raw_line.len() + 1;
    }
    errors
}

/// Recursively expands non-protected `#+INCLUDE:` lines inside `lines`.
fn expand_lines_recursive(
    lines: &[String],
    including_file: &Path,
    ws_root: &Path,
    stack: &mut Vec<(PathBuf, Option<String>)>,
) -> Result<Vec<String>, IncludeError> {
    let protected = protected_regions(&lines.join("\n"));
    let mut out = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        if !is_protected(idx, &protected) {
            if let Some(parsed) = parse_include_directive(line) {
                out.extend(expand_single(&parsed?, including_file, ws_root, stack)?);
                continue;
            }
        }
        out.push(line.clone());
    }
    Ok(out)
}

/// Resolves, slices, shifts, and recursively expands a single [`IncludeDirective`].
fn expand_single(
    dir: &IncludeDirective,
    including_file: &Path,
    ws_root: &Path,
    stack: &mut Vec<(PathBuf, Option<String>)>,
) -> Result<Vec<String>, IncludeError> {
    let target = resolve_confined_target(&dir.raw_path, including_file, ws_root)?;
    let next = (target.clone(), dir.search_option.clone());
    if stack
        .iter()
        .any(|(f, s)| f == &next.0 && (s.is_none() || next.1.is_none() || s == &next.1))
    {
        let mut cycle: Vec<String> = stack.iter().map(frame_label).collect();
        cycle.push(frame_label(&next));
        return Err(IncludeError::CycleDetected(cycle));
    }
    let src = SourceFile::from_path(&target)
        .map_err(|e| IncludeError::IoError(target.clone(), e.to_string()))?;
    let all = split_logical_lines(&src.content);
    let mut sel = match &dir.search_option {
        Some(s) => extract_search_target_lines(&src, &all, s, dir.only_contents)?,
        None => all,
    };
    if let Some((s, e)) = dir.lines {
        let (start, end) = (s.unwrap_or(1).max(1), e.unwrap_or(sel.len()).min(sel.len()));
        sel = sel
            .get(start - 1..end)
            .map(<[String]>::to_vec)
            .unwrap_or_default();
    }
    if let Some((kind, arg)) = &dir.block_name {
        let open = arg.as_ref().map_or_else(
            || format!("#+begin_{kind}"),
            |a| format!("#+begin_{kind} {a}"),
        );
        return Ok(std::iter::once(open)
            .chain(sel)
            .chain(std::iter::once(format!("#+end_{kind}")))
            .collect());
    }
    if let Some(minlevel) = dir.minlevel {
        sel = apply_minlevel_shift(sel, minlevel);
    }
    stack.push(next);
    let res = expand_lines_recursive(&sel, &target, ws_root, stack);
    stack.pop();
    res
}

// endregion
