// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `to-task`, `discard`, and interactive `process` subcommand execution.

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{
    current_capture_datetime, mint_capture_id, resolve_target_vault, TemplateVars,
};
use org_tools_core::config::Config;
use org_tools_core::files::write_file_atomic;
use org_tools_core::fleeting::{
    build_gtd_task_for_fleeting, clean_fleeting_title, resolve_fleeting_target,
    scan_fleeting_items, update_source_after_discard, update_source_after_task_convert,
    FleetingKind, FleetingStatusFilter, FleetingTaskSpec,
};
use org_tools_core::gtd_roam::sync_roam_agenda_filetag;

use super::super::update::invalidate_warm_workspace_caches;
use super::super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use super::zettel::run_to_zettel;
use super::{FleetingDiscardArgs, FleetingProcessArgs, FleetingToTaskArgs, FleetingToZettelArgs};
use crate::{EXIT_ERROR, EXIT_OK};

// region Resolution & Cache Invalidation Helpers

/// Resolves search roots for finding the target fleeting note.
pub(crate) fn resolve_target_roots(
    workspace: Option<&Path>,
    roam_dir_arg: Option<&str>,
) -> Result<(String, PathBuf, Config, Vec<PathBuf>), i32> {
    let (v_name, v_path, v_cfg) = match resolve_target_vault(workspace, roam_dir_arg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("org: {e}");
            return Err(EXIT_ERROR);
        }
    };
    let mut roots = vec![v_path.clone()];
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let base = workspace.unwrap_or(&cwd);
    if !roots.contains(&base.to_path_buf()) {
        roots.push(base.to_path_buf());
    }
    if let Ok(cfg) = Config::load_for_dir(base) {
        for (_, p) in cfg.roam_directories() {
            if p.exists() && !roots.contains(&p) {
                roots.push(p);
            }
        }
    }
    Ok((v_name, v_path, v_cfg, roots))
}

/// Formats the current inactive Org timestamp `[YYYY-MM-DD Day HH:MM]`.
pub(crate) fn current_inactive_ts(dt: (i32, u32, u32, u32, u32, u32)) -> String {
    let vars = TemplateVars::new("temp", "temp", "temp", "temp", dt);
    vars.timestamp_inactive
}

// endregion

// region GTD Task Conversion Runner

/// Executes `org fleeting to-task`.
pub(crate) fn run_to_task(args: FleetingToTaskArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let (v_name, v_path, v_cfg, roots) =
        match resolve_target_roots(args.workspace.as_deref(), args.roam_dir.as_deref()) {
            Ok(v) => v,
            Err(code) => return code,
        };
    let item = match resolve_fleeting_target(&roots, &args.target) {
        Ok(it) => it,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let inbox_path = if let Some(ref dest) = args.to {
        PathBuf::from(dest)
    } else {
        v_path.join(&v_cfg.roam.gtd_subdir).join("inbox.org")
    };

    let existing_inbox = if inbox_path.is_file() {
        fs::read_to_string(&inbox_path).unwrap_or_default()
    } else {
        "#+title: Inbox\n#+filetags: :agenda:\n\n".to_string()
    };

    let task_id = mint_capture_id();
    let task_spec = FleetingTaskSpec {
        item: &item,
        task_title: args.title.as_deref(),
        context: args.context.as_deref(),
        effort: args.effort.as_deref(),
        scheduled: args.scheduled.as_deref(),
        deadline: args.deadline.as_deref(),
        inbox_content: &existing_inbox,
        inbox_path: &inbox_path,
        task_id: &task_id,
        vault_name: &v_name,
        vault_dir: &v_path,
        no_cache,
    };
    let (mut combined_inbox, task_line) = match build_gtd_task_for_fleeting(&task_spec) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    if let Some(synced) = sync_roam_agenda_filetag(
        &inbox_path,
        &combined_inbox,
        &v_cfg.roam.gtd_subdir,
        Some(&v_path),
    ) {
        combined_inbox = synced;
    }

    let dt = current_capture_datetime();
    let ts_inactive = current_inactive_ts(dt);
    let task_title = args.title.as_deref().unwrap_or(&item.title);
    let clean_title = clean_fleeting_title(task_title);

    let mut updated_source =
        match update_source_after_task_convert(&item, &task_id, &clean_title, &ts_inactive) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let src_path = PathBuf::from(&item.file);
    if let Some(synced) = sync_roam_agenda_filetag(
        &src_path,
        &updated_source,
        &v_cfg.roam.gtd_subdir,
        Some(&v_path),
    ) {
        updated_source = synced;
    }

    if !args.dry_run {
        if let Err(e) = write_file_atomic(&inbox_path, &combined_inbox) {
            eprintln!("org: error writing {}: {e}", inbox_path.display());
            return EXIT_ERROR;
        }
        if let Err(e) = write_file_atomic(&src_path, &updated_source) {
            eprintln!("org: error updating {}: {e}", src_path.display());
            return EXIT_ERROR;
        }
        invalidate_warm_workspace_caches(
            args.workspace.as_deref(),
            &[inbox_path.clone(), src_path.clone()],
        );
        if !no_cache {
            let _ = WorkspaceCache::open_and_sync(&v_path);
        }
    }

    let mut result = MutationResult::new("fleeting-to-task", args.dry_run);
    result.files_modified = 2;
    result.entries_affected = 2;
    result.details.push(MutationDetail {
        file: inbox_path.display().to_string(),
        line: task_line,
        action: "created-task".to_string(),
        detail: Some(format!("id:{task_id} ({clean_title})")),
    });
    result.details.push(MutationDetail {
        file: item.file.clone(),
        line: item.line,
        action: "converted-to-task".to_string(),
        detail: Some(format!("id:{task_id}")),
    });
    result.finish(started, 2);
    output_mutation(&result, args.format);
    EXIT_OK
}

// endregion

// region Discard & Interactive Process Runners

/// Executes `org fleeting discard`.
pub(crate) fn run_discard(args: FleetingDiscardArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let (_, v_path, _, roots) =
        match resolve_target_roots(args.workspace.as_deref(), args.roam_dir.as_deref()) {
            Ok(v) => v,
            Err(code) => return code,
        };
    let item = match resolve_fleeting_target(&roots, &args.target) {
        Ok(it) => it,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let src_path = PathBuf::from(&item.file);
    let dt = current_capture_datetime();
    let ts_inactive = current_inactive_ts(dt);
    let is_delete = args.delete && item.kind == FleetingKind::File;

    if is_delete {
        if !args.dry_run {
            if let Err(e) = fs::remove_file(&src_path) {
                eprintln!("org: error removing {}: {e}", src_path.display());
                return EXIT_ERROR;
            }
            invalidate_warm_workspace_caches(
                args.workspace.as_deref(),
                std::slice::from_ref(&src_path),
            );
            if !no_cache {
                let _ = WorkspaceCache::open_and_sync(&v_path);
            }
        }
    } else {
        let updated = match update_source_after_discard(&item, &ts_inactive) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };
        if !args.dry_run {
            if let Err(e) = write_file_atomic(&src_path, &updated) {
                eprintln!("org: error writing {}: {e}", src_path.display());
                return EXIT_ERROR;
            }
            invalidate_warm_workspace_caches(
                args.workspace.as_deref(),
                std::slice::from_ref(&src_path),
            );
            if !no_cache {
                let _ = WorkspaceCache::open_and_sync(&v_path);
            }
        }
    }

    let mut result = MutationResult::new("fleeting-discard", args.dry_run);
    result.files_modified = 1;
    result.entries_affected = 1;
    result.details.push(MutationDetail {
        file: item.file.clone(),
        line: item.line,
        action: if is_delete {
            "deleted".to_string()
        } else {
            "discarded".to_string()
        },
        detail: item.id.map(|id| format!("id:{id}")),
    });
    result.finish(started, 1);
    output_mutation(&result, args.format);
    EXIT_OK
}

/// Executes interactive decision loop for `org fleeting process`.
pub(crate) fn run_process(args: FleetingProcessArgs, no_cache: bool) -> i32 {
    let (_, _, _, roots) =
        match resolve_target_roots(args.workspace.as_deref(), args.roam_dir.as_deref()) {
            Ok(v) => v,
            Err(code) => return code,
        };

    let items = scan_fleeting_items(&roots, FleetingStatusFilter::Unrefined);
    if items.is_empty() {
        println!("org: zero unrefined fleeting notes found.");
        return EXIT_OK;
    }

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let total = items.len();

    for (idx, item) in items.iter().enumerate() {
        println!("\n[{}/{}] Fleeting Note: {}", idx + 1, total, item.title);
        println!("Locator: {}", item.locator);
        if !item.raw_thoughts.is_empty() {
            println!(
                "Thoughts:\n  {}",
                item.raw_thoughts
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("\n  ")
            );
        }
        print!("[r]eform to Zettel ($EDITOR), [t]ask in Inbox, [d]iscard, [s]kip, [q]uit: ");
        let _ = std::io::stdout().flush();

        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let choice = line.trim().to_ascii_lowercase();
        match choice.as_str() {
            "r" | "reform" => {
                let z_args = FleetingToZettelArgs {
                    target: item.locator.clone(),
                    title: None,
                    slug: None,
                    roam_dir: args.roam_dir.clone(),
                    parent: None,
                    links: Vec::new(),
                    body: None,
                    edit: true,
                    no_edit: false,
                    dry_run: false,
                    workspace: args.workspace.clone(),
                    format: MutationOutputFormat::Human,
                };
                let _ = run_to_zettel(z_args, no_cache);
            }
            "t" | "task" => {
                let t_args = FleetingToTaskArgs {
                    target: item.locator.clone(),
                    title: None,
                    to: None,
                    roam_dir: args.roam_dir.clone(),
                    context: None,
                    effort: None,
                    scheduled: None,
                    deadline: None,
                    dry_run: false,
                    workspace: args.workspace.clone(),
                    format: MutationOutputFormat::Human,
                };
                let _ = run_to_task(t_args, no_cache);
            }
            "d" | "discard" => {
                let d_args = FleetingDiscardArgs {
                    target: item.locator.clone(),
                    roam_dir: args.roam_dir.clone(),
                    delete: false,
                    dry_run: false,
                    workspace: args.workspace.clone(),
                    format: MutationOutputFormat::Human,
                };
                let _ = run_discard(d_args, no_cache);
            }
            "s" | "skip" => continue,
            "q" | "quit" => break,
            _ => println!("Skipped."),
        }
    }
    EXIT_OK
}

// endregion
