// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org fleeting capture` subcommand execution.

use std::io::{IsTerminal, Read as _};
use std::time::Instant;

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{
    append_daily_capture_log, current_capture_datetime, mint_capture_id, resolve_target_vault,
};
use org_tools_core::document::OrgDocument;
use org_tools_core::files::write_file_atomic;
use org_tools_core::fleeting::{build_fleeting_capture_file, FleetingCaptureSpec};
use org_tools_core::source::SourceFile;

use super::super::roam_capture::{resolve_editor_command, run_interactive_editor};
use super::super::{output_mutation, MutationDetail, MutationResult};
use super::FleetingCaptureArgs;
use crate::{EXIT_ERROR, EXIT_OK};

// region Capture Execution Helpers

/// Reads inline body or standard input text when requested.
fn resolve_capture_body(body_arg: Option<&str>, read_stdin: bool) -> Result<Option<String>, i32> {
    if read_stdin {
        let mut buf = String::new();
        if let Err(e) = std::io::stdin().lock().read_to_string(&mut buf) {
            eprintln!("org: error reading stdin: {e}");
            return Err(EXIT_ERROR);
        }
        Ok(Some(buf.trim_end().to_string()))
    } else {
        Ok(body_arg.map(str::to_string))
    }
}

/// Decides whether interactive editor should launch for `args`.
fn should_edit(args: &FleetingCaptureArgs) -> bool {
    if args.no_edit {
        false
    } else if args.edit {
        true
    } else if args.body.is_some() || args.stdin {
        false
    } else {
        std::io::stdin().is_terminal()
    }
}

/// Handles optional interactive editor refinement on `content`.
fn refine_with_editor(
    editor_cmd: &str,
    content: &str,
    was_untitled: bool,
) -> Result<(String, Option<String>), i32> {
    run_interactive_editor(editor_cmd, content, was_untitled)
}

// endregion

// region Public Runner

/// Executes `org fleeting capture`.
pub(crate) fn run_capture(args: FleetingCaptureArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let (vault_name, vault_path, vault_cfg) =
        match resolve_target_vault(args.workspace.as_deref(), args.roam_dir.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let title_input = args
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let was_untitled = title_input.is_none();
    let title = title_input.unwrap_or("Untitled");

    let body = match resolve_capture_body(args.body.as_deref(), args.stdin) {
        Ok(b) => b,
        Err(code) => return code,
    };

    let note_id = mint_capture_id();
    let dt = current_capture_datetime();
    let spec = FleetingCaptureSpec {
        title,
        body: body.as_deref(),
        medium: args.medium.as_deref(),
        location: args.location.as_deref(),
        zettel: args.zettel.as_deref(),
        source_file: args.source_file.as_deref(),
        vault_name: &vault_name,
        vault_path: &vault_path,
        fleeting_subdir: &vault_cfg.roam.fleeting_subdir,
        note_id: &note_id,
        dt,
        no_cache,
    };

    let (mut fname, mut content) = build_fleeting_capture_file(&spec);

    if should_edit(&args) {
        let editor = resolve_editor_command(&vault_cfg);
        match refine_with_editor(&editor, &content, was_untitled) {
            Ok((edited, opt_title)) => {
                content = edited;
                if let Some(new_title) = opt_title {
                    let doc =
                        OrgDocument::from_source(&SourceFile::new("temp.org", content.clone()));
                    let final_title = doc.file_title().unwrap_or(&new_title);
                    let (y, m, d, hh, mm, ss) = dt;
                    let ts = format!("{y:04}{m:02}{d:02}{hh:02}{mm:02}{ss:02}");
                    fname = format!(
                        "{ts}-{}.org",
                        org_tools_core::capture::slugify_title(final_title)
                    );
                }
            }
            Err(code) => return code,
        }
    }

    let dest_path = vault_path
        .join(&vault_cfg.roam.fleeting_subdir)
        .join(&fname);

    if !args.dry_run {
        if let Err(e) = write_file_atomic(&dest_path, &content) {
            eprintln!("org: error writing {}: {e}", dest_path.display());
            return EXIT_ERROR;
        }
        if vault_cfg.roam.log_to_daily {
            let (y, m, d, hh, mm, _) = dt;
            let date = format!("{y:04}-{m:02}-{d:02}");
            let time_str = format!("{hh:02}:{mm:02}");
            let _ = append_daily_capture_log(
                &vault_path,
                &vault_cfg.roam.daily_subdir,
                &date,
                &time_str,
                "fleeting",
                &note_id,
                title,
            );
        }
        if !no_cache {
            let _ = WorkspaceCache::open_and_sync(&vault_path);
        }
    }

    let mut result = MutationResult::new("fleeting-capture", args.dry_run);
    result.files_modified = 1;
    result.entries_affected = 1;
    result.details.push(MutationDetail {
        file: dest_path.display().to_string(),
        line: 1,
        action: "captured".to_string(),
        detail: Some(format!("id:{note_id} ({title})")),
    });
    result.finish(started, 1);
    output_mutation(&result, args.format);
    EXIT_OK
}

// endregion
