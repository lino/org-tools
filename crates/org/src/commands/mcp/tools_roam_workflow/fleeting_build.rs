// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Buffer assembly and transform text generators for fleeting notes.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use org_tools_core::capture::{current_capture_datetime, mint_capture_id, TemplateVars};
use org_tools_core::config::Config;
use org_tools_core::fleeting::{
    build_gtd_task_for_fleeting, build_permanent_zettel_buffer, clean_fleeting_title,
    update_source_after_task_convert, update_source_after_zettel_reform, FleetingItem,
    FleetingTaskSpec, PermanentZettelSpec,
};
use org_tools_core::gtd_roam::sync_roam_agenda_filetag;

use super::super::schema::{optional_str, optional_str_array};

/// Builds permanent Zettel content and updated fleeting source text.
pub(super) fn build_zettel_transform(
    args: &Map<String, Value>,
    item: &FleetingItem,
    v_name: &str,
    v_path: &Path,
    v_cfg: &Config,
    no_cache: bool,
) -> Result<(String, String, PathBuf, String, String), String> {
    let new_id = mint_capture_id();
    let links = optional_str_array(args, "links")?.unwrap_or_default();
    let title_override = optional_str(args, "title")?;
    let slug_override = optional_str(args, "slug")?;
    let parent_override = optional_str(args, "parent")?;
    let body_override = optional_str(args, "body")?;

    let spec = PermanentZettelSpec {
        item,
        title_override: title_override.as_deref(),
        slug_override: slug_override.as_deref(),
        parent_override: parent_override.as_deref(),
        links: &links,
        body_override: body_override.as_deref(),
        vault_name: v_name,
        vault_path: v_path,
        no_cache,
        zettel_id: &new_id,
    };
    let (fname, z_content) = build_permanent_zettel_buffer(&spec);
    let z_dest = v_path.join(&v_cfg.roam.zettel_subdir).join(&fname);
    let dt = current_capture_datetime();
    let vars = TemplateVars::new("temp", "temp", "temp", "temp", dt);
    let z_title = title_override.as_deref().unwrap_or(&item.title);
    let clean_title = clean_fleeting_title(z_title);
    let updated_src =
        update_source_after_zettel_reform(item, &new_id, &clean_title, &vars.timestamp_inactive)?;

    Ok((new_id, clean_title, z_dest, z_content, updated_src))
}

/// Builds GTD task entry and updated fleeting source text.
pub(super) fn build_task_transform(
    args: &Map<String, Value>,
    item: &FleetingItem,
    v_name: &str,
    v_path: &Path,
    v_cfg: &Config,
    inbox_path: &Path,
    no_cache: bool,
) -> Result<(String, String, String, String), String> {
    let existing = if inbox_path.is_file() {
        fs::read_to_string(inbox_path).unwrap_or_default()
    } else {
        "#+title: Inbox\n#+filetags: :agenda:\n\n".to_string()
    };
    let task_id = mint_capture_id();
    let title_override = optional_str(args, "title")?;
    let context_val = optional_str(args, "context")?;
    let effort_val = optional_str(args, "effort")?;
    let scheduled_val = optional_str(args, "scheduled")?;
    let deadline_val = optional_str(args, "deadline")?;
    let task_spec = FleetingTaskSpec {
        item,
        task_title: title_override.as_deref(),
        context: context_val.as_deref(),
        effort: effort_val.as_deref(),
        scheduled: scheduled_val.as_deref(),
        deadline: deadline_val.as_deref(),
        inbox_content: &existing,
        inbox_path,
        task_id: &task_id,
        vault_name: v_name,
        vault_dir: v_path,
        no_cache,
    };
    let (mut combined_inbox, _task_line) = build_gtd_task_for_fleeting(&task_spec)?;
    if let Some(synced) = sync_roam_agenda_filetag(
        inbox_path,
        &combined_inbox,
        &v_cfg.roam.gtd_subdir,
        Some(v_path),
    ) {
        combined_inbox = synced;
    }
    let dt = current_capture_datetime();
    let vars = TemplateVars::new("temp", "temp", "temp", "temp", dt);
    let t_title = title_override.as_deref().unwrap_or(&item.title);
    let clean_title = clean_fleeting_title(t_title);
    let mut updated_src =
        update_source_after_task_convert(item, &task_id, &clean_title, &vars.timestamp_inactive)?;
    let src_path = PathBuf::from(&item.file);
    if let Some(synced) = sync_roam_agenda_filetag(
        &src_path,
        &updated_src,
        &v_cfg.roam.gtd_subdir,
        Some(v_path),
    ) {
        updated_src = synced;
    }

    Ok((task_id, clean_title, combined_inbox, updated_src))
}
