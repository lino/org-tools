// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org roam gtd` (`inbox`, `project`) and `org roam sync-agenda-tags` subcommand handlers.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, Subcommand, ValueHint};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{
    append_daily_capture_log, current_capture_datetime, mint_capture_id, resolve_target_vault,
    slugify_title,
};
use org_tools_core::config::Config;
use org_tools_core::files::{collect_org_files, write_file_atomic};
use org_tools_core::gtd_roam::{
    build_gtd_inbox_entry_buffer, build_gtd_project_file_buffer, sync_roam_agenda_filetag,
    GtdInboxEntrySpec,
};

use super::roam_capture::{resolve_editor_command, run_interactive_editor};
use super::update::invalidate_warm_workspace_caches;
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI arguments for `org roam sync-agenda-tags`.
#[derive(Args, Debug, Clone)]
pub(crate) struct SyncAgendaTagsArgs {
    /// Optional files or directories to scan (defaults to configured `org-roam` vaults).
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,
    /// Repeatable named `[roam.directories]` vault or directory path selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Vec<String>,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Preview filetag changes without writing files on disk.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// Subcommands under `org roam gtd`.
#[derive(Subcommand, Debug, Clone)]
pub(crate) enum RoamGtdCommand {
    /// Append a new `* TODO` entry into `<roam_dir>/<gtd_subdir>/inbox.org`.
    Inbox(Box<RoamGtdInboxArgs>),
    /// Create a new GTD project node at `<roam_dir>/<gtd_subdir>/projects/<slug>.org`.
    Project(Box<RoamGtdProjectArgs>),
}

/// CLI arguments for `org roam gtd inbox`.
#[derive(Args, Debug, Clone)]
pub(crate) struct RoamGtdInboxArgs {
    /// Title of the GTD inbox task.
    pub(crate) title: String,
    /// Named `[roam.directories]` vault or path override.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Optional workspace directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Optional owning project name, alias, or `:ID:`.
    #[arg(long, short = 'p')]
    pub(crate) project: Option<String>,
    /// Optional task body prose.
    #[arg(long)]
    pub(crate) body: Option<String>,
    /// GTD `@context` tag or `:CONTEXT:` value (e.g. `computer` or `@computer`).
    #[arg(long, short = 'c')]
    pub(crate) context: Option<String>,
    /// Estimated effort duration (`H:MM` or minutes).
    #[arg(long)]
    pub(crate) effort: Option<String>,
    /// Energy level (`low`, `medium`, `high`).
    #[arg(long, short = 'e')]
    pub(crate) energy: Option<String>,
    /// Workmode (`deep`, `shallow`, `any`).
    #[arg(long, short = 'w')]
    pub(crate) workmode: Option<String>,
    /// Optional `SCHEDULED:` date (`YYYY-MM-DD` or `<...>`).
    #[arg(long)]
    pub(crate) scheduled: Option<String>,
    /// Optional `DEADLINE:` date (`YYYY-MM-DD` or `<...>`).
    #[arg(long)]
    pub(crate) deadline: Option<String>,
    /// Open `$VISUAL` / `$EDITOR` on the inbox entry before saving.
    #[arg(long)]
    pub(crate) edit: bool,
    /// Preview the inbox entry without modifying files on disk.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org roam gtd project`.
#[derive(Args, Debug, Clone)]
pub(crate) struct RoamGtdProjectArgs {
    /// Title of the new GTD project.
    pub(crate) title: String,
    /// Named `[roam.directories]` vault or path override.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Optional workspace directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Explicit filename slug override.
    #[arg(long)]
    pub(crate) slug: Option<String>,
    /// Optional `:CATEGORY:` property value.
    #[arg(long)]
    pub(crate) category: Option<String>,
    /// Optional first `** NEXT` task created under `* Tasks`.
    #[arg(long)]
    pub(crate) first_task: Option<String>,
    /// Open `$VISUAL` / `$EDITOR` on the project buffer before saving.
    #[arg(long)]
    pub(crate) edit: bool,
    /// Preview the project buffer without modifying files on disk.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Sync Agenda Tags Execution

/// Resolves `(scan_root, gtd_subdir, roam_dir_opt)` tuples for `sync-agenda-tags`.
fn resolve_sync_agenda_roots(
    args: &SyncAgendaTagsArgs,
) -> Result<Vec<(PathBuf, String, Option<PathBuf>)>, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let base = args.workspace.as_deref().unwrap_or(&cwd);
    let cfg = Config::load_for_dir(base).map_err(|e| e.to_string())?;
    let to_vault_root = |p: PathBuf| {
        let v_cfg = Config::load_for_dir(&p).unwrap_or_else(|_| cfg.clone());
        (p.clone(), v_cfg.roam.gtd_subdir, Some(p))
    };
    if !args.roam_dir.is_empty() {
        return args
            .roam_dir
            .iter()
            .map(|sel| {
                let (_, path) = cfg
                    .resolve_roam_dir(sel)
                    .ok_or_else(|| format!("unknown roam directory: {sel}"))?;
                if !path.exists() {
                    return Err(format!("roam directory not found: {}", path.display()));
                }
                Ok(to_vault_root(path))
            })
            .collect();
    }
    if !args.paths.is_empty() {
        return args
            .paths
            .iter()
            .map(|p| {
                if !p.exists() {
                    return Err(format!("path not found: {}", p.display()));
                }
                Ok((p.clone(), cfg.roam.gtd_subdir.clone(), None))
            })
            .collect();
    }
    let configured = cfg.roam_directories();
    if !configured.is_empty() {
        return Ok(configured
            .into_iter()
            .filter(|(_, p)| p.exists())
            .map(|(_, p)| to_vault_root(p))
            .collect());
    }
    Ok(vec![(base.to_path_buf(), cfg.roam.gtd_subdir, None)])
}

/// Executes `org roam sync-agenda-tags`.
pub(crate) fn run_sync_agenda_tags(args: SyncAgendaTagsArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let Ok(roots) = resolve_sync_agenda_roots(&args).map_err(|m| eprintln!("org: {m}")) else {
        return EXIT_ERROR;
    };
    let mut mutation = MutationResult::new("sync-agenda-tags", args.dry_run);
    let mut scanned = 0usize;
    let mut modified_paths = Vec::new();

    for (root, gtd_subdir, roam_dir) in &roots {
        for file in collect_org_files(std::slice::from_ref(root)) {
            scanned += 1;
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Some(updated) =
                sync_roam_agenda_filetag(&file, &content, gtd_subdir, roam_dir.as_deref())
            else {
                continue;
            };
            let added = updated.contains(":agenda:") && !content.contains(":agenda:");
            if !args.dry_run {
                if let Err(e) = write_file_atomic(&file, &updated) {
                    let msg = format!("error writing {}: {e}", file.display());
                    eprintln!("org: {msg}");
                    mutation.errors.push(msg);
                    continue;
                }
            }
            modified_paths.push(file.clone());
            mutation.entries_affected += 1;
            let act = if added {
                "added-agenda-tag"
            } else {
                "removed-agenda-tag"
            };
            mutation.details.push(MutationDetail {
                file: file.display().to_string(),
                line: 1,
                action: act.to_string(),
                detail: Some(":agenda:".to_string()),
            });
        }
    }
    if !args.dry_run && !no_cache && !modified_paths.is_empty() {
        invalidate_warm_workspace_caches(args.workspace.as_deref(), &modified_paths);
    }
    mutation.files_modified = modified_paths.len();
    mutation.finish(started, scanned);
    output_mutation(&mutation, args.format);
    EXIT_OK
}

// endregion

// region GTD Lifecycle Execution

/// Emits a single-file [`MutationResult`] for `org roam gtd inbox` or `org roam gtd project`.
#[allow(clippy::too_many_arguments)]
fn emit_single_gtd_mutation(
    command: &str,
    dry_run: bool,
    file: &Path,
    line: usize,
    action: &str,
    id: &str,
    title: &str,
    started: Instant,
    format: MutationOutputFormat,
) -> i32 {
    let mut m = MutationResult::new(command, dry_run);
    m.files_modified = 1;
    m.entries_affected = 1;
    m.details.push(MutationDetail {
        file: file.display().to_string(),
        line,
        action: action.to_string(),
        detail: Some(format!("id:{id} ({})", title.trim())),
    });
    m.finish(started, 1);
    output_mutation(&m, format);
    EXIT_OK
}

/// Executes `org roam gtd inbox`.
fn run_gtd_inbox(args: RoamGtdInboxArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let Ok((vault_name, vault_dir, cfg)) =
        resolve_target_vault(args.workspace.as_deref(), args.roam_dir.as_deref())
            .map_err(|m| eprintln!("org: {m}"))
    else {
        return EXIT_ERROR;
    };
    let inbox_path = vault_dir.join(&cfg.roam.gtd_subdir).join("inbox.org");
    let existing = if inbox_path.is_file() {
        let Ok(c) = std::fs::read_to_string(&inbox_path)
            .map_err(|e| eprintln!("org: error reading {}: {e}", inbox_path.display()))
        else {
            return EXIT_ERROR;
        };
        c
    } else {
        "#+TITLE: Inbox\n#+filetags: :agenda:\n\n".to_string()
    };
    let entry_id = mint_capture_id();
    let spec = GtdInboxEntrySpec {
        title: &args.title,
        project: args.project.as_deref(),
        body: args.body.clone(),
        context: args.context.as_deref(),
        effort: args.effort.as_deref(),
        energy: args.energy.as_deref(),
        workmode: args.workmode.as_deref(),
        scheduled: args.scheduled.as_deref(),
        deadline: args.deadline.as_deref(),
        vault_name: &vault_name,
        vault_dir: &vault_dir,
        entry_id: &entry_id,
        no_cache,
    };
    let Ok((mut combined, line_no)) = build_gtd_inbox_entry_buffer(&existing, &inbox_path, &spec)
        .map_err(|m| eprintln!("org: {m}"))
    else {
        return EXIT_ERROR;
    };
    if args.edit && !args.dry_run {
        match run_interactive_editor(&resolve_editor_command(&cfg), &combined, false) {
            Ok((edited, _)) => combined = edited,
            Err(code) => return code,
        }
    }
    if let Some(s) = sync_roam_agenda_filetag(
        &inbox_path,
        &combined,
        &cfg.roam.gtd_subdir,
        Some(&vault_dir),
    ) {
        combined = s;
    }
    if !args.dry_run {
        if let Err(e) = write_file_atomic(&inbox_path, &combined) {
            eprintln!("org: error writing {}: {e}", inbox_path.display());
            return EXIT_ERROR;
        }
        if !no_cache {
            let _ = WorkspaceCache::open_and_sync(&vault_dir);
        }
    }
    emit_single_gtd_mutation(
        "roam-gtd-inbox",
        args.dry_run,
        &inbox_path,
        line_no,
        "captured-inbox",
        &entry_id,
        &args.title,
        started,
        args.format,
    )
}

/// Executes `org roam gtd project`.
fn run_gtd_project(args: RoamGtdProjectArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let Ok((_, vault_dir, cfg)) =
        resolve_target_vault(args.workspace.as_deref(), args.roam_dir.as_deref())
            .map_err(|m| eprintln!("org: {m}"))
    else {
        return EXIT_ERROR;
    };
    let slug = args
        .slug
        .clone()
        .unwrap_or_else(|| slugify_title(&args.title));
    let proj_path = vault_dir
        .join(&cfg.roam.gtd_subdir)
        .join("projects")
        .join(format!("{slug}.org"));
    let proj_id = mint_capture_id();
    let Ok(mut content) = build_gtd_project_file_buffer(
        &args.title,
        args.category.as_deref(),
        args.first_task.as_deref(),
        &proj_path,
        &proj_id,
    )
    .map_err(|m| eprintln!("org: {m}")) else {
        return EXIT_ERROR;
    };
    if args.edit && !args.dry_run {
        match run_interactive_editor(&resolve_editor_command(&cfg), &content, false) {
            Ok((edited, _)) => content = edited,
            Err(code) => return code,
        }
    }
    if !args.dry_run {
        if let Err(e) = write_file_atomic(&proj_path, &content) {
            eprintln!("org: error writing {}: {e}", proj_path.display());
            return EXIT_ERROR;
        }
        if cfg.roam.log_to_daily {
            let (y, m, d, hh, mm, _) = current_capture_datetime();
            let _ = append_daily_capture_log(
                &vault_dir,
                &cfg.roam.daily_subdir,
                &format!("{y:04}-{m:02}-{d:02}"),
                &format!("{hh:02}:{mm:02}"),
                "project",
                &proj_id,
                args.title.trim(),
            );
        }
        if !no_cache {
            let _ = WorkspaceCache::open_and_sync(&vault_dir);
        }
    }
    emit_single_gtd_mutation(
        "roam-gtd-project",
        args.dry_run,
        &proj_path,
        1,
        "created-project",
        &proj_id,
        &args.title,
        started,
        args.format,
    )
}

/// Dispatches `org roam gtd` subcommands (`inbox`, `project`).
pub(crate) fn run_roam_gtd(command: RoamGtdCommand, no_cache: bool) -> i32 {
    match command {
        RoamGtdCommand::Inbox(args) => run_gtd_inbox(*args, no_cache),
        RoamGtdCommand::Project(args) => run_gtd_project(*args, no_cache),
    }
}

// endregion
