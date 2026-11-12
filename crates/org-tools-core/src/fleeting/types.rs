// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Data structures and enums for Luhmann fleeting note processing.

use std::fmt;
use std::path::Path;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

// region Fleeting Status & Filter

/// Lifecycle status of a Luhmann fleeting note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FleetingStatus {
    /// Newly captured thought awaiting processing or refinement.
    #[default]
    Unrefined,
    /// Successfully transformed into a permanent Zettelkasten concept note.
    Reformed,
    /// Converted into an actionable GTD inbox task.
    ConvertedToTask,
    /// Discarded as irrelevant, noise, or duplicate.
    Discarded,
}

impl FleetingStatus {
    /// Returns the canonical kebab-case string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unrefined => "unrefined",
            Self::Reformed => "reformed",
            Self::ConvertedToTask => "converted-to-task",
            Self::Discarded => "discarded",
        }
    }

    /// Parses a status string, case-insensitively handling aliases.
    pub fn from_status_str(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("unrefined") || trimmed.eq_ignore_ascii_case("pending") {
            Some(Self::Unrefined)
        } else if trimmed.eq_ignore_ascii_case("reformed")
            || trimmed.eq_ignore_ascii_case("refined")
            || trimmed.eq_ignore_ascii_case("zettel")
        {
            Some(Self::Reformed)
        } else if trimmed.eq_ignore_ascii_case("converted-to-task")
            || trimmed.eq_ignore_ascii_case("task")
            || trimmed.eq_ignore_ascii_case("todo")
        {
            Some(Self::ConvertedToTask)
        } else if trimmed.eq_ignore_ascii_case("discarded")
            || trimmed.eq_ignore_ascii_case("cancelled")
            || trimmed.eq_ignore_ascii_case("deleted")
        {
            Some(Self::Discarded)
        } else {
            None
        }
    }
}

impl fmt::Display for FleetingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Status filter option for `org fleeting list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FleetingStatusFilter {
    /// Filter for unrefined notes only.
    #[default]
    Unrefined,
    /// Filter for reformed permanent notes only.
    Reformed,
    /// Filter for notes converted to GTD tasks only.
    ConvertedToTask,
    /// Filter for discarded notes only.
    Discarded,
    /// Match fleeting notes in any lifecycle state.
    All,
}

impl FleetingStatusFilter {
    /// Checks whether `status` matches this filter criterion.
    pub fn matches(&self, status: FleetingStatus) -> bool {
        match self {
            Self::All => true,
            Self::Unrefined => status == FleetingStatus::Unrefined,
            Self::Reformed => status == FleetingStatus::Reformed,
            Self::ConvertedToTask => status == FleetingStatus::ConvertedToTask,
            Self::Discarded => status == FleetingStatus::Discarded,
        }
    }
}

// endregion

// region Fleeting Item Model & Output Envelope

/// Granularity of the fleeting note (standalone file vs heading stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FleetingKind {
    /// Dedicated `.org` file note.
    #[default]
    File,
    /// Heading-level stub inside another Org file.
    Heading,
}

impl FleetingKind {
    /// Returns the lowercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Heading => "heading",
        }
    }
}

/// A discovered Luhmann fleeting note item ready for display or transformation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetingItem {
    /// Unique identifier from `:ID:` if present.
    pub id: Option<String>,
    /// Note title or heading text.
    pub title: String,
    /// Lifecycle refinement status.
    pub status: FleetingStatus,
    /// Note kind (`file` or `heading`).
    pub kind: FleetingKind,
    /// Filesystem path to the containing Org source file.
    pub file: String,
    /// One-based line number in the source file.
    pub line: usize,
    /// Canonical locator (`id:<uuid>` or `<file>::*<title>` / `<file>`).
    pub locator: String,
    /// Formatted capture timestamp string (`[YYYY-MM-DD Day HH:MM]`).
    pub captured_at: Option<String>,
    /// Optional attached medium node link or citation string.
    pub attached_medium: Option<String>,
    /// Optional source location (page, chapter, or section).
    pub medium_location: Option<String>,
    /// Optional attached parent Zettel link or node query.
    pub attached_zettel: Option<String>,
    /// Optional path to source code or document that sparked the thought.
    pub source_file: Option<String>,
    /// Optional link to permanent Zettel if reformed.
    pub permanent_note: Option<String>,
    /// Optional link to GTD task if converted.
    pub task_note: Option<String>,
    /// Raw unrefined thoughts extracted from the body.
    #[serde(skip)]
    pub raw_thoughts: String,
}

/// JSON envelope emitted by `org fleeting list --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetingListEnvelope {
    /// Total number of matching fleeting notes found.
    pub count: usize,
    /// List of matched fleeting notes.
    pub items: Vec<FleetingItem>,
}

/// Input parameters for creating a new fleeting note via `org fleeting capture`.
pub struct FleetingCaptureSpec<'a> {
    /// Note title.
    pub title: &'a str,
    /// Body text supplied via `--body` or `--stdin`.
    pub body: Option<&'a str>,
    /// Optional source medium node query or citation.
    pub medium: Option<&'a str>,
    /// Optional medium location (page, chapter, etc.).
    pub location: Option<&'a str>,
    /// Optional attached parent Zettel node query.
    pub zettel: Option<&'a str>,
    /// Optional source file path.
    pub source_file: Option<&'a str>,
    /// Vault name.
    pub vault_name: &'a str,
    /// Vault root path.
    pub vault_path: &'a Path,
    /// Subdirectory for fleeting notes.
    pub fleeting_subdir: &'a str,
    /// Pre-minted unique identifier.
    pub note_id: &'a str,
    /// Current local timestamp tuple `(Y, M, D, h, m, s)`.
    pub dt: (i32, u32, u32, u32, u32, u32),
    /// Whether cache lookups are bypassed.
    pub no_cache: bool,
}

/// Parameters for reforming a fleeting note into a permanent Zettel.
pub struct PermanentZettelSpec<'a> {
    /// Source fleeting note.
    pub item: &'a FleetingItem,
    /// Optional Zettel title override.
    pub title_override: Option<&'a str>,
    /// Optional filename slug override.
    pub slug_override: Option<&'a str>,
    /// Optional parent Zettel node query.
    pub parent_override: Option<&'a str>,
    /// Related node queries linked under `* Related Concepts & Links`.
    pub links: &'a [String],
    /// Optional body text override.
    pub body_override: Option<&'a str>,
    /// Vault name.
    pub vault_name: &'a str,
    /// Vault root path.
    pub vault_path: &'a Path,
    /// Whether cache lookups are bypassed.
    pub no_cache: bool,
    /// Minted permanent Zettel identifier.
    pub zettel_id: &'a str,
}

/// Parameters for converting a fleeting note into a GTD inbox task.
pub struct FleetingTaskSpec<'a> {
    /// Source fleeting note.
    pub item: &'a FleetingItem,
    /// Optional task title override.
    pub task_title: Option<&'a str>,
    /// Optional GTD context tag (`@context`).
    pub context: Option<&'a str>,
    /// Optional effort estimate (`H:MM`).
    pub effort: Option<&'a str>,
    /// Optional `SCHEDULED:` date.
    pub scheduled: Option<&'a str>,
    /// Optional `DEADLINE:` date.
    pub deadline: Option<&'a str>,
    /// Existing inbox Org text.
    pub inbox_content: &'a str,
    /// Target inbox file path.
    pub inbox_path: &'a Path,
    /// Minted GTD task identifier.
    pub task_id: &'a str,
    /// Vault name.
    pub vault_name: &'a str,
    /// Vault root path.
    pub vault_dir: &'a Path,
    /// Whether cache lookups are bypassed.
    pub no_cache: bool,
}

// endregion
