// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Variable expansion and `org-roam` v2 file/entry rendering for capture templates.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::expand_config_path;
use crate::document::OrgDocument;
use crate::entry::{add_entry, NewEntryOpts};
use crate::files::write_file_atomic;
use crate::property::{
    finalize_added_todo, normalize_token, resolve_tag_column, FinalizeTodoOptions,
};
use crate::source::SourceFile;

use super::{CaptureRenderInput, EffectiveCaptureTemplate, TemplateVars};

/// Looks up a single placeholder variable name inside `vars`.
fn lookup_var<'a>(key: &str, vars: &'a TemplateVars) -> Option<&'a str> {
    match key.trim() {
        "id" => Some(&vars.id),
        "title" => Some(&vars.title),
        "slug" => Some(&vars.slug),
        "date" => Some(&vars.date),
        "year" => Some(&vars.year),
        "month" => Some(&vars.month),
        "timestamp" => Some(&vars.timestamp),
        "timestamp_inactive" => Some(&vars.timestamp_inactive),
        "timestamp_active" => Some(&vars.timestamp_active),
        "roam_dir" => Some(&vars.roam_dir),
        "body" => Some(&vars.body),
        "medium" => Some(&vars.medium),
        "location" => Some(&vars.location),
        "parent" => Some(&vars.parent),
        other => vars.custom.get(other).map(String::as_str),
    }
}

/// Expands `${...}` variable placeholders in `raw` against `vars`.
pub fn expand_template_vars(raw: &str, vars: &TemplateVars) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            if let Some(val) = lookup_var(&after[..end], vars) {
                out.push_str(val);
            } else {
                out.push_str(&rest[start..start + 3 + end]);
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Formats a token list for `:ROAM_ALIASES:` or `:ROAM_REFS:`, quoting items containing spaces.
pub fn format_roam_token_list(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| {
            let s = t.trim();
            if s.contains(char::is_whitespace) && !s.starts_with('"') {
                format!("\"{}\"", s.replace('"', "\\\""))
            } else {
                s.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Formats an `F005`-aligned `:PROPERTIES:` drawer from ordered `(key, value)` pairs.
pub fn format_aligned_properties_drawer(props: &[(String, String)]) -> String {
    let max_len = props.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut out = String::from(":PROPERTIES:\n");
    for (k, v) in props {
        let pad = if props.len() > 1 {
            " ".repeat(max_len.saturating_sub(k.len()))
        } else {
            String::new()
        };
        out.push_str(&format!(":{k}:{pad} {v}\n"));
    }
    out.push_str(":END:\n");
    out
}

/// Loads and expands the template body from `body_file` or inline `body`.
fn resolve_expanded_body(
    tpl: &EffectiveCaptureTemplate,
    vars: &TemplateVars,
    related_links: &[String],
) -> Result<String, String> {
    let raw_body = if let Some(ref bf) = tpl.body_file {
        let path = expand_config_path(bf, tpl.base_dir.as_deref().unwrap_or(Path::new(".")));
        fs::read_to_string(&path)
            .map_err(|e| format!("failed to read template body_file {}: {e}", path.display()))?
    } else if let Some(ref b) = tpl.body {
        b.clone()
    } else if !vars.body.is_empty() {
        format!("{}\n", vars.body)
    } else {
        String::new()
    };
    let mut expanded = expand_template_vars(&raw_body, vars);
    if !related_links.is_empty() {
        let bullets = related_links
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        let marker = "* Related Concepts & Links\n";
        if let Some(pos) = expanded.find(marker) {
            expanded.insert_str(pos + marker.len(), &format!("{bullets}\n"));
        } else {
            if !expanded.ends_with('\n') {
                expanded.push('\n');
            }
            expanded.push_str(&format!("\n{marker}{bullets}\n"));
        }
    }
    Ok(expanded)
}

/// Renders a `kind = "file"` `org-roam` v2 document from `tpl`, `vars`, and `input`.
pub fn render_capture_file(
    tpl: &EffectiveCaptureTemplate,
    vars: &TemplateVars,
    input: &CaptureRenderInput,
) -> Result<String, String> {
    let mut props = vec![("ID".to_string(), vars.id.clone())];
    if let Some(ref rt) = tpl.roam_type {
        props.push(("TYPE".to_string(), expand_template_vars(rt, vars)));
    }
    if !input.aliases.is_empty() {
        props.push((
            "ROAM_ALIASES".into(),
            format_roam_token_list(&input.aliases),
        ));
    }
    if !input.refs.is_empty() {
        props.push(("ROAM_REFS".into(), format_roam_token_list(&input.refs)));
    }
    for (key, val) in [
        ("SOURCE_MEDIUM", &vars.medium),
        ("MEDIUM_LOCATION", &vars.location),
        ("PARENT_ZETTEL", &vars.parent),
    ] {
        if !val.is_empty() {
            props.push((key.to_string(), val.clone()));
        }
    }
    for (k, v) in &tpl.properties {
        let exp = expand_template_vars(v, vars);
        if !exp.is_empty() && !props.iter().any(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            props.push((k.clone(), exp));
        }
    }
    let mut out = format_aligned_properties_drawer(&props);
    if let Some(ref th) = tpl.todo_header {
        let t = th.trim();
        out.push_str(&if t.starts_with("#+") {
            format!("{t}\n")
        } else {
            format!("#+TODO: {t}\n")
        });
    }
    if let Some(ref head) = tpl.head {
        out.push_str(&format!("{}\n", expand_template_vars(head.trim(), vars)));
    }
    out.push_str(&format!("#+title: {}\n", vars.title));
    let mut tags = tpl.filetags.clone();
    for t in &input.extra_tags {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }
    if !tags.is_empty() {
        out.push_str(&format!("#+filetags: :{}: \n", tags.join(":")).replace(": \n", ":\n"));
    }
    let body = resolve_expanded_body(tpl, vars, &input.related_links)?;
    if !body.trim().is_empty() {
        out.push_str(&format!("\n{}\n", body.trim_end()));
    }
    Ok(out)
}

/// Renders and inserts a `kind = "entry"` capture into `existing_content` (creating `target_heading` if needed).
pub fn render_capture_entry(
    existing_content: &str,
    dest_path: &Path,
    tpl: &EffectiveCaptureTemplate,
    vars: &TemplateVars,
    input: &CaptureRenderInput,
) -> Result<(String, usize), String> {
    let mut base = if existing_content.trim().is_empty() {
        let stem = dest_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Capture");
        format!("#+title: {stem}\n")
    } else {
        existing_content.to_string()
    };
    let th_title = tpl
        .target_heading
        .as_deref()
        .map(|h| h.trim().trim_start_matches('*').trim().to_string());
    if let Some(ref th) = th_title {
        let doc = OrgDocument::from_source(&SourceFile::new(dest_path, base.clone()));
        if !doc.entries.iter().any(|e| e.title.eq_ignore_ascii_case(th)) {
            if !base.ends_with('\n') {
                base.push('\n');
            }
            if !base.ends_with("\n\n") {
                base.push('\n');
            }
            base.push_str(&format!("* {th}\n"));
        }
    }
    let src = SourceFile::new(dest_path, base);
    let doc = OrgDocument::from_source(&src);
    let parent_idx = th_title.as_ref().and_then(|th| {
        doc.entries
            .iter()
            .position(|e| e.title.eq_ignore_ascii_case(th))
    });
    let level = parent_idx.map_or(1, |idx| doc.entries[idx].level + 1);
    let mut tags = tpl.filetags.clone();
    for t in &input.extra_tags {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }
    let added = add_entry(
        &src,
        &doc,
        parent_idx,
        &NewEntryOpts {
            title: vars.title.clone(),
            level,
            keyword: (tpl.name == "inbox").then(|| "TODO".to_string()),
            priority: None,
            tags,
            scheduled: None,
            deadline: None,
        },
    );
    let mut props = vec![("ID".to_string(), vars.id.clone())];
    for (k, v) in &tpl.properties {
        let exp = expand_template_vars(v, vars);
        if !exp.is_empty() {
            props.push((k.clone(), exp));
        }
    }
    let body = resolve_expanded_body(tpl, vars, &input.related_links)?;
    let fin = finalize_added_todo(
        false,
        &added.content,
        dest_path,
        parent_idx,
        &FinalizeTodoOptions {
            properties: props,
            body: (!body.trim().is_empty()).then(|| body.trim().to_string()),
            tag_column: resolve_tag_column(None),
        },
    )?;
    Ok((fin.content, fin.heading_line))
}

/// Appends `- HH:MM [<type>] :: [[id:<id>][<title>]]` under `* Captured today` in `<daily_subdir>/<date>.org`.
pub fn append_daily_capture_log(
    vault_path: &Path,
    daily_subdir: &str,
    date_str: &str,
    time_hhmm: &str,
    note_type: &str,
    id: &str,
    title: &str,
) -> Result<PathBuf, String> {
    let daily_file = vault_path
        .join(daily_subdir)
        .join(format!("{date_str}.org"));
    let log_line = format!("- {time_hhmm} [{note_type}] :: [[id:{id}][{title}]]");
    let mut content = if daily_file.is_file() {
        fs::read_to_string(&daily_file).map_err(|e| e.to_string())?
    } else {
        format!("#+title: {date_str}\n#+filetags: :daily:\n\n* Captured today\n")
    };
    if let Some(pos) = content.find("* Captured today") {
        let insert_pos = content[pos + 1..]
            .find("\n* ")
            .map_or(content.len(), |idx| pos + 1 + idx);
        let nl = if insert_pos > 0 && !content[..insert_pos].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        content.insert_str(insert_pos, &format!("{nl}{log_line}\n"));
    } else {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("\n* Captured today\n{log_line}\n"));
    }
    write_file_atomic(&daily_file, &content).map_err(|e| e.to_string())?;
    Ok(daily_file)
}

/// Normalizes a note title into a filesystem-safe lowercase slug.
pub fn slugify_title(raw: &str) -> String {
    let s = normalize_token(raw, false);
    if s.is_empty() {
        "untitled".to_string()
    } else {
        s
    }
}

/// Generates a fresh lowercase RFC 9562 UUIDv4 string for a captured note or entry.
pub fn mint_capture_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Writes `scaffold` to a temporary `.org` file, invokes `runner(path)`, and returns the edited text.
pub fn with_temp_capture_file<F>(scaffold: &str, runner: F) -> Result<String, String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let temp = tempfile::Builder::new()
        .prefix("org-capture-")
        .suffix(".org")
        .tempfile()
        .map_err(|e| format!("failed to create temporary capture buffer: {e}"))?;
    fs::write(temp.path(), scaffold)
        .map_err(|e| format!("failed to write temporary capture buffer: {e}"))?;
    runner(temp.path())?;
    fs::read_to_string(temp.path())
        .map_err(|e| format!("failed to read temporary capture buffer: {e}"))
}
