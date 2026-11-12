// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org fleeting to-zettel` (alias `reform`) subcommand execution.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Instant;

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{current_capture_datetime, mint_capture_id};
use org_tools_core::files::write_file_atomic;
use org_tools_core::fleeting::{
    build_permanent_zettel_buffer, clean_fleeting_title, resolve_fleeting_target,
    update_source_after_zettel_reform, PermanentZettelSpec,
};

use super::super::roam_capture::{resolve_editor_command, run_interactive_editor};
use super::super::update::invalidate_warm_workspace_caches;
use super::super::{output_mutation, MutationDetail, MutationResult};
use super::task::{current_inactive_ts, resolve_target_roots};
use super::FleetingToZettelArgs;
use crate::{EXIT_ERROR, EXIT_OK};

// region Zettel Transformation Runner

/// Executes `org fleeting to-zettel` (alias `reform`).
pub(crate) fn run_to_zettel(args: FleetingToZettelArgs, no_cache: bool) -> i32 {
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

    let new_id = mint_capture_id();
    let spec = PermanentZettelSpec {
        item: &item,
        title_override: args.title.as_deref(),
        slug_override: args.slug.as_deref(),
        parent_override: args.parent.as_deref(),
        links: &args.links,
        body_override: args.body.as_deref(),
        vault_name: &v_name,
        vault_path: &v_path,
        no_cache,
        zettel_id: &new_id,
    };
    let (fname, mut zettel_content) = build_permanent_zettel_buffer(&spec);

    let should_edit = if args.no_edit {
        false
    } else if args.edit {
        true
    } else if args.body.is_some() {
        false
    } else {
        std::io::stdin().is_terminal()
    };

    if should_edit {
        let editor = resolve_editor_command(&v_cfg);
        match run_interactive_editor(&editor, &zettel_content, false) {
            Ok((edited, _)) => zettel_content = edited,
            Err(code) => return code,
        }
    }

    let zettel_dest = v_path.join(&v_cfg.roam.zettel_subdir).join(&fname);
    let dt = current_capture_datetime();
    let ts_inactive = current_inactive_ts(dt);
    let z_title = args.title.as_deref().unwrap_or(&item.title);
    let clean_z_title = clean_fleeting_title(z_title);

    let updated_source =
        match update_source_after_zettel_reform(&item, &new_id, &clean_z_title, &ts_inactive) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let src_path = PathBuf::from(&item.file);
    if !args.dry_run {
        if let Err(e) = write_file_atomic(&zettel_dest, &zettel_content) {
            eprintln!("org: error writing {}: {e}", zettel_dest.display());
            return EXIT_ERROR;
        }
        if let Err(e) = write_file_atomic(&src_path, &updated_source) {
            eprintln!("org: error updating {}: {e}", src_path.display());
            return EXIT_ERROR;
        }
        invalidate_warm_workspace_caches(
            args.workspace.as_deref(),
            &[zettel_dest.clone(), src_path.clone()],
        );
        if !no_cache {
            let _ = WorkspaceCache::open_and_sync(&v_path);
        }
    }

    let mut result = MutationResult::new("fleeting-to-zettel", args.dry_run);
    result.files_modified = 2;
    result.entries_affected = 2;
    result.details.push(MutationDetail {
        file: zettel_dest.display().to_string(),
        line: 1,
        action: "created-zettel".to_string(),
        detail: Some(format!("id:{new_id} ({clean_z_title})")),
    });
    result.details.push(MutationDetail {
        file: item.file.clone(),
        line: item.line,
        action: "reformed".to_string(),
        detail: Some(format!("id:{new_id}")),
    });
    result.finish(started, 2);
    output_mutation(&result, args.format);
    EXIT_OK
}

// endregion
