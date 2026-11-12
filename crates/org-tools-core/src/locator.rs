// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=780 reason="Cohesive OrgLocator parser, formatter, document/filesystem resolvers, and unit test suite"

//! Org Item Locator — a unique identifier for headings and file nodes in org files.
//!
//! An [`OrgLocator`] uniquely identifies an entry or level-0 `org-roam` v2 file
//! node across a tree of org files. Four variants exist, ordered by stability:
//!
//! | Pattern | Example | Stability |
//! |---|---|---|
//! | `id:<uuid>` | `id:550e8400-e29b-...` | Survives moves |
//! | `<file>::#<custom-id>` | `notes.org::#project-x` | File-scoped |
//! | `<file>::*/<path>` | `todo.org::*/Work/Meeting` | Fragile on rename |
//! | `<file>::<line>` | `inbox.org::42` | Fragile on edit |
//!
//! Spec: [§4.2 Internal Links](https://orgmode.org/manual/Internal-Links.html),
//! [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)

use std::fmt;
use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::files::collect_org_files;
use crate::source::SourceFile;

// region Public Types

/// Heading level assigned to a level-0 file node (`:PROPERTIES:` before any heading).
pub const FILE_NODE_LEVEL: usize = 0;

/// Sentinel [`ResolvedEntry::entry_index`] value representing a level-0 file node.
pub const FILE_NODE_ENTRY_INDEX: usize = usize::MAX;

/// A unique identifier for an org heading entry or level-0 file node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgLocator {
    /// UUID-based: `id:550e8400-e29b-41d4-a716-446655440000`.
    /// Globally unique — found via a heading or top-of-file `:ID:` property.
    /// Survives file moves and heading renames.
    Id(String),

    /// File-scoped custom ID: `path/to/file.org::#custom-id`.
    /// Unique within a file — found via the `:CUSTOM_ID:` property.
    CustomId {
        /// Path to the org file.
        file: PathBuf,
        /// `CUSTOM_ID` value (without `#` prefix).
        custom_id: String,
    },

    /// Outline path: `path/to/file.org::*/Top/Sub/Entry`.
    /// Matches heading titles joined by `/` with a `*` prefix.
    OutlinePath {
        /// Path to the org file.
        file: PathBuf,
        /// Heading title segments from root to target.
        path: Vec<String>,
    },

    /// Line reference: `path/to/file.org::42`.
    /// Refers to the heading at or before the given line.
    /// Fragile — changes on any edit above the target.
    LineRef {
        /// Path to the org file.
        file: PathBuf,
        /// 1-based line number.
        line: usize,
    },
}

/// A successfully resolved locator pointing to a concrete file and entry (or file node).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntry {
    /// Path to the org file.
    pub file: PathBuf,
    /// 1-based line number of the heading (or `:PROPERTIES:` drawer start line for file nodes).
    pub line: usize,
    /// Heading title text (or `#+TITLE:` / file stem for level-0 file nodes).
    pub heading_text: String,
    /// Heading level (`1` = top-level heading, `0` = file-level node).
    pub level: usize,
    /// Index into [`OrgDocument::entries`], or [`FILE_NODE_ENTRY_INDEX`] (`usize::MAX`) for file nodes.
    pub entry_index: usize,
}

impl ResolvedEntry {
    /// Returns `true` when this resolved target is a level-0 file node rather than a heading.
    pub fn is_file_node(&self) -> bool {
        self.level == FILE_NODE_LEVEL
    }
}

/// Errors that can occur during locator parsing or resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocatorError {
    /// The locator string could not be parsed.
    InvalidFormat(String),
    /// The referenced file does not exist.
    FileNotFound(PathBuf),
    /// No entry matches the locator.
    EntryNotFound(String),
    /// I/O error reading a file.
    IoError(String),
}

impl fmt::Display for LocatorError {
    /// Formats a human-readable diagnostic description of the locator error.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(s) => write!(f, "invalid locator format: {s}"),
            Self::FileNotFound(p) => write!(f, "file not found: {}", p.display()),
            Self::EntryNotFound(s) => write!(f, "entry not found: {s}"),
            Self::IoError(s) => write!(f, "I/O error: {s}"),
        }
    }
}

impl std::error::Error for LocatorError {}

// endregion

// region Public API

impl OrgLocator {
    /// Parse a locator string from CLI input.
    ///
    /// Accepted formats:
    /// - `id:<uuid>` — ID-based
    /// - `<file>::#<custom-id>` — CUSTOM_ID-based
    /// - `<file>::*/<path>` — outline path
    /// - `<file>::<number>` — line reference
    pub fn parse(input: &str) -> Result<Self, LocatorError> {
        let input = input.trim();

        if let Some(uuid) = input.strip_prefix("id:") {
            let uuid = uuid.trim();
            if uuid.is_empty() {
                return Err(LocatorError::InvalidFormat(
                    "id: locator has empty UUID".to_string(),
                ));
            }
            return Ok(Self::Id(uuid.to_string()));
        }

        if let Some(sep_pos) = input.find("::") {
            return parse_file_scoped_locator(
                PathBuf::from(&input[..sep_pos]),
                &input[sep_pos + 2..],
            );
        }
        Err(LocatorError::InvalidFormat(format!(
            "locator must start with 'id:' or contain '::': {input}"
        )))
    }

    /// Render the locator as a canonical string.
    pub fn to_canonical_string(&self) -> String {
        match self {
            Self::Id(uuid) => format!("id:{uuid}"),
            Self::CustomId { file, custom_id } => {
                format!("{}::#{custom_id}", file.display())
            }
            Self::OutlinePath { file, path } => {
                format!("{}::*/{}", file.display(), path.join("/"))
            }
            Self::LineRef { file, line } => {
                format!("{}::{line}", file.display())
            }
        }
    }
}

impl fmt::Display for OrgLocator {
    /// Formats the locator in canonical CLI link syntax.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_canonical_string())
    }
}

/// Resolve a locator against an already-parsed [`OrgDocument`].
///
/// Supports both heading entries and level-0 `org-roam` v2 file `:ID:` drawers.
/// For a level-0 file match, returns a [`ResolvedEntry`] with `level == 0`,
/// `line` set to the 1-based `:PROPERTIES:` drawer start line (or `1`),
/// `heading_text` set to `#+TITLE:` (falling back to the file stem), and
/// `entry_index == usize::MAX` ([`FILE_NODE_ENTRY_INDEX`]).
pub fn resolve_in_document(
    locator: &OrgLocator,
    doc: &OrgDocument,
) -> Result<ResolvedEntry, LocatorError> {
    match locator {
        OrgLocator::Id(id) => resolve_id_in_doc(doc, id),
        OrgLocator::CustomId { file, custom_id } => resolve_custom_id_in_doc(doc, file, custom_id),
        OrgLocator::OutlinePath { file, path } => resolve_outline_path_in_doc(doc, file, path),
        OrgLocator::LineRef { file, line } => resolve_line_ref_in_doc(doc, file, *line),
    }
}

/// Resolve a locator to a concrete file and entry (or level-0 file node).
///
/// For [`OrgLocator::Id`] locators, scans all `.org` files in `search_paths`
/// for a matching file-level or heading `:ID:` property. For file-scoped
/// variants, searches within the specified file.
pub fn resolve_locator(
    locator: &OrgLocator,
    search_paths: &[PathBuf],
) -> Result<ResolvedEntry, LocatorError> {
    match locator {
        OrgLocator::Id(id) => resolve_id_across_paths(id, search_paths),
        OrgLocator::CustomId { file, .. }
        | OrgLocator::OutlinePath { file, .. }
        | OrgLocator::LineRef { file, .. } => resolve_file_scoped(file, locator),
    }
}

/// Generate the most specific locator for an entry (or file node when `entry_idx` is out of bounds).
///
/// Prefers `ID` > `CUSTOM_ID` > `OutlinePath`.
pub fn locator_for_entry(doc: &OrgDocument, entry_idx: usize) -> OrgLocator {
    let Some(entry) = doc.entries.get(entry_idx) else {
        if let Some(file_id) = doc.file_id() {
            return OrgLocator::Id(file_id.to_string());
        }
        return OrgLocator::LineRef {
            file: doc.file.clone(),
            line: doc.file_properties_line.unwrap_or(1),
        };
    };

    if let Some(id) = entry.properties.get("ID") {
        OrgLocator::Id(id.clone())
    } else if let Some(custom_id) = entry.properties.get("CUSTOM_ID") {
        OrgLocator::CustomId {
            file: doc.file.clone(),
            custom_id: custom_id.clone(),
        }
    } else {
        OrgLocator::OutlinePath {
            file: doc.file.clone(),
            path: doc
                .outline_path(entry_idx)
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }
}

// endregion

// region Resolution Helpers

/// Parses the `<file>::<search>` portion of a locator string.
fn parse_file_scoped_locator(file: PathBuf, search: &str) -> Result<OrgLocator, LocatorError> {
    if file.as_os_str().is_empty() {
        return Err(LocatorError::InvalidFormat(
            "empty file path before ::".to_string(),
        ));
    }

    if let Some(custom_id) = search.strip_prefix('#') {
        let custom_id = custom_id.trim();
        return if custom_id.is_empty() {
            Err(LocatorError::InvalidFormat(
                "empty CUSTOM_ID after ::#".to_string(),
            ))
        } else {
            Ok(OrgLocator::CustomId {
                file,
                custom_id: custom_id.to_string(),
            })
        };
    }

    if let Some(path_str) = search.strip_prefix("*/") {
        return parse_outline_segments(file, path_str);
    }

    if let Ok(line) = search.trim().parse::<usize>() {
        if line == 0 {
            return Err(LocatorError::InvalidFormat(
                "line number must be >= 1".to_string(),
            ));
        }
        return Ok(OrgLocator::LineRef { file, line });
    }

    Err(LocatorError::InvalidFormat(format!(
        "unrecognized search syntax after '::': {search}"
    )))
}

/// Parses `/`-separated outline path segments following `::*/`.
fn parse_outline_segments(file: PathBuf, path_str: &str) -> Result<OrgLocator, LocatorError> {
    let segments: Vec<String> = path_str
        .split('/')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(LocatorError::InvalidFormat(
            "empty outline path after ::*/".to_string(),
        ));
    }
    Ok(OrgLocator::OutlinePath {
        file,
        path: segments,
    })
}

/// Scans `.org` files under `search_paths` to resolve an `id:<uuid>` locator.
fn resolve_id_across_paths(
    id: &str,
    search_paths: &[PathBuf],
) -> Result<ResolvedEntry, LocatorError> {
    let files = collect_org_files(search_paths);
    let locator = OrgLocator::Id(id.to_string());

    for file_path in &files {
        let source =
            SourceFile::from_path(file_path).map_err(|e| LocatorError::IoError(e.to_string()))?;
        if !source.content.contains(id) {
            continue;
        }
        let doc = OrgDocument::from_source(&source);
        if let Ok(resolved) = resolve_in_document(&locator, &doc) {
            return Ok(resolved);
        }
    }

    Err(LocatorError::EntryNotFound(format!("id:{id}")))
}

/// Loads `file` from disk and resolves a file-scoped `locator` within it.
fn resolve_file_scoped(file: &Path, locator: &OrgLocator) -> Result<ResolvedEntry, LocatorError> {
    if !file.is_file() {
        return Err(LocatorError::FileNotFound(file.to_path_buf()));
    }
    let source = SourceFile::from_path(file).map_err(|e| LocatorError::IoError(e.to_string()))?;
    let doc = OrgDocument::from_source(&source);
    resolve_in_document(locator, &doc)
}

/// Builds a [`ResolvedEntry`] from a heading index in `doc`.
fn build_entry_resolution(doc: &OrgDocument, file: PathBuf, idx: usize) -> ResolvedEntry {
    let entry = &doc.entries[idx];
    ResolvedEntry {
        file,
        line: entry.heading_line,
        heading_text: entry.title.clone(),
        level: entry.level,
        entry_index: idx,
    }
}

/// Resolves an `id:<uuid>` against both top-of-file `:PROPERTIES:` and headings in `doc`.
fn resolve_id_in_doc(doc: &OrgDocument, id: &str) -> Result<ResolvedEntry, LocatorError> {
    if doc.has_file_id(id) {
        return Ok(ResolvedEntry {
            file: doc.file.clone(),
            line: doc.file_properties_line.unwrap_or(1),
            heading_text: doc.file_title_or_stem(),
            level: FILE_NODE_LEVEL,
            entry_index: FILE_NODE_ENTRY_INDEX,
        });
    }

    if let Some(idx) = doc.find_by_id(id) {
        return Ok(build_entry_resolution(doc, doc.file.clone(), idx));
    }

    Err(LocatorError::EntryNotFound(format!("id:{id}")))
}

/// Resolves a `:CUSTOM_ID:` property within `doc`.
fn resolve_custom_id_in_doc(
    doc: &OrgDocument,
    file: &Path,
    custom_id: &str,
) -> Result<ResolvedEntry, LocatorError> {
    match doc.find_by_custom_id(custom_id) {
        Some(idx) => Ok(build_entry_resolution(doc, file.to_path_buf(), idx)),
        None => Err(LocatorError::EntryNotFound(format!(
            "{}::#{custom_id}",
            file.display()
        ))),
    }
}

/// Resolves a case-insensitive `::*/...` outline path within `doc`.
fn resolve_outline_path_in_doc(
    doc: &OrgDocument,
    file: &Path,
    path: &[String],
) -> Result<ResolvedEntry, LocatorError> {
    let mut candidates: Vec<usize> = (0..doc.entries.len())
        .filter(|&i| doc.entries[i].parent.is_none())
        .collect();
    let mut current_match = None;

    for (depth, segment) in path.iter().enumerate() {
        let segment_lower = segment.to_lowercase();
        let matched = candidates
            .iter()
            .copied()
            .find(|&idx| doc.entries[idx].title.to_lowercase() == segment_lower);

        let Some(idx) = matched else {
            return Err(LocatorError::EntryNotFound(format!(
                "{}::*/{}",
                file.display(),
                path.join("/")
            )));
        };

        current_match = Some(idx);
        if depth + 1 < path.len() {
            candidates = doc.entries[idx].children.clone();
        }
    }

    match current_match {
        Some(idx) => Ok(build_entry_resolution(doc, file.to_path_buf(), idx)),
        None => Err(LocatorError::EntryNotFound(format!(
            "{}::*/{}",
            file.display(),
            path.join("/")
        ))),
    }
}

/// Resolves a 1-based line number to the nearest heading at or above `line` in `doc`.
fn resolve_line_ref_in_doc(
    doc: &OrgDocument,
    file: &Path,
    line: usize,
) -> Result<ResolvedEntry, LocatorError> {
    let best = doc
        .entries
        .iter()
        .enumerate()
        .take_while(|(_, entry)| entry.heading_line <= line)
        .map(|(idx, _)| idx)
        .last();

    match best {
        Some(idx) => Ok(build_entry_resolution(doc, file.to_path_buf(), idx)),
        None if line == 1
            || !doc.file_properties.is_empty()
            || doc.file_id().is_some()
            || doc.file_properties_line.is_some()
            || doc.entries.first().is_none_or(|e| line < e.heading_line) =>
        {
            Ok(ResolvedEntry {
                file: file.to_path_buf(),
                line: doc.file_properties_line.unwrap_or(1),
                heading_text: doc.file_title_or_stem(),
                level: FILE_NODE_LEVEL,
                entry_index: FILE_NODE_ENTRY_INDEX,
            })
        }
        None => Err(LocatorError::EntryNotFound(format!(
            "{}::{line}",
            file.display()
        ))),
    }
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Verifies parsing an `id:<uuid>` locator string.
    #[test]
    fn parse_id_locator() {
        let loc = OrgLocator::parse("id:550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(
            loc,
            OrgLocator::Id("550e8400-e29b-41d4-a716-446655440000".into())
        );
    }

    /// Verifies parsing a `<file>::#<custom-id>` locator string.
    #[test]
    fn parse_custom_id_locator() {
        let loc = OrgLocator::parse("notes.org::#project-x").unwrap();
        assert_eq!(
            loc,
            OrgLocator::CustomId {
                file: "notes.org".into(),
                custom_id: "project-x".into()
            }
        );
    }

    /// Verifies parsing a `<file>::*/<outline-path>` locator string.
    #[test]
    fn parse_outline_path_locator() {
        let loc = OrgLocator::parse("todo.org::*/Work/Meeting notes").unwrap();
        assert_eq!(
            loc,
            OrgLocator::OutlinePath {
                file: PathBuf::from("todo.org"),
                path: vec!["Work".to_string(), "Meeting notes".to_string()],
            }
        );
    }

    /// Verifies parsing a `<file>::<line>` locator string.
    #[test]
    fn parse_line_ref_locator() {
        let loc = OrgLocator::parse("inbox.org::42").unwrap();
        assert_eq!(
            loc,
            OrgLocator::LineRef {
                file: PathBuf::from("inbox.org"),
                line: 42,
            }
        );
    }

    /// Verifies rejection of malformed locator inputs.
    #[test]
    fn parse_invalid_locators() {
        assert!(OrgLocator::parse("").is_err());
        assert!(OrgLocator::parse("just-text").is_err());
        assert!(OrgLocator::parse("id:").is_err());
        assert!(OrgLocator::parse("file.org::#").is_err());
        assert!(OrgLocator::parse("file.org::*/").is_err());
        assert!(OrgLocator::parse("file.org::0").is_err());
        assert!(OrgLocator::parse("::search").is_err());
    }

    /// Verifies `Display` and `OrgLocator::parse` round-trip across all four locator variants.
    #[test]
    fn display_roundtrip() {
        let cases = vec![
            "id:abc-123",
            "notes.org::#my-section",
            "todo.org::*/Work/Meeting",
            "inbox.org::42",
        ];
        for case in cases {
            let parsed = OrgLocator::parse(case).unwrap();
            assert_eq!(parsed.to_string(), case);
        }
    }

    /// Verifies resolving a heading `:ID:` locator across a directory of `.org` files.
    #[test]
    fn resolve_id_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Heading\n:PROPERTIES:\n:ID: test-uuid\n:END:\n").unwrap();
        let res =
            resolve_locator(&OrgLocator::Id("test-uuid".into()), &[dir.path().into()]).unwrap();
        assert_eq!(res.heading_text, "Heading");
        assert_eq!(res.line, 1);
    }

    /// Verifies resolving a `:CUSTOM_ID:` locator inside a specific file.
    #[test]
    fn resolve_custom_id_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(
            &file,
            "* Section\n:PROPERTIES:\n:CUSTOM_ID: my-sect\n:END:\n",
        )
        .unwrap();
        let loc = OrgLocator::CustomId {
            file,
            custom_id: "my-sect".into(),
        };
        let res = resolve_locator(&loc, &[]).unwrap();
        assert_eq!(res.heading_text, "Section");
    }

    /// Verifies resolving a nested outline path locator.
    #[test]
    fn resolve_outline_path_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Work\n** Meeting notes\nBody\n").unwrap();
        let path = vec!["Work".to_string(), "Meeting notes".to_string()];
        let res = resolve_locator(&OrgLocator::OutlinePath { file, path }, &[]).unwrap();
        assert_eq!(res.heading_text, "Meeting notes");
        assert_eq!(res.level, 2);
    }

    /// Verifies case-insensitive matching of outline path segments.
    #[test]
    fn resolve_outline_path_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Work\n** MEETING NOTES\n").unwrap();
        let path = vec!["work".to_string(), "meeting notes".to_string()];
        let res = resolve_locator(&OrgLocator::OutlinePath { file, path }, &[]).unwrap();
        assert_eq!(res.heading_text, "MEETING NOTES");
    }

    /// Verifies resolving a line reference to its enclosing heading.
    #[test]
    fn resolve_line_ref_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* First\nBody\n* Second\nMore body\n").unwrap();
        let res = resolve_locator(&OrgLocator::LineRef { file, line: 4 }, &[]).unwrap();
        assert_eq!(res.heading_text, "Second");
    }

    /// Verifies returning `LocatorError::EntryNotFound` when an `id:` UUID does not exist.
    #[test]
    fn resolve_id_not_found() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Heading\n").unwrap();
        let loc = OrgLocator::Id("nonexistent".into());
        assert!(resolve_locator(&loc, &[dir.path().into()]).is_err());
    }

    /// Verifies `locator_for_entry` prefers `:ID:` over `:CUSTOM_ID:` and outline path.
    #[test]
    fn locator_prefers_id() {
        let source = SourceFile::new(
            "test.org",
            "* Heading\n:PROPERTIES:\n:ID: uuid-1\n:CUSTOM_ID: cid\n:END:\n".to_string(),
        );
        let doc = OrgDocument::from_source(&source);
        let loc = locator_for_entry(&doc, 0);
        assert_eq!(loc, OrgLocator::Id("uuid-1".to_string()));
    }

    /// Verifies `locator_for_entry` falls back to `:CUSTOM_ID:` when `:ID:` is absent.
    #[test]
    fn locator_falls_back_to_custom_id() {
        let source = SourceFile::new(
            "test.org",
            "* Heading\n:PROPERTIES:\n:CUSTOM_ID: my-heading\n:END:\n".to_string(),
        );
        let doc = OrgDocument::from_source(&source);
        let loc = locator_for_entry(&doc, 0);
        assert_eq!(
            loc,
            OrgLocator::CustomId {
                file: PathBuf::from("test.org"),
                custom_id: "my-heading".to_string(),
            }
        );
    }

    /// Verifies `locator_for_entry` falls back to `OutlinePath` when neither ID property exists.
    #[test]
    fn locator_falls_back_to_outline_path() {
        let source = SourceFile::new("test.org", "* Work\n** Meeting\n".to_string());
        let doc = OrgDocument::from_source(&source);
        let loc = locator_for_entry(&doc, 1);
        assert_eq!(
            loc,
            OrgLocator::OutlinePath {
                file: PathBuf::from("test.org"),
                path: vec!["Work".to_string(), "Meeting".to_string()],
            }
        );
    }

    /// Verifies that `resolve_locator` resolves a top-of-file org-roam v2 `:ID:` drawer.
    #[test]
    fn resolve_id_locator_matches_file_level_org_roam_node() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("zettelkasten.org");
        fs::write(
            &file,
            ":PROPERTIES:\n:ID: roam-file-uuid\n:END:\n#+TITLE: My Roam Note\nBody text.\n",
        )
        .unwrap();

        let resolved = resolve_locator(
            &OrgLocator::Id("roam-file-uuid".to_string()),
            &[dir.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(resolved.file, file);
        assert_eq!(resolved.level, 0);
        assert!(resolved.is_file_node());
        assert_eq!(resolved.line, 1);
        assert_eq!(resolved.heading_text, "My Roam Note");
        assert_eq!(resolved.entry_index, usize::MAX);

        let source = SourceFile::from_path(&file).unwrap();
        let doc = OrgDocument::from_source(&source);
        assert!(doc.entries.get(resolved.entry_index).is_none());
    }

    /// Verifies file-node line resolution after leading `# comment` lines and stem fallback without `#+TITLE:`.
    #[test]
    fn resolve_id_locator_file_node_after_comments_uses_drawer_line_and_stem_fallback() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("my-stem-note.org");
        fs::write(
            &file,
            "# -*- mode: org; -*-\n:PROPERTIES:\n:ID: stem-uuid\n:END:\nBody without title.\n",
        )
        .unwrap();

        let resolved = resolve_locator(
            &OrgLocator::Id("stem-uuid".to_string()),
            &[dir.path().to_path_buf()],
        )
        .unwrap();
        assert!(resolved.is_file_node());
        assert_eq!(resolved.level, 0);
        assert_eq!(resolved.line, 2);
        assert_eq!(resolved.heading_text, "my-stem-note");
        assert_eq!(resolved.entry_index, FILE_NODE_ENTRY_INDEX);
    }

    /// Verifies `resolve_in_document` resolves both level-0 file `:ID:`s and heading `:ID:`s.
    #[test]
    fn resolve_in_document_resolves_both_file_and_heading_ids() {
        let source = SourceFile::new(
            "hybrid.org",
            ":PROPERTIES:\n:ID: file-uuid\n:END:\n#+TITLE: Hybrid Note\n* Subtopic\n:PROPERTIES:\n:ID: heading-uuid\n:END:\n"
                .to_string(),
        );
        let doc = OrgDocument::from_source(&source);

        let file_res = resolve_in_document(&OrgLocator::Id("file-uuid".to_string()), &doc).unwrap();
        assert!(file_res.is_file_node());
        assert_eq!(file_res.level, 0);
        assert_eq!(file_res.line, 1);
        assert_eq!(file_res.heading_text, "Hybrid Note");
        assert_eq!(file_res.entry_index, usize::MAX);

        let heading_res =
            resolve_in_document(&OrgLocator::Id("heading-uuid".to_string()), &doc).unwrap();
        assert!(!heading_res.is_file_node());
        assert_eq!(heading_res.level, 1);
        assert_eq!(heading_res.line, 5);
        assert_eq!(heading_res.heading_text, "Subtopic");
        assert_eq!(heading_res.entry_index, 0);
    }
}

// endregion
