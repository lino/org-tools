// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org roam capture` subcommand handler for template-driven and `$EDITOR` note capture.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use clap::{Args, ValueHint};
use serde::Serialize;

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::capture::{
    append_daily_capture_log, current_capture_datetime, expand_template_vars, find_template,
    mint_capture_id, render_capture_entry, render_capture_file, resolve_effective_templates,
    resolve_roam_link_or_raw, resolve_target_vault, slugify_title, with_temp_capture_file,
    CaptureRenderInput, CaptureTargetKind, EffectiveCaptureTemplate, TemplateVars,
};
use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::{join_slash_rel, write_file_atomic};
use org_tools_core::source::SourceFile;

use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

// region CLI Arguments & JSON Envelope

/// Placeholder title used when `org roam capture` is launched interactively without a title.
const UNTITLED_PLACEHOLDER: &str = "Untitled";

/// CLI arguments for `org roam capture`.
#[derive(Args, Debug, Clone)]
pub(crate) struct RoamCaptureArgs {
    /// Note or entry title (optional when `--list-templates` or interactive `--edit` is used).
    pub(crate) title: Option<String>,
    /// Template name or short key from `[capture.templates.<name>]`.
    #[arg(long, short = 't')]
    pub(crate) template: Option<String>,
    /// Built-in note type (`zettel`, `concept`, `fleeting`, `project`, `technical`, `howto`, `person`, `place`, `daily`, `inbox`).
    #[arg(long = "type")]
    pub(crate) note_type: Option<String>,
    /// List all effective built-in, global, workspace, and vault capture templates.
    #[arg(long)]
    pub(crate) list_templates: bool,
    /// Target `org-roam` vault name or directory path.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Option<String>,
    /// Explicit filename slug override.
    #[arg(long)]
    pub(crate) slug: Option<String>,
    /// Comma-separated filetags or heading tags (`--tags tag1,tag2`).
    #[arg(long)]
    pub(crate) tags: Option<String>,
    /// Repeatable `:ROAM_ALIASES:` entries.
    #[arg(long = "alias")]
    pub(crate) aliases: Vec<String>,
    /// Repeatable `:ROAM_REFS:` entries.
    #[arg(long = "ref")]
    pub(crate) refs: Vec<String>,
    /// Source medium node query or citation string (`:SOURCE_MEDIUM:`).
    #[arg(long)]
    pub(crate) medium: Option<String>,
    /// Source location string such as page, chapter, or section (`:MEDIUM_LOCATION:`).
    #[arg(long)]
    pub(crate) location: Option<String>,
    /// Parent Zettel node query (`:PARENT_ZETTEL:` and Folgezettel link).
    #[arg(long)]
    pub(crate) parent: Option<String>,
    /// Repeatable related node queries linked under `* Related Concepts & Links`.
    #[arg(long = "link")]
    pub(crate) links: Vec<String>,
    /// Repeatable `KEY=VALUE` template variable bindings.
    #[arg(long = "var")]
    pub(crate) vars: Vec<String>,
    /// Inline note body text (implies headless capture unless `--edit` is passed).
    #[arg(long)]
    pub(crate) body: Option<String>,
    /// Read note body text from standard input.
    #[arg(long)]
    pub(crate) stdin: bool,
    /// Force opening `$VISUAL` / `$EDITOR` to refine the note before saving.
    #[arg(long)]
    pub(crate) edit: bool,
    /// Skip opening `$VISUAL` / `$EDITOR` and write the note headlessly.
    #[arg(long)]
    pub(crate) no_edit: bool,
    /// Preview the capture operation without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// JSON envelope emitted by `org roam capture --list-templates --format json`.
#[derive(Serialize)]
struct ListTemplatesEnvelope {
    /// Selected vault name.
    roam_dir: String,
    /// Effective templates sorted by name.
    templates: Vec<EffectiveCaptureTemplate>,
}

// endregion

// region Vault & Link Resolution Helpers

/// Renders `--list-templates` in `human` or `json` format.
fn handle_list_templates(
    vault_name: String,
    vault_cfg: &Config,
    format: MutationOutputFormat,
) -> i32 {
    let templates: Vec<EffectiveCaptureTemplate> = resolve_effective_templates(vault_cfg)
        .into_values()
        .collect();
    match format {
        MutationOutputFormat::Json => {
            let env = ListTemplatesEnvelope {
                roam_dir: vault_name,
                templates,
            };
            println!("{}", serde_json::to_string_pretty(&env).unwrap_or_default());
        }
        MutationOutputFormat::Human => {
            for t in &templates {
                println!(
                    "{:<12} [{}] ({:?}) -> {} — {}",
                    t.name,
                    t.keys.as_deref().unwrap_or("-"),
                    t.source_layer,
                    t.target,
                    t.description
                );
            }
        }
    }
    EXIT_OK
}

/// Builds [`TemplateVars`] and [`CaptureRenderInput`] from CLI arguments.
fn build_capture_context(
    args: &RoamCaptureArgs,
    vault_name: &str,
    vault_path: &Path,
    no_cache: bool,
) -> Result<(TemplateVars, CaptureRenderInput), String> {
    let title = args
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(UNTITLED_PLACEHOLDER);
    let slug = args
        .slug
        .as_deref()
        .map_or_else(|| slugify_title(title), slugify_title);
    let mut vars = TemplateVars::new(
        &mint_capture_id(),
        title,
        &slug,
        vault_name,
        current_capture_datetime(),
    );
    if args.stdin {
        let mut buf = String::new();
        std::io::stdin()
            .lock()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed reading stdin: {e}"))?;
        vars.body = buf.trim_end().to_string();
    } else if let Some(ref b) = args.body {
        vars.body = b.clone();
    }
    if let Some(ref med) = args.medium {
        vars.medium = resolve_roam_link_or_raw(vault_name, vault_path, med, no_cache);
    }
    if let Some(ref loc) = args.location {
        vars.location = loc.clone();
    }
    let mut related_links = Vec::new();
    if let Some(ref par) = args.parent {
        let resolved = resolve_roam_link_or_raw(vault_name, vault_path, par, no_cache);
        if resolved.starts_with("[[id:") {
            related_links.push(resolved.clone());
        }
        vars.parent = resolved;
    }
    for l in &args.links {
        related_links.push(resolve_roam_link_or_raw(
            vault_name, vault_path, l, no_cache,
        ));
    }
    for kv in &args.vars {
        let Some((k, v)) = kv.split_once('=') else {
            return Err(format!("invalid --var '{kv}' (expected KEY=VALUE)"));
        };
        vars.custom.insert(k.trim().to_string(), v.to_string());
    }
    let extra_tags = args
        .tags
        .as_deref()
        .map(|s| {
            s.split([',', ':'])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok((
        vars,
        CaptureRenderInput {
            extra_tags,
            aliases: args.aliases.clone(),
            refs: args.refs.clone(),
            related_links,
        },
    ))
}

// endregion

// region Interactive Editor & Execution

/// Determines whether `$VISUAL` / `$EDITOR` should be launched for `args` and `tpl`.
fn should_launch_editor(args: &RoamCaptureArgs, tpl: &EffectiveCaptureTemplate) -> bool {
    if args.no_edit {
        false
    } else if args.edit {
        true
    } else if args.body.is_some() || args.stdin {
        false
    } else {
        tpl.edit.unwrap_or(true)
    }
}

/// Resolves the active editor command from `[roam] editor`, `$VISUAL`, `$EDITOR`, or `vi`.
pub(crate) fn resolve_editor_command(cfg: &Config) -> String {
    cfg.roam
        .editor
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("VISUAL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string())
}

/// Spawns `editor_cmd` on a temporary `.org` file containing `scaffold` and returns `(content, title)`.
pub(crate) fn run_interactive_editor(
    editor_cmd: &str,
    scaffold: &str,
    was_untitled: bool,
) -> Result<(String, Option<String>), i32> {
    let mut exit_code_override: Option<i32> = None;
    let edited_res = with_temp_capture_file(scaffold, |temp_path| {
        #[cfg(unix)]
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor_cmd} \"$1\""))
            .arg("sh")
            .arg(temp_path)
            .status();
        #[cfg(not(unix))]
        let status = Command::new(editor_cmd).arg(temp_path).status();

        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => {
                exit_code_override = Some(EXIT_ISSUES);
                Err("editor aborted".to_string())
            }
            Err(e) => Err(format!("failed to launch editor '{editor_cmd}': {e}")),
        }
    });
    let edited = match edited_res {
        Ok(text) => text,
        Err(msg) => {
            if let Some(code) = exit_code_override {
                return Err(code);
            }
            eprintln!("org: {msg}");
            return Err(EXIT_ERROR);
        }
    };
    if edited.trim().is_empty() || (was_untitled && edited == scaffold) {
        return Err(EXIT_ISSUES);
    }
    let doc = OrgDocument::from_source(&SourceFile::new("capture.org", edited.clone()));
    let extracted_title = doc.file_title().map(str::to_string);
    if was_untitled
        && extracted_title
            .as_deref()
            .is_none_or(|t| t.trim().is_empty() || t == UNTITLED_PLACEHOLDER)
    {
        return Err(EXIT_ISSUES);
    }
    Ok((edited, extracted_title))
}

/// Renders and optionally edits the capture content for `tpl`.
fn prepare_capture_content(
    args: &RoamCaptureArgs,
    vault_cfg: &Config,
    tpl: &EffectiveCaptureTemplate,
    dest_path: &Path,
    vars: &mut TemplateVars,
    input: &CaptureRenderInput,
) -> Result<(String, usize), i32> {
    match tpl.kind {
        CaptureTargetKind::File => {
            let scaffold = render_capture_file(tpl, vars, input).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            if should_launch_editor(args, tpl) {
                let editor = resolve_editor_command(vault_cfg);
                let (edited, new_title) =
                    run_interactive_editor(&editor, &scaffold, args.title.is_none())?;
                if let Some(t) = new_title {
                    vars.title = t;
                }
                Ok((edited, 1))
            } else {
                Ok((scaffold, 1))
            }
        }
        CaptureTargetKind::Entry => {
            let existing = fs::read_to_string(dest_path).unwrap_or_default();
            render_capture_entry(&existing, dest_path, tpl, vars, input).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })
        }
    }
}

/// Executes `org roam capture`.
pub(crate) fn run_roam_capture(args: RoamCaptureArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let (vault_name, vault_path, vault_cfg) =
        match resolve_target_vault(args.workspace.as_deref(), args.roam_dir.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };
    if args.list_templates {
        return handle_list_templates(vault_name, &vault_cfg, args.format);
    }
    let templates = resolve_effective_templates(&vault_cfg);
    let selector = args
        .template
        .as_deref()
        .or(args.note_type.as_deref())
        .unwrap_or("zettel");
    let Some(tpl) = find_template(&templates, selector).cloned() else {
        eprintln!("org: unknown capture template or type: {selector}");
        return EXIT_ERROR;
    };
    let (mut vars, input) = match build_capture_context(&args, &vault_name, &vault_path, no_cache) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    let dest_path = join_slash_rel(&vault_path, &expand_template_vars(&tpl.target, &vars));
    let (final_content, affected_line) =
        match prepare_capture_content(&args, &vault_cfg, &tpl, &dest_path, &mut vars, &input) {
            Ok(v) => v,
            Err(code) => return code,
        };
    if !args.dry_run {
        if let Err(e) = write_file_atomic(&dest_path, &final_content) {
            eprintln!("org: error writing {}: {e}", dest_path.display());
            return EXIT_ERROR;
        }
        if vault_cfg.roam.log_to_daily {
            let hhmm = format!("{}:{}", &vars.timestamp[8..10], &vars.timestamp[10..12]);
            let rtype = tpl.roam_type.as_deref().unwrap_or(&tpl.name);
            let _ = append_daily_capture_log(
                &vault_path,
                &vault_cfg.roam.daily_subdir,
                &vars.date,
                &hhmm,
                rtype,
                &vars.id,
                &vars.title,
            );
        }
        if !no_cache {
            let _ = WorkspaceCache::open_and_sync(&vault_path);
        }
    }
    let mut result = MutationResult::new("roam capture", args.dry_run);
    result.files_modified = 1;
    result.entries_affected = 1;
    result.details.push(MutationDetail {
        file: dest_path.display().to_string(),
        line: affected_line,
        action: "captured".to_string(),
        detail: Some(format!("id:{} ({})", vars.id, vars.title)),
    });
    result.finish(started, 1);
    output_mutation(&result, args.format);
    EXIT_OK
}

// endregion
