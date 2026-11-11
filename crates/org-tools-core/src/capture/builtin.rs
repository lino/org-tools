// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Built-in capture template catalog and hierarchical overlay merging.

use std::collections::BTreeMap;

use crate::config::{Config, RoamConfig};

use super::{
    CaptureTargetKind, CaptureTemplateConfig, CaptureTemplateLayer, EffectiveCaptureTemplate,
};

/// Default Zettelkasten concept body scaffold.
const ZETTEL_BODY: &str = "* Thesis\n${body}\n\n* Core Concept\n\n* Context & Significance\n\n* Related Concepts & Links\n\n* Open Questions & Folgezettel\n";

/// Tuple descriptor used to construct built-in capture templates concisely.
type BuiltinSpec<'a> = (
    &'a str,
    &'a str,
    &'a str,
    CaptureTargetKind,
    String,
    Option<&'a str>,
    &'a [&'a str],
    &'a [(&'a str, &'a str)],
    &'a str,
);

/// Returns the built-in capture templates parameterized by `roam_cfg` subdirectory settings.
pub fn builtin_capture_templates(
    roam_cfg: &RoamConfig,
) -> BTreeMap<String, EffectiveCaptureTemplate> {
    let z_tgt = format!("{}/${{slug}}.org", roam_cfg.zettel_subdir);
    let specs: [BuiltinSpec<'_>; 10] = [
        (
            "zettel",
            "Permanent Zettelkasten concept note",
            "z",
            CaptureTargetKind::File,
            z_tgt.clone(),
            Some("concept"),
            &[],
            &[],
            ZETTEL_BODY,
        ),
        (
            "concept",
            "Permanent Zettelkasten concept note",
            "c",
            CaptureTargetKind::File,
            z_tgt,
            Some("concept"),
            &[],
            &[],
            ZETTEL_BODY,
        ),
        (
            "technical",
            "Technical architecture or troubleshooting note",
            "t",
            CaptureTargetKind::File,
            "tech/${slug}.org".into(),
            Some("technical"),
            &[],
            &[],
            "* Problem\n${body}\n\n* Solution\n\n* References\n",
        ),
        (
            "howto",
            "Step-by-step how-to guide or recipe",
            "h",
            CaptureTargetKind::File,
            "howto/${slug}.org".into(),
            Some("howto"),
            &[],
            &[],
            "* Goal\n${body}\n\n* Prerequisites\n\n* Steps\n\n* Verification\n\n* References\n",
        ),
        (
            "person",
            "Person or contact reference note",
            "P",
            CaptureTargetKind::File,
            "people/${slug}.org".into(),
            Some("person"),
            &[],
            &[],
            "* Context\n${body}\n\n* Notes\n",
        ),
        (
            "place",
            "Place or location reference note",
            "l",
            CaptureTargetKind::File,
            "places/${slug}.org".into(),
            Some("place"),
            &[],
            &[],
            "* Overview\n${body}\n\n* Notes\n",
        ),
        (
            "project",
            "GTD project node with tasks",
            "p",
            CaptureTargetKind::File,
            format!("{}/projects/${{slug}}.org", roam_cfg.gtd_subdir),
            Some("project"),
            &["project", "agenda"],
            &[("STATUS", "active")],
            "* Description\n${body}\n\n* Tasks\n",
        ),
        (
            "fleeting",
            "Luhmann fleeting thought note",
            "f",
            CaptureTargetKind::File,
            format!("{}/${{timestamp}}-${{slug}}.org", roam_cfg.fleeting_subdir),
            Some("fleeting"),
            &["fleeting"],
            &[
                ("STATUS", "unrefined"),
                ("CAPTURED_AT", "${timestamp_inactive}"),
            ],
            "* Fleeting Thoughts\n${body}\n\n* Context & Source\n",
        ),
        (
            "daily",
            "Daily journal and capture log note",
            "d",
            CaptureTargetKind::File,
            format!("{}/${{date}}.org", roam_cfg.daily_subdir),
            Some("daily"),
            &["daily"],
            &[],
            "* Notes\n${body}\n\n* Captured today\n",
        ),
        (
            "inbox",
            "GTD inbox task entry",
            "i",
            CaptureTargetKind::Entry,
            format!("{}/inbox.org", roam_cfg.gtd_subdir),
            None,
            &[],
            &[("CAPTURED_AT", "${timestamp_inactive}")],
            "${body}",
        ),
    ];
    specs
        .into_iter()
        .map(|(n, d, k, kind, target, rt, tags, props, body)| {
            (
                n.to_string(),
                EffectiveCaptureTemplate {
                    name: n.to_string(),
                    description: d.to_string(),
                    keys: Some(k.to_string()),
                    kind,
                    target,
                    target_heading: None,
                    roam_type: rt.map(str::to_string),
                    properties: props
                        .iter()
                        .map(|(pk, pv)| ((*pk).into(), (*pv).into()))
                        .collect(),
                    filetags: tags.iter().map(|t| (*t).to_string()).collect(),
                    todo_header: None,
                    head: None,
                    body: Some(body.to_string()),
                    body_file: None,
                    edit: None,
                    source_layer: CaptureTemplateLayer::Builtin,
                    base_dir: None,
                },
            )
        })
        .collect()
}

/// Applies a user-configured [`CaptureTemplateConfig`] overlay onto an existing or new template.
fn apply_template_overlay(
    name: &str,
    existing: Option<EffectiveCaptureTemplate>,
    ov: &CaptureTemplateConfig,
    roam_cfg: &RoamConfig,
) -> EffectiveCaptureTemplate {
    let mut b = existing.unwrap_or_else(|| EffectiveCaptureTemplate {
        name: name.to_string(),
        description: format!("Custom capture template '{name}'"),
        keys: None,
        kind: CaptureTargetKind::File,
        target: format!("{}/${{slug}}.org", roam_cfg.zettel_subdir),
        target_heading: None,
        roam_type: None,
        properties: BTreeMap::new(),
        filetags: Vec::new(),
        todo_header: None,
        head: None,
        body: None,
        body_file: None,
        edit: None,
        source_layer: ov.source_layer,
        base_dir: ov.base_dir.clone(),
    });
    if let Some(ref d) = ov.description {
        b.description = d.clone();
    }
    if ov.keys.is_some() {
        b.keys = ov.keys.clone();
    }
    if let Some(k) = ov.kind {
        b.kind = k;
    }
    if let Some(ref t) = ov.target {
        b.target = t.clone();
    }
    if ov.target_heading.is_some() {
        b.target_heading = ov.target_heading.clone();
    }
    if ov.roam_type.is_some() {
        b.roam_type = ov.roam_type.clone();
    }
    b.properties
        .extend(ov.properties.iter().map(|(k, v)| (k.clone(), v.clone())));
    if !ov.filetags.is_empty() {
        b.filetags = ov.filetags.clone();
    }
    if ov.todo_header.is_some() {
        b.todo_header = ov.todo_header.clone();
    }
    if ov.head.is_some() {
        b.head = ov.head.clone();
    }
    if ov.body.is_some() {
        b.body = ov.body.clone();
    }
    if ov.body_file.is_some() {
        b.body_file = ov.body_file.clone();
    }
    if ov.edit.is_some() {
        b.edit = ov.edit;
    }
    b.source_layer = ov.source_layer;
    if ov.base_dir.is_some() {
        b.base_dir = ov.base_dir.clone();
    }
    b
}

/// Merges built-in templates with configured `[capture.templates.<name>]` entries from `cfg`.
pub fn resolve_effective_templates(cfg: &Config) -> BTreeMap<String, EffectiveCaptureTemplate> {
    let mut map = builtin_capture_templates(&cfg.roam);
    for (name, ov) in &cfg.capture.templates {
        let prev = map.remove(name);
        map.insert(
            name.clone(),
            apply_template_overlay(name, prev, ov, &cfg.roam),
        );
    }
    map
}

/// Finds a template by exact name, case-insensitive name, or short `keys` selector.
pub fn find_template<'a>(
    templates: &'a BTreeMap<String, EffectiveCaptureTemplate>,
    selector: &str,
) -> Option<&'a EffectiveCaptureTemplate> {
    let trimmed = selector.trim();
    templates
        .get(trimmed)
        .or_else(|| {
            templates
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(trimmed))
                .map(|(_, v)| v)
        })
        .or_else(|| {
            templates
                .values()
                .find(|t| t.keys.as_deref() == Some(trimmed))
        })
}
