// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Org Item Locator — a unique identifier for headings in org files.
//!
//! An [`OrgLocator`] uniquely identifies a heading entry across a tree of org
//! files. Four variants exist, ordered by stability:
//!
//! | Pattern | Example | Stability |
//! |---|---|---|
//! | `id:<uuid>` | `id:550e8400-e29b-...` | Survives moves |
//! | `<file>::#<custom-id>` | `notes.org::#project-x` | File-scoped |
//! | `<file>::*/<path>` | `todo.org::*/Work/Meeting` | Fragile on rename |
//! | `<file>::<line>` | `inbox.org::42` | Fragile on edit |
//!
//! The `::` separator mirrors org-mode's own link syntax (`file::search`),
//! making the format familiar to org-mode users.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::files::collect_org_files;
use crate::source::SourceFile;

/// A unique identifier for an org heading entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgLocator {
    /// UUID-based: `id:550e8400-e29b-41d4-a716-446655440000`.
    /// Globally unique — found via the `:ID:` property.
    /// Survives file moves and heading renames.
    Id(String),

    /// File-scoped custom ID: `path/to/file.org::#custom-id`.
    /// Unique within a file — found via the `:CUSTOM_ID:` property.
    CustomId {
        /// Path to the org file.
        file: PathBuf,
        /// CUSTOM_ID value (without `#` prefix).
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

/// A successfully resolved locator pointing to a concrete file and entry.
#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    /// Path to the org file.
    pub file: PathBuf,
    /// 1-based line number of the heading.
    pub line: usize,
    /// Heading title text.
    pub heading_text: String,
    /// Heading level (1 = top-level).
    pub level: usize,
    /// Index into the [`OrgDocument::entries`] vec.
    pub entry_index: usize,
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

        // id:<uuid>
        if let Some(uuid) = input.strip_prefix("id:") {
            let uuid = uuid.trim();
            if uuid.is_empty() {
                return Err(LocatorError::InvalidFormat(
                    "id: locator has empty UUID".to_string(),
                ));
            }
            return Ok(Self::Id(uuid.to_string()));
        }

        // file::search variants
        if let Some(sep_pos) = input.find("::") {
            let file = PathBuf::from(&input[..sep_pos]);
            let search = &input[sep_pos + 2..];

            if file.as_os_str().is_empty() {
                return Err(LocatorError::InvalidFormat(
                    "empty file path before ::".to_string(),
                ));
            }

            // ::#custom-id
            if let Some(custom_id) = search.strip_prefix('#') {
                let custom_id = custom_id.trim();
                if custom_id.is_empty() {
                    return Err(LocatorError::InvalidFormat(
                        "empty CUSTOM_ID after ::#".to_string(),
                    ));
                }
                return Ok(Self::CustomId {
                    file,
                    custom_id: custom_id.to_string(),
                });
            }

            // ::*/path/segments
            if let Some(path_str) = search.strip_prefix("*/") {
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
                return Ok(Self::OutlinePath {
                    file,
                    path: segments,
                });
            }

            // ::<number>
            if let Ok(line) = search.trim().parse::<usize>() {
                if line == 0 {
                    return Err(LocatorError::InvalidFormat(
                        "line number must be >= 1".to_string(),
                    ));
                }
                return Ok(Self::LineRef { file, line });
            }

            return Err(LocatorError::InvalidFormat(format!(
                "unrecognized search syntax after '::': {search}"
            )));
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_canonical_string())
    }
}

/// Resolve a locator to a concrete file + entry.
///
/// For [`OrgLocator::Id`] locators, scans all `.org` files in `search_paths`
/// for a matching `:ID:` property. For file-scoped variants, searches within
/// the specified file.
pub fn resolve_locator(
    locator: &OrgLocator,
    search_paths: &[PathBuf],
) -> Result<ResolvedEntry, LocatorError> {
    match locator {
        OrgLocator::Id(id) => resolve_id(id, search_paths),
        OrgLocator::CustomId { file, custom_id } => resolve_custom_id(file, custom_id),
        OrgLocator::OutlinePath { file, path } => resolve_outline_path(file, path),
        OrgLocator::LineRef { file, line } => resolve_line_ref(file, *line),
    }
}

/// Generate the most specific locator for an entry.
/// Prefers ID > CUSTOM_ID > OutlinePath.
pub fn locator_for_entry(doc: &OrgDocument, entry_idx: usize) -> OrgLocator {
    let entry = &doc.entries[entry_idx];

    if let Some(id) = entry.properties.get("ID") {
        return OrgLocator::Id(id.clone());
    }

    if let Some(custom_id) = entry.properties.get("CUSTOM_ID") {
        return OrgLocator::CustomId {
            file: doc.file.clone(),
            custom_id: custom_id.clone(),
        };
    }

    let path = doc
        .outline_path(entry_idx)
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    OrgLocator::OutlinePath {
        file: doc.file.clone(),
        path,
    }
}

// --- Resolution helpers ---

fn resolve_id(id: &str, search_paths: &[PathBuf]) -> Result<ResolvedEntry, LocatorError> {
    let files = collect_org_files(search_paths);

    for file_path in &files {
        let source = SourceFile::from_path(file_path)
            .map_err(|e| LocatorError::IoError(e.to_string()))?;
        let doc = OrgDocument::from_source(&source);
        if let Some(idx) = doc.find_by_id(id) {
            let entry = &doc.entries[idx];
            return Ok(ResolvedEntry {
                file: file_path.clone(),
                line: entry.heading_line,
                heading_text: entry.title.clone(),
                level: entry.level,
                entry_index: idx,
            });
        }
    }

    Err(LocatorError::EntryNotFound(format!("id:{id}")))
}

fn resolve_custom_id(
    file: &Path,
    custom_id: &str,
) -> Result<ResolvedEntry, LocatorError> {
    if !file.is_file() {
        return Err(LocatorError::FileNotFound(file.to_path_buf()));
    }
    let source =
        SourceFile::from_path(file).map_err(|e| LocatorError::IoError(e.to_string()))?;
    let doc = OrgDocument::from_source(&source);

    match doc.find_by_custom_id(custom_id) {
        Some(idx) => {
            let entry = &doc.entries[idx];
            Ok(ResolvedEntry {
                file: file.to_path_buf(),
                line: entry.heading_line,
                heading_text: entry.title.clone(),
                level: entry.level,
                entry_index: idx,
            })
        }
        None => Err(LocatorError::EntryNotFound(format!(
            "{}::#{custom_id}",
            file.display()
        ))),
    }
}

fn resolve_outline_path(
    file: &Path,
    path: &[String],
) -> Result<ResolvedEntry, LocatorError> {
    if !file.is_file() {
        return Err(LocatorError::FileNotFound(file.to_path_buf()));
    }
    let source =
        SourceFile::from_path(file).map_err(|e| LocatorError::IoError(e.to_string()))?;
    let doc = OrgDocument::from_source(&source);

    // Walk the tree matching path segments.
    let mut candidates: Vec<usize> = (0..doc.entries.len())
        .filter(|&i| doc.entries[i].parent.is_none())
        .collect();

    let mut current_match = None;

    for (depth, segment) in path.iter().enumerate() {
        let segment_lower = segment.to_lowercase();
        let mut found = false;
        for &idx in &candidates {
            if doc.entries[idx].title.to_lowercase() == segment_lower {
                current_match = Some(idx);
                if depth + 1 < path.len() {
                    candidates = doc.entries[idx].children.clone();
                }
                found = true;
                break;
            }
        }
        if !found {
            return Err(LocatorError::EntryNotFound(format!(
                "{}::*/{}",
                file.display(),
                path.join("/")
            )));
        }
    }

    match current_match {
        Some(idx) => {
            let entry = &doc.entries[idx];
            Ok(ResolvedEntry {
                file: file.to_path_buf(),
                line: entry.heading_line,
                heading_text: entry.title.clone(),
                level: entry.level,
                entry_index: idx,
            })
        }
        None => Err(LocatorError::EntryNotFound(format!(
            "{}::*/{}",
            file.display(),
            path.join("/")
        ))),
    }
}

fn resolve_line_ref(file: &Path, line: usize) -> Result<ResolvedEntry, LocatorError> {
    if !file.is_file() {
        return Err(LocatorError::FileNotFound(file.to_path_buf()));
    }
    let source =
        SourceFile::from_path(file).map_err(|e| LocatorError::IoError(e.to_string()))?;
    let doc = OrgDocument::from_source(&source);

    // Find the heading at or before the given line.
    let mut best: Option<usize> = None;
    for (idx, entry) in doc.entries.iter().enumerate() {
        if entry.heading_line <= line {
            best = Some(idx);
        } else {
            break;
        }
    }

    match best {
        Some(idx) => {
            let entry = &doc.entries[idx];
            Ok(ResolvedEntry {
                file: file.to_path_buf(),
                line: entry.heading_line,
                heading_text: entry.title.clone(),
                level: entry.level,
                entry_index: idx,
            })
        }
        None => Err(LocatorError::EntryNotFound(format!(
            "{}::{line}",
            file.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // --- Parse tests ---

    #[test]
    fn parse_id_locator() {
        let loc = OrgLocator::parse("id:550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(
            loc,
            OrgLocator::Id("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
    }

    #[test]
    fn parse_custom_id_locator() {
        let loc = OrgLocator::parse("notes.org::#project-x").unwrap();
        assert_eq!(
            loc,
            OrgLocator::CustomId {
                file: PathBuf::from("notes.org"),
                custom_id: "project-x".to_string(),
            }
        );
    }

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

    // --- Display / roundtrip tests ---

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

    // --- Resolution tests ---

    #[test]
    fn resolve_id_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(
            &file,
            "* Heading\n:PROPERTIES:\n:ID: test-uuid\n:END:\n",
        )
        .unwrap();

        let result =
            resolve_locator(&OrgLocator::Id("test-uuid".to_string()), &[dir.path().to_path_buf()])
                .unwrap();
        assert_eq!(result.heading_text, "Heading");
        assert_eq!(result.line, 1);
    }

    #[test]
    fn resolve_custom_id_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(
            &file,
            "* Section\n:PROPERTIES:\n:CUSTOM_ID: my-sect\n:END:\n",
        )
        .unwrap();

        let result = resolve_locator(
            &OrgLocator::CustomId {
                file: file.clone(),
                custom_id: "my-sect".to_string(),
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.heading_text, "Section");
    }

    #[test]
    fn resolve_outline_path_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Work\n** Meeting notes\nBody\n").unwrap();

        let result = resolve_locator(
            &OrgLocator::OutlinePath {
                file: file.clone(),
                path: vec!["Work".to_string(), "Meeting notes".to_string()],
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.heading_text, "Meeting notes");
        assert_eq!(result.level, 2);
    }

    #[test]
    fn resolve_outline_path_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Work\n** MEETING NOTES\n").unwrap();

        let result = resolve_locator(
            &OrgLocator::OutlinePath {
                file: file.clone(),
                path: vec!["work".to_string(), "meeting notes".to_string()],
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.heading_text, "MEETING NOTES");
    }

    #[test]
    fn resolve_line_ref_locator() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* First\nBody\n* Second\nMore body\n").unwrap();

        // Line 4 is in the "Second" heading's body.
        let result = resolve_locator(
            &OrgLocator::LineRef {
                file: file.clone(),
                line: 4,
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.heading_text, "Second");
    }

    #[test]
    fn resolve_id_not_found() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.org");
        fs::write(&file, "* Heading\n").unwrap();

        let result =
            resolve_locator(&OrgLocator::Id("nonexistent".to_string()), &[dir.path().to_path_buf()]);
        assert!(result.is_err());
    }

    // --- locator_for_entry tests ---

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

    #[test]
    fn locator_falls_back_to_outline_path() {
        let source = SourceFile::new(
            "test.org",
            "* Work\n** Meeting\n".to_string(),
        );
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
}
