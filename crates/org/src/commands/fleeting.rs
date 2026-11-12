// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org fleeting` subcommand router: Luhmann fleeting note queue and transformation workflows.

#[path = "fleeting/capture.rs"]
mod capture;
#[path = "fleeting/list.rs"]
mod list;
#[path = "fleeting/task.rs"]
mod task;
#[path = "fleeting/zettel.rs"]
mod zettel;

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueHint};

use org_tools_core::fleeting::FleetingStatusFilter;

use self::capture::run_capture;
use self::list::{run_list, FleetingListFormat};
use self::task::{run_discard, run_process, run_to_task};
use self::zettel::run_to_zettel;
use super::MutationOutputFormat;

// region CLI Subcommands & Arguments

/// Subcommands under `org fleeting`.
#[derive(Subcommand, Debug, Clone)]
pub(crate) enum FleetingCommand {
    /// Capture a new Luhmann fleeting thought note.
    Capture(Box<FleetingCaptureArgs>),
    /// List pending or refined fleeting notes across vaults.
    List(Box<FleetingListArgs>),
    /// Reform a fleeting note into a permanent Zettelkasten concept note.
    #[command(visible_alias = "reform")]
    ToZettel(Box<FleetingToZettelArgs>),
    /// Convert a fleeting note into an actionable GTD inbox task.
    ToTask(Box<FleetingToTaskArgs>),
    /// Discard or remove a fleeting note.
    Discard(Box<FleetingDiscardArgs>),
    /// Interactively review and process unrefined fleeting notes.
    Process(Box<FleetingProcessArgs>),
}

/// CLI arguments for `org fleeting capture`.
#[derive(Args, Debug, Clone)]
pub(crate) struct FleetingCaptureArgs {
    /// Note title (defaults to "Untitled" when omitted).
    pub(crate) title: Option<String>,
    /// Target roam vault name or directory.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Inline fleeting thought body text.
    #[arg(long)]
    pub(crate) body: Option<String>,
    /// Read body text from standard input.
    #[arg(long)]
    pub(crate) stdin: bool,
    /// Attached source medium node query or citation (`:ATTACHED_MEDIUM:`).
    #[arg(long)]
    pub(crate) medium: Option<String>,
    /// Source medium location such as page or chapter (`:MEDIUM_LOCATION:`).
    #[arg(long)]
    pub(crate) location: Option<String>,
    /// Attached parent Zettel node query (`:ATTACHED_ZETTEL:`).
    #[arg(long)]
    pub(crate) zettel: Option<String>,
    /// Path to file or source code that sparked the thought (`:SOURCE_FILE:`).
    #[arg(long)]
    pub(crate) source_file: Option<String>,
    /// Force opening `$VISUAL` / `$EDITOR` before saving.
    #[arg(long)]
    pub(crate) edit: bool,
    /// Skip opening `$VISUAL` / `$EDITOR`.
    #[arg(long)]
    pub(crate) no_edit: bool,
    /// Preview capture without writing files to disk.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org fleeting list`.
#[derive(Args, Debug, Clone)]
pub(crate) struct FleetingListArgs {
    /// Optional search paths (defaults to configured roam vaults).
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,
    /// Target roam vault selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Filter by lifecycle status.
    #[arg(long, value_enum, default_value = "unrefined")]
    pub(crate) status: FleetingStatusFilter,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Output format (`human`, `json`, or `locator`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: FleetingListFormat,
}

/// CLI arguments for `org fleeting to-zettel` (alias `reform`).
#[derive(Args, Debug, Clone)]
pub(crate) struct FleetingToZettelArgs {
    /// Target fleeting note locator, UUID, path, or title.
    pub(crate) target: String,
    /// Permanent Zettel title override.
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Filename slug override.
    #[arg(long)]
    pub(crate) slug: Option<String>,
    /// Target roam vault selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Parent Zettel query (`:PARENT_ZETTEL:` and Folgezettel link).
    #[arg(long)]
    pub(crate) parent: Option<String>,
    /// Repeatable related concept queries linked in `* Related Concepts & Links`.
    #[arg(long = "link")]
    pub(crate) links: Vec<String>,
    /// Core concept body text override.
    #[arg(long)]
    pub(crate) body: Option<String>,
    /// Force opening `$VISUAL` / `$EDITOR` to refine permanent note.
    #[arg(long)]
    pub(crate) edit: bool,
    /// Skip opening `$VISUAL` / `$EDITOR`.
    #[arg(long)]
    pub(crate) no_edit: bool,
    /// Preview transformation without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org fleeting to-task`.
#[derive(Args, Debug, Clone)]
pub(crate) struct FleetingToTaskArgs {
    /// Target fleeting note locator, UUID, path, or title.
    pub(crate) target: String,
    /// Task title override.
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Destination inbox path or locator.
    #[arg(long = "to")]
    pub(crate) to: Option<String>,
    /// Target roam vault selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// GTD `@context` tag or property.
    #[arg(long, short = 'c')]
    pub(crate) context: Option<String>,
    /// Estimated effort duration.
    #[arg(long)]
    pub(crate) effort: Option<String>,
    /// Optional `SCHEDULED:` date.
    #[arg(long)]
    pub(crate) scheduled: Option<String>,
    /// Optional `DEADLINE:` date.
    #[arg(long)]
    pub(crate) deadline: Option<String>,
    /// Preview task conversion without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org fleeting discard`.
#[derive(Args, Debug, Clone)]
pub(crate) struct FleetingDiscardArgs {
    /// Target fleeting note locator, UUID, path, or title.
    pub(crate) target: String,
    /// Target roam vault selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Permanently remove file from disk instead of stamping discarded.
    #[arg(long)]
    pub(crate) delete: bool,
    /// Preview discard without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org fleeting process`.
#[derive(Args, Debug, Clone)]
pub(crate) struct FleetingProcessArgs {
    /// Target roam vault selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
}

// endregion

// region Subcommand Dispatch

/// Dispatches an `org fleeting` subcommand.
pub(crate) fn run_fleeting(command: FleetingCommand, no_cache: bool) -> i32 {
    match command {
        FleetingCommand::Capture(args) => run_capture(*args, no_cache),
        FleetingCommand::List(args) => run_list(*args),
        FleetingCommand::ToZettel(args) => run_to_zettel(*args, no_cache),
        FleetingCommand::ToTask(args) => run_to_task(*args, no_cache),
        FleetingCommand::Discard(args) => run_discard(*args, no_cache),
        FleetingCommand::Process(args) => run_process(*args, no_cache),
    }
}

// endregion
